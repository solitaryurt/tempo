//! Binary snapshot of the catalog browse tables for fast startup.
//!
//! On startup, loading 8k+ tracks/artists/albums via three sequential SQLite
//! queries dominates the critical path (~300 ms in our reference library).
//! SQLite is still the source of truth, but we keep an opportunistic binary
//! snapshot of the same data that hydrates in a single sequential read with
//! no SQL parsing, no statement preparation, and no per-row allocations
//! beyond the final `String`/`PathBuf`.
//!
//! ## File format
//!
//! All integers are little-endian. Strings are length-prefixed (`u32` byte
//! length followed by raw UTF-8 bytes). `Option<String>` and
//! `Option<PathBuf>` use a single byte tag (0 = None, 1 = Some) followed by
//! the value when present.
//!
//! ```text
//! magic: 8 bytes = b"TEMPO_S1"
//! version: u32  = SNAPSHOT_VERSION
//! roots_hash: u64  (FNV-1a of joined root paths; if mismatched, refuse load)
//! tracks_count: u32
//! artists_count: u32
//! albums_count: u32
//! tracks: tracks_count * Track record
//! artists: artists_count * Artist record
//! albums: albums_count * Album record
//! ```
//!
//! On any read error or version/header mismatch, the loader returns `None`
//! and startup falls back to SQLite. The snapshot is rewritten asynchronously
//! after every successful scan finish (and also at shutdown) so it stays
//! reasonably fresh without ever blocking startup.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};

use crate::{
    catalog::{CatalogAlbum, CatalogArtist, CatalogTrack},
    perf,
};

const MAGIC: &[u8; 8] = b"TEMPO_S1";
// v3: collaboration splitter now treats `/`, `\`, and `;` as artist
// separators (e.g. "John Lennon/Yoko Ono" -> ["John Lennon", "Yoko Ono"]).
// Bumped to invalidate snapshots written by builds that used the
// feat-only splitter, so upgraded users see the corrected Artists view
// without waiting for a rescan.
const SNAPSHOT_VERSION: u32 = 4;
const SNAPSHOT_FILE: &str = "startup_snapshot.v3.bin";

pub struct StartupSnapshot {
    pub tracks: Vec<CatalogTrack>,
    pub artists: Vec<CatalogArtist>,
    pub albums: Vec<CatalogAlbum>,
}

/// Stable hash of the configured library roots. The snapshot is only valid
/// when the same set of roots is requested at startup.
pub fn roots_hash(roots: &[PathBuf]) -> u64 {
    // FNV-1a 64-bit; small, stable, no extra deps. Sort the inputs so order
    // does not invalidate an otherwise compatible snapshot.
    let mut sorted: Vec<&str> = roots.iter().filter_map(|root| root.to_str()).collect();
    sorted.sort();

    let mut hash: u64 = 0xcbf29ce484222325;
    for root in sorted {
        for byte in root.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn snapshot_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(SNAPSHOT_FILE)
}

pub fn load(cache_dir: &Path, roots: &[PathBuf]) -> Option<StartupSnapshot> {
    let _span = perf::span("snapshot.load", "");
    let path = snapshot_path(cache_dir);
    let bytes = fs::read(&path).ok()?;
    perf::event("snapshot.load.bytes", format!("bytes={}", bytes.len()));

    match decode(&bytes, roots) {
        Ok(snapshot) => {
            perf::event(
                "snapshot.load.count",
                format!(
                    "tracks={} artists={} albums={}",
                    snapshot.tracks.len(),
                    snapshot.artists.len(),
                    snapshot.albums.len()
                ),
            );
            Some(snapshot)
        }
        Err(error) => {
            perf::event("snapshot.load.invalid", format!("error={error:#}"));
            None
        }
    }
}

pub fn save(
    cache_dir: &Path,
    roots: &[PathBuf],
    tracks: &[CatalogTrack],
    artists: &[CatalogArtist],
    albums: &[CatalogAlbum],
) -> Result<()> {
    let _span = perf::span(
        "snapshot.save",
        format!(
            "tracks={} artists={} albums={}",
            tracks.len(),
            artists.len(),
            albums.len()
        ),
    );
    fs::create_dir_all(cache_dir).with_context(|| {
        format!(
            "failed to create snapshot cache dir {}",
            cache_dir.display()
        )
    })?;
    let path = snapshot_path(cache_dir);
    let tmp_path = path.with_extension("tmp");

    let mut buffer = Vec::with_capacity(estimate_capacity(tracks, artists, albums));
    encode(&mut buffer, roots, tracks, artists, albums)?;

    {
        let mut file = fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create snapshot temp {}", tmp_path.display()))?;
        file.write_all(&buffer)
            .context("failed to write snapshot bytes")?;
        file.sync_data().ok();
    }

    fs::rename(&tmp_path, &path)
        .with_context(|| format!("failed to commit snapshot to {}", path.display()))?;
    perf::event("snapshot.save.bytes", format!("bytes={}", buffer.len()));
    Ok(())
}

fn estimate_capacity(
    tracks: &[CatalogTrack],
    artists: &[CatalogArtist],
    albums: &[CatalogAlbum],
) -> usize {
    // Rough heuristic: average ~256 bytes per track, ~128 per artist/album.
    8 + 4 + 8 + 4 * 3 + tracks.len() * 256 + artists.len() * 128 + albums.len() * 128
}

fn encode(
    buffer: &mut Vec<u8>,
    roots: &[PathBuf],
    tracks: &[CatalogTrack],
    artists: &[CatalogArtist],
    albums: &[CatalogAlbum],
) -> Result<()> {
    buffer.extend_from_slice(MAGIC);
    write_u32(buffer, SNAPSHOT_VERSION);
    write_u64(buffer, roots_hash(roots));
    write_u32(buffer, tracks.len() as u32);
    write_u32(buffer, artists.len() as u32);
    write_u32(buffer, albums.len() as u32);

    for track in tracks {
        write_track(buffer, track);
    }
    for artist in artists {
        write_artist(buffer, artist);
    }
    for album in albums {
        write_album(buffer, album);
    }

    Ok(())
}

fn decode(bytes: &[u8], roots: &[PathBuf]) -> Result<StartupSnapshot> {
    let mut cursor = Cursor::new(bytes);
    let mut magic = [0u8; 8];
    cursor.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(anyhow!("snapshot magic mismatch"));
    }
    let version = cursor.read_u32()?;
    if version != SNAPSHOT_VERSION {
        return Err(anyhow!("snapshot version mismatch: got {version}"));
    }
    let stored_hash = cursor.read_u64()?;
    if stored_hash != roots_hash(roots) {
        return Err(anyhow!("snapshot roots hash mismatch"));
    }

    let tracks_count = cursor.read_u32()? as usize;
    let artists_count = cursor.read_u32()? as usize;
    let albums_count = cursor.read_u32()? as usize;

    let mut tracks = Vec::with_capacity(tracks_count);
    for _ in 0..tracks_count {
        tracks.push(read_track(&mut cursor)?);
    }
    let mut artists = Vec::with_capacity(artists_count);
    for _ in 0..artists_count {
        artists.push(read_artist(&mut cursor)?);
    }
    let mut albums = Vec::with_capacity(albums_count);
    for _ in 0..albums_count {
        albums.push(read_album(&mut cursor)?);
    }

    Ok(StartupSnapshot {
        tracks,
        artists,
        albums,
    })
}

// ---------------------------------------------------------------------------
// Per-record encoders/decoders
// ---------------------------------------------------------------------------

fn write_track(buffer: &mut Vec<u8>, track: &CatalogTrack) {
    write_i64(buffer, track.track_id);
    write_i64(buffer, track.file_id);
    write_i64(buffer, track.artist_id);
    write_i64(buffer, track.album_id);
    write_path(buffer, &track.path);
    write_str(buffer, &track.title);
    write_str(buffer, &track.artist);
    write_str(buffer, &track.album);
    write_opt_str(buffer, track.genre.as_deref());
    write_opt_u32(buffer, track.track_number);
    write_opt_str(buffer, track.year.as_deref());
    write_u64(buffer, system_time_to_millis(track.date_added));
    write_u64(buffer, track.duration.as_millis() as u64);
    write_str(buffer, &track.codec);
    write_opt_u32(buffer, track.bitrate);
    write_u64(buffer, track.file_size);
    write_u32(buffer, track.play_count);
    buffer.push(if track.liked { 1 } else { 0 });
    write_opt_path(buffer, track.artwork_path.as_deref());
}

fn read_track(cursor: &mut Cursor<'_>) -> Result<CatalogTrack> {
    Ok(CatalogTrack {
        track_id: cursor.read_i64()?,
        file_id: cursor.read_i64()?,
        artist_id: cursor.read_i64()?,
        album_id: cursor.read_i64()?,
        path: cursor.read_path()?,
        title: cursor.read_string()?,
        artist: cursor.read_string()?,
        album: cursor.read_string()?,
        genre: cursor.read_opt_string()?,
        track_number: cursor.read_opt_u32()?,
        year: cursor.read_opt_string()?,
        date_added: millis_to_system_time(cursor.read_u64()?),
        duration: Duration::from_millis(cursor.read_u64()?),
        codec: cursor.read_string()?,
        bitrate: cursor.read_opt_u32()?,
        file_size: cursor.read_u64()?,
        play_count: cursor.read_u32()?,
        liked: cursor.read_bool()?,
        artwork_path: cursor.read_opt_path()?,
    })
}

fn write_artist(buffer: &mut Vec<u8>, artist: &CatalogArtist) {
    write_i64(buffer, artist.artist_id);
    write_str(buffer, &artist.name);
    write_opt_str(buffer, artist.bio.as_deref());
    write_opt_path(buffer, artist.photo_path.as_deref());
    write_u64(buffer, artist.album_count as u64);
    write_u64(buffer, artist.track_count as u64);
}

fn read_artist(cursor: &mut Cursor<'_>) -> Result<CatalogArtist> {
    Ok(CatalogArtist {
        artist_id: cursor.read_i64()?,
        name: cursor.read_string()?,
        bio: cursor.read_opt_string()?,
        photo_path: cursor.read_opt_path()?,
        album_count: cursor.read_u64()? as usize,
        track_count: cursor.read_u64()? as usize,
    })
}

fn write_album(buffer: &mut Vec<u8>, album: &CatalogAlbum) {
    write_i64(buffer, album.album_id);
    write_i64(buffer, album.artist_id);
    write_str(buffer, &album.title);
    write_str(buffer, &album.artist);
    write_opt_str(buffer, album.year.as_deref());
    write_opt_path(buffer, album.artwork_path.as_deref());
    write_u64(buffer, album.track_count as u64);
}

fn read_album(cursor: &mut Cursor<'_>) -> Result<CatalogAlbum> {
    Ok(CatalogAlbum {
        album_id: cursor.read_i64()?,
        artist_id: cursor.read_i64()?,
        title: cursor.read_string()?,
        artist: cursor.read_string()?,
        year: cursor.read_opt_string()?,
        artwork_path: cursor.read_opt_path()?,
        track_count: cursor.read_u64()? as usize,
    })
}

// ---------------------------------------------------------------------------
// Cursor + primitive helpers
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        let end = self.position + buffer.len();
        if end > self.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "snapshot truncated",
            ));
        }
        buffer.copy_from_slice(&self.bytes[self.position..end]);
        self.position = end;
        Ok(())
    }

    fn read_u32(&mut self) -> Result<u32> {
        let mut buffer = [0u8; 4];
        self.read_exact(&mut buffer)?;
        Ok(u32::from_le_bytes(buffer))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let mut buffer = [0u8; 8];
        self.read_exact(&mut buffer)?;
        Ok(u64::from_le_bytes(buffer))
    }

    fn read_i64(&mut self) -> Result<i64> {
        let mut buffer = [0u8; 8];
        self.read_exact(&mut buffer)?;
        Ok(i64::from_le_bytes(buffer))
    }

    fn read_string(&mut self) -> Result<String> {
        let length = self.read_u32()? as usize;
        let end = self.position + length;
        if end > self.bytes.len() {
            return Err(anyhow!("snapshot truncated reading string"));
        }
        let bytes = &self.bytes[self.position..end];
        self.position = end;
        String::from_utf8(bytes.to_vec()).context("invalid utf-8 in snapshot")
    }

    fn read_opt_string(&mut self) -> Result<Option<String>> {
        let mut tag = [0u8; 1];
        self.read_exact(&mut tag)?;
        match tag[0] {
            0 => Ok(None),
            1 => Ok(Some(self.read_string()?)),
            other => Err(anyhow!("invalid optional tag {other}")),
        }
    }

    fn read_path(&mut self) -> Result<PathBuf> {
        Ok(PathBuf::from(self.read_string()?))
    }

    fn read_opt_path(&mut self) -> Result<Option<PathBuf>> {
        Ok(self.read_opt_string()?.map(PathBuf::from))
    }

    fn read_bool(&mut self) -> Result<bool> {
        let mut tag = [0u8; 1];
        self.read_exact(&mut tag)?;
        Ok(tag[0] != 0)
    }

    fn read_opt_u32(&mut self) -> Result<Option<u32>> {
        let mut tag = [0u8; 1];
        self.read_exact(&mut tag)?;
        match tag[0] {
            0 => Ok(None),
            1 => Ok(Some(self.read_u32()?)),
            other => Err(anyhow!("invalid optional u32 tag {other}")),
        }
    }
}

fn write_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(buffer: &mut Vec<u8>, value: u64) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn write_i64(buffer: &mut Vec<u8>, value: i64) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn write_str(buffer: &mut Vec<u8>, value: &str) {
    write_u32(buffer, value.len() as u32);
    buffer.extend_from_slice(value.as_bytes());
}

fn write_opt_str(buffer: &mut Vec<u8>, value: Option<&str>) {
    match value {
        None => buffer.push(0),
        Some(value) => {
            buffer.push(1);
            write_str(buffer, value);
        }
    }
}

fn write_path(buffer: &mut Vec<u8>, value: &Path) {
    write_str(buffer, &value.to_string_lossy());
}

fn write_opt_path(buffer: &mut Vec<u8>, value: Option<&Path>) {
    match value {
        None => buffer.push(0),
        Some(value) => {
            buffer.push(1);
            write_path(buffer, value);
        }
    }
}

fn write_opt_u32(buffer: &mut Vec<u8>, value: Option<u32>) {
    match value {
        None => buffer.push(0),
        Some(value) => {
            buffer.push(1);
            write_u32(buffer, value);
        }
    }
}

fn system_time_to_millis(value: SystemTime) -> u64 {
    value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn millis_to_system_time(millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    fn sample_track(id: i64) -> CatalogTrack {
        CatalogTrack {
            track_id: id,
            file_id: id + 1000,
            artist_id: 7,
            album_id: 9,
            path: PathBuf::from(format!("/music/song-{id}.flac")),
            title: format!("Title {id}"),
            artist: "Artist".to_string(),
            album: "Album".to_string(),
            genre: Some("Rock".to_string()),
            track_number: Some(id as u32),
            year: Some("2024".to_string()),
            date_added: UNIX_EPOCH + Duration::from_millis(1_000_000 + id as u64),
            duration: Duration::from_secs(180),
            codec: "flac".to_string(),
            bitrate: Some(900),
            file_size: 12345,
            play_count: 0,
            liked: false,
            artwork_path: None,
        }
    }

    fn sample_artist(id: i64) -> CatalogArtist {
        CatalogArtist {
            artist_id: id,
            name: format!("Artist {id}"),
            bio: None,
            photo_path: None,
            album_count: 1,
            track_count: 5,
        }
    }

    fn sample_album(id: i64) -> CatalogAlbum {
        CatalogAlbum {
            album_id: id,
            artist_id: id,
            title: format!("Album {id}"),
            artist: format!("Artist {id}"),
            year: Some("2024".to_string()),
            artwork_path: None,
            track_count: 10,
        }
    }

    #[test]
    fn round_trip_snapshot_roundtrips_records() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path();
        let roots = vec![PathBuf::from("/music")];
        let tracks: Vec<_> = (0..3).map(sample_track).collect();
        let artists: Vec<_> = (0..2).map(sample_artist).collect();
        let albums: Vec<_> = (0..2).map(sample_album).collect();

        save(cache_dir, &roots, &tracks, &artists, &albums).unwrap();
        let snapshot = load(cache_dir, &roots).expect("snapshot loads");
        assert_eq!(snapshot.tracks.len(), 3);
        assert_eq!(snapshot.artists.len(), 2);
        assert_eq!(snapshot.albums.len(), 2);
        assert_eq!(snapshot.tracks[0].title, "Title 0");
        assert_eq!(snapshot.artists[1].name, "Artist 1");
        assert_eq!(snapshot.albums[0].title, "Album 0");
    }

    #[test]
    fn snapshot_rejects_when_roots_change() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path();
        let roots = vec![PathBuf::from("/music")];
        save(cache_dir, &roots, &[], &[], &[]).unwrap();
        assert!(load(cache_dir, &[PathBuf::from("/different")]).is_none());
    }

    #[test]
    fn snapshot_rejects_when_magic_corrupted() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path();
        let roots = vec![PathBuf::from("/music")];
        save(cache_dir, &roots, &[], &[], &[]).unwrap();

        let path = snapshot_path(cache_dir);
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] = 0;
        fs::write(&path, bytes).unwrap();

        assert!(load(cache_dir, &roots).is_none());
    }

    #[test]
    fn roots_hash_is_stable_across_order() {
        let a = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        let b = vec![PathBuf::from("/b"), PathBuf::from("/a")];
        assert_eq!(roots_hash(&a), roots_hash(&b));
        assert_ne!(roots_hash(&a), roots_hash(&[PathBuf::from("/c")]));
    }
}
