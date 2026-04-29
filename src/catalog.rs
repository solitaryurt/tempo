use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::library::{Artwork, Track};

const APP_DIR: &str = "tempo";

#[derive(Clone, Debug)]
pub struct CatalogStore {
    db_path: PathBuf,
    cache_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct CatalogTrack {
    pub track_id: i64,
    pub file_id: i64,
    pub artist_id: i64,
    pub album_id: i64,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<String>,
    pub duration: Duration,
    pub codec: String,
    pub bitrate: Option<u32>,
    pub file_size: u64,
    pub artwork_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct CatalogArtist {
    pub artist_id: i64,
    pub name: String,
    pub bio: Option<String>,
    pub photo_path: Option<PathBuf>,
    pub album_count: usize,
    pub track_count: usize,
}

#[derive(Clone, Debug)]
pub struct CatalogAlbum {
    pub album_id: i64,
    pub artist_id: i64,
    pub title: String,
    pub artist: String,
    pub year: Option<String>,
    pub artwork_path: Option<PathBuf>,
    pub track_count: usize,
}

#[derive(Clone, Debug)]
pub struct CatalogFileFingerprint {
    pub size_bytes: u64,
    pub modified_at: Option<i64>,
    pub device_id: Option<i64>,
    pub inode: Option<i64>,
}

impl CatalogFileFingerprint {
    pub fn from_path(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        let (device_id, inode) = device_inode(&metadata);
        Some(Self {
            size_bytes: metadata.len(),
            modified_at: metadata.modified().ok().and_then(system_time_to_millis),
            device_id,
            inode,
        })
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.size_bytes == other.size_bytes
            && self.modified_at == other.modified_at
            && option_matches_if_present(self.device_id, other.device_id)
            && option_matches_if_present(self.inode, other.inode)
    }
}

impl CatalogStore {
    pub fn open_default() -> Result<Self> {
        let data_dir = data_home().join(APP_DIR);
        let cache_dir = cache_home().join(APP_DIR);
        fs::create_dir_all(&data_dir).context("failed to create Tempo data directory")?;
        fs::create_dir_all(&cache_dir).context("failed to create Tempo cache directory")?;

        let store = Self {
            db_path: data_dir.join("tempo.sqlite"),
            cache_dir,
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open {}", self.db_path.display()))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(connection)
    }

    fn migrate(&self) -> Result<()> {
        let connection = self.connect()?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS library_roots (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                added_at INTEGER NOT NULL,
                last_scan_started_at INTEGER,
                last_scan_finished_at INTEGER
             );

             CREATE TABLE IF NOT EXISTS scan_runs (
                id INTEGER PRIMARY KEY,
                started_at INTEGER NOT NULL,
                finished_at INTEGER,
                status TEXT NOT NULL
             );

             CREATE TABLE IF NOT EXISTS assets (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                source TEXT NOT NULL,
                source_url TEXT,
                cache_path TEXT NOT NULL UNIQUE,
                content_hash TEXT,
                mime_type TEXT,
                status TEXT NOT NULL,
                fetched_at INTEGER,
                error TEXT
             );

             CREATE TABLE IF NOT EXISTS artists (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                normalized_name TEXT NOT NULL UNIQUE,
                sort_name TEXT,
                musicbrainz_id TEXT UNIQUE,
                audiodb_id TEXT,
                bio TEXT,
                bio_source TEXT,
                photo_asset_id INTEGER REFERENCES assets(id),
                metadata_status TEXT NOT NULL DEFAULT 'missing',
                metadata_checked_at INTEGER,
                metadata_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS albums (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                normalized_title TEXT NOT NULL,
                artist_id INTEGER NOT NULL REFERENCES artists(id),
                artist_name TEXT NOT NULL,
                year TEXT,
                musicbrainz_release_group_id TEXT UNIQUE,
                musicbrainz_release_id TEXT,
                audiodb_id TEXT,
                cover_asset_id INTEGER REFERENCES assets(id),
                metadata_status TEXT NOT NULL DEFAULT 'missing',
                metadata_checked_at INTEGER,
                metadata_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(normalized_title, artist_id)
             );

             CREATE TABLE IF NOT EXISTS files (
                id INTEGER PRIMARY KEY,
                root_id INTEGER REFERENCES library_roots(id),
                path TEXT NOT NULL UNIQUE,
                path_parent TEXT NOT NULL,
                filename TEXT NOT NULL,
                extension TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                modified_at INTEGER,
                device_id INTEGER,
                inode INTEGER,
                last_seen_scan_id INTEGER REFERENCES scan_runs(id),
                missing_since INTEGER,
                removed_at INTEGER,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS tracks (
                id INTEGER PRIMARY KEY,
                file_id INTEGER NOT NULL UNIQUE REFERENCES files(id),
                artist_id INTEGER NOT NULL REFERENCES artists(id),
                album_id INTEGER NOT NULL REFERENCES albums(id),
                title TEXT NOT NULL,
                artist_name TEXT NOT NULL,
                album_name TEXT NOT NULL,
                year TEXT,
                duration_ms INTEGER NOT NULL,
                codec TEXT NOT NULL,
                bitrate INTEGER,
                sample_rate INTEGER,
                channels INTEGER,
                file_size INTEGER NOT NULL,
                modified_at INTEGER,
                artwork_asset_id INTEGER REFERENCES assets(id),
                artwork_path TEXT,
                search_blob TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS metadata_jobs (
                id INTEGER PRIMARY KEY,
                entity_type TEXT NOT NULL,
                entity_id INTEGER NOT NULL,
                job_type TEXT NOT NULL,
                status TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL,
                last_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(entity_type, entity_id, job_type)
             );

             CREATE INDEX IF NOT EXISTS files_root_seen_idx ON files(root_id, last_seen_scan_id);
             CREATE INDEX IF NOT EXISTS files_inode_idx ON files(device_id, inode);
             CREATE INDEX IF NOT EXISTS files_status_idx ON files(status);
             CREATE INDEX IF NOT EXISTS tracks_artist_idx ON tracks(artist_id);
             CREATE INDEX IF NOT EXISTS tracks_album_idx ON tracks(album_id);
             CREATE INDEX IF NOT EXISTS albums_artist_idx ON albums(artist_id);
             CREATE INDEX IF NOT EXISTS artists_name_idx ON artists(normalized_name);",
        )?;
        Ok(())
    }

    pub fn begin_scan(&self, roots: &[PathBuf]) -> Result<i64> {
        let mut connection = self.connect()?;
        let now = now_millis();
        let transaction = connection.transaction()?;

        for root in roots {
            let path = root.display().to_string();
            transaction.execute(
                "INSERT INTO library_roots(path, added_at, last_scan_started_at)
                 VALUES(?1, ?2, ?2)
                 ON CONFLICT(path) DO UPDATE SET last_scan_started_at = excluded.last_scan_started_at",
                params![path, now],
            )?;
        }

        transaction.execute(
            "INSERT INTO scan_runs(started_at, status) VALUES(?1, 'running')",
            params![now],
        )?;
        let scan_id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(scan_id)
    }

    pub fn finish_scan(&self, scan_id: i64, roots: &[PathBuf]) -> Result<()> {
        let mut connection = self.connect()?;
        let now = now_millis();
        let transaction = connection.transaction()?;

        for root in roots {
            transaction.execute(
                "UPDATE library_roots SET last_scan_finished_at = ?1 WHERE path = ?2",
                params![now, root.display().to_string()],
            )?;
        }

        transaction.execute(
            "UPDATE files
             SET status = 'missing', missing_since = COALESCE(missing_since, ?1), updated_at = ?1
             WHERE status = 'present'
               AND (last_seen_scan_id IS NULL OR last_seen_scan_id <> ?2)",
            params![now, scan_id],
        )?;
        transaction.execute(
            "UPDATE scan_runs SET finished_at = ?1, status = 'finished' WHERE id = ?2",
            params![now, scan_id],
        )?;

        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_track(&self, track: &Track, scan_id: Option<i64>) -> Result<CatalogTrack> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let artist_id = upsert_artist(&transaction, &track.artist, now)?;
        let album_id = upsert_album(
            &transaction,
            &track.album,
            &track.artist,
            artist_id,
            track.year.as_deref(),
            now,
        )?;
        let (artwork_asset_id, artwork_path) = self.persist_artwork(&transaction, track, now)?;
        let metadata = fs::metadata(&track.path).ok();
        let (device_id, inode) = metadata.as_ref().map(device_inode).unwrap_or_default();
        let modified_at = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .or(track.modified)
            .and_then(system_time_to_millis);
        let size_bytes = metadata
            .as_ref()
            .map_or(track.file_size, |metadata| metadata.len());
        let path = track.path.display().to_string();
        let parent = track
            .path
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let filename = track
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let extension = track
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        transaction.execute(
            "INSERT INTO files(
                root_id, path, path_parent, filename, extension, size_bytes, modified_at,
                device_id, inode, last_seen_scan_id, missing_since, removed_at, status,
                created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, 'present', ?11, ?11)
             ON CONFLICT(path) DO UPDATE SET
                root_id = excluded.root_id,
                path_parent = excluded.path_parent,
                filename = excluded.filename,
                extension = excluded.extension,
                size_bytes = excluded.size_bytes,
                modified_at = excluded.modified_at,
                device_id = excluded.device_id,
                inode = excluded.inode,
                last_seen_scan_id = excluded.last_seen_scan_id,
                missing_since = NULL,
                removed_at = NULL,
                status = 'present',
                updated_at = excluded.updated_at",
            params![
                Option::<i64>::None,
                path,
                parent,
                filename,
                extension,
                size_bytes as i64,
                modified_at,
                device_id,
                inode,
                scan_id,
                now,
            ],
        )?;
        let file_id = select_id_by_text(&transaction, "files", "path", &path)?;
        let search_blob = format!(
            "{} {} {} {} {} {}",
            track.title,
            track.artist,
            track.album,
            track.year.as_deref().unwrap_or_default(),
            track.codec,
            path
        )
        .to_lowercase();
        let duration_ms = track.duration.as_millis().min(i64::MAX as u128) as i64;

        transaction.execute(
            "INSERT INTO tracks(
                file_id, artist_id, album_id, title, artist_name, album_name, year, duration_ms,
                codec, bitrate, sample_rate, channels, file_size, modified_at, artwork_asset_id,
                artwork_path, search_blob, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)
             ON CONFLICT(file_id) DO UPDATE SET
                artist_id = excluded.artist_id,
                album_id = excluded.album_id,
                title = excluded.title,
                artist_name = excluded.artist_name,
                album_name = excluded.album_name,
                year = excluded.year,
                duration_ms = excluded.duration_ms,
                codec = excluded.codec,
                bitrate = excluded.bitrate,
                sample_rate = excluded.sample_rate,
                channels = excluded.channels,
                file_size = excluded.file_size,
                modified_at = excluded.modified_at,
                artwork_asset_id = excluded.artwork_asset_id,
                artwork_path = excluded.artwork_path,
                search_blob = excluded.search_blob,
                updated_at = excluded.updated_at",
            params![
                file_id,
                artist_id,
                album_id,
                &track.title,
                &track.artist,
                &track.album,
                track.year.as_deref(),
                duration_ms,
                &track.codec,
                track.bitrate.map(|bitrate| bitrate as i64),
                track.sample_rate.map(|sample_rate| sample_rate as i64),
                track.channels.map(|channels| channels as i64),
                size_bytes as i64,
                modified_at,
                artwork_asset_id,
                artwork_path.as_ref().map(|path| path.display().to_string()),
                &search_blob,
                now,
            ],
        )?;
        let track_id = select_id_by_i64(&transaction, "tracks", "file_id", file_id)?;
        transaction.commit()?;

        Ok(CatalogTrack {
            track_id,
            file_id,
            artist_id,
            album_id,
            path: track.path.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            year: track.year.clone(),
            duration: track.duration,
            codec: track.codec.clone(),
            bitrate: track.bitrate,
            file_size: size_bytes,
            artwork_path,
        })
    }

    pub fn mark_file_removed(&self, path: &Path) -> Result<()> {
        let connection = self.connect()?;
        let now = now_millis();
        connection.execute(
            "UPDATE files
             SET status = 'missing', missing_since = COALESCE(missing_since, ?1), removed_at = ?1, updated_at = ?1
             WHERE path = ?2",
            params![now, path.display().to_string()],
        )?;
        Ok(())
    }

    pub fn cached_track_if_unchanged(
        &self,
        path: &Path,
        scan_id: Option<i64>,
    ) -> Result<Option<CatalogTrack>> {
        let Some(fingerprint) = CatalogFileFingerprint::from_path(path) else {
            return Ok(None);
        };

        let connection = self.connect()?;
        let cached = self.load_track_by_path(&connection, path)?;
        let Some(cached) = cached else {
            return Ok(None);
        };

        if !self.file_fingerprint_matches(&connection, path, &fingerprint)? {
            return Ok(None);
        }

        let now = now_millis();
        connection.execute(
            "UPDATE files
             SET last_seen_scan_id = COALESCE(?1, last_seen_scan_id),
                 status = 'present',
                 missing_since = NULL,
                 removed_at = NULL,
                 updated_at = ?2
             WHERE path = ?3",
            params![scan_id, now, path.display().to_string()],
        )?;

        Ok(Some(cached))
    }

    pub fn load_tracks(&self, roots: &[PathBuf]) -> Result<Vec<CatalogTrack>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT
                tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                tracks.artist_name, tracks.album_name, tracks.year, tracks.duration_ms, tracks.codec,
                tracks.bitrate, tracks.file_size, tracks.artwork_path
             FROM tracks
             JOIN files ON files.id = tracks.file_id
             WHERE files.status = 'present'
             ORDER BY files.path",
        )?;
        let rows = statement.query_map([], |row| {
            let path: String = row.get(4)?;
            let duration_ms: i64 = row.get(9)?;
            let bitrate: Option<i64> = row.get(11)?;
            let file_size: i64 = row.get(12)?;
            let artwork_path: Option<String> = row.get(13)?;
            Ok(CatalogTrack {
                track_id: row.get(0)?,
                file_id: row.get(1)?,
                artist_id: row.get(2)?,
                album_id: row.get(3)?,
                path: PathBuf::from(path),
                title: row.get(5)?,
                artist: row.get(6)?,
                album: row.get(7)?,
                year: row.get(8)?,
                duration: Duration::from_millis(duration_ms.max(0) as u64),
                codec: row.get(10)?,
                bitrate: bitrate.map(|bitrate| bitrate as u32),
                file_size: file_size.max(0) as u64,
                artwork_path: artwork_path.map(PathBuf::from),
            })
        })?;

        let tracks = rows
            .filter_map(|row| row.ok())
            .filter(|track| path_in_roots(&track.path, roots))
            .collect();
        Ok(tracks)
    }

    pub fn load_track_fingerprints(
        &self,
        roots: &[PathBuf],
    ) -> Result<HashMap<PathBuf, (CatalogFileFingerprint, CatalogTrack)>> {
        if roots.is_empty() {
            return Ok(HashMap::new());
        }

        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT
                tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                tracks.artist_name, tracks.album_name, tracks.year, tracks.duration_ms, tracks.codec,
                tracks.bitrate, tracks.file_size, tracks.artwork_path,
                files.size_bytes, files.modified_at, files.device_id, files.inode
             FROM tracks
             JOIN files ON files.id = tracks.file_id
             WHERE files.status = 'present'",
        )?;
        let rows = statement.query_map([], |row| {
            let path: String = row.get(4)?;
            let duration_ms: i64 = row.get(9)?;
            let bitrate: Option<i64> = row.get(11)?;
            let file_size: i64 = row.get(12)?;
            let artwork_path: Option<String> = row.get(13)?;
            let path = PathBuf::from(path);
            Ok((
                path.clone(),
                CatalogFileFingerprint {
                    size_bytes: row.get::<_, i64>(14)?.max(0) as u64,
                    modified_at: row.get(15)?,
                    device_id: row.get(16)?,
                    inode: row.get(17)?,
                },
                CatalogTrack {
                    track_id: row.get(0)?,
                    file_id: row.get(1)?,
                    artist_id: row.get(2)?,
                    album_id: row.get(3)?,
                    path,
                    title: row.get(5)?,
                    artist: row.get(6)?,
                    album: row.get(7)?,
                    year: row.get(8)?,
                    duration: Duration::from_millis(duration_ms.max(0) as u64),
                    codec: row.get(10)?,
                    bitrate: bitrate.map(|bitrate| bitrate as u32),
                    file_size: file_size.max(0) as u64,
                    artwork_path: artwork_path.map(PathBuf::from),
                },
            ))
        })?;

        let mut tracks = HashMap::new();
        for row in rows.filter_map(|row| row.ok()) {
            let (path, fingerprint, track) = row;
            if path_in_roots(&path, roots) {
                tracks.insert(path, (fingerprint, track));
            }
        }
        Ok(tracks)
    }

    pub fn mark_paths_seen(&self, scan_id: i64, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        {
            let mut statement = transaction.prepare(
                "UPDATE files
                 SET last_seen_scan_id = ?1,
                     status = 'present',
                     missing_since = NULL,
                     removed_at = NULL,
                     updated_at = ?2
                 WHERE path = ?3",
            )?;

            for path in paths {
                statement.execute(params![scan_id, now, path.display().to_string()])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn load_track_by_path(
        &self,
        connection: &Connection,
        path: &Path,
    ) -> Result<Option<CatalogTrack>> {
        connection
            .query_row(
                "SELECT
                    tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                    tracks.artist_name, tracks.album_name, tracks.year, tracks.duration_ms, tracks.codec,
                    tracks.bitrate, tracks.file_size, tracks.artwork_path
                 FROM tracks
                 JOIN files ON files.id = tracks.file_id
                 WHERE files.path = ?1 AND files.status = 'present'",
                params![path.display().to_string()],
                |row| {
                    let path: String = row.get(4)?;
                    let duration_ms: i64 = row.get(9)?;
                    let bitrate: Option<i64> = row.get(11)?;
                    let file_size: i64 = row.get(12)?;
                    let artwork_path: Option<String> = row.get(13)?;
                    Ok(CatalogTrack {
                        track_id: row.get(0)?,
                        file_id: row.get(1)?,
                        artist_id: row.get(2)?,
                        album_id: row.get(3)?,
                        path: PathBuf::from(path),
                        title: row.get(5)?,
                        artist: row.get(6)?,
                        album: row.get(7)?,
                        year: row.get(8)?,
                        duration: Duration::from_millis(duration_ms.max(0) as u64),
                        codec: row.get(10)?,
                        bitrate: bitrate.map(|bitrate| bitrate as u32),
                        file_size: file_size.max(0) as u64,
                        artwork_path: artwork_path.map(PathBuf::from),
                    })
                },
            )
            .optional()
            .context("failed to load cached track")
    }

    fn file_fingerprint_matches(
        &self,
        connection: &Connection,
        path: &Path,
        fingerprint: &CatalogFileFingerprint,
    ) -> Result<bool> {
        let stored = connection
            .query_row(
                "SELECT size_bytes, modified_at, device_id, inode FROM files WHERE path = ?1",
                params![path.display().to_string()],
                |row| {
                    Ok(CatalogFileFingerprint {
                        size_bytes: row.get::<_, i64>(0)?.max(0) as u64,
                        modified_at: row.get(1)?,
                        device_id: row.get(2)?,
                        inode: row.get(3)?,
                    })
                },
            )
            .optional()?;

        let Some(stored) = stored else {
            return Ok(false);
        };

        Ok(stored.size_bytes == fingerprint.size_bytes
            && stored.modified_at == fingerprint.modified_at
            && option_matches_if_present(stored.device_id, fingerprint.device_id)
            && option_matches_if_present(stored.inode, fingerprint.inode))
    }

    pub fn load_artists(&self, roots: &[PathBuf]) -> Result<Vec<CatalogArtist>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connect()?;
        struct ArtistAggregate {
            artist: CatalogArtist,
            album_ids: HashSet<i64>,
        }

        let mut statement = connection.prepare(
            "SELECT
                artists.id,
                artists.name,
                artists.bio,
                photo_assets.cache_path,
                tracks.album_id,
                files.path
             FROM artists
             JOIN tracks ON tracks.artist_id = artists.id
             JOIN files ON files.id = tracks.file_id
             LEFT JOIN assets AS photo_assets ON photo_assets.id = artists.photo_asset_id
             WHERE files.status = 'present'",
        )?;
        let rows = statement.query_map([], |row| {
            let photo_path: Option<String> = row.get(3)?;
            Ok((
                CatalogArtist {
                    artist_id: row.get(0)?,
                    name: row.get(1)?,
                    bio: row.get(2)?,
                    photo_path: photo_path.map(PathBuf::from),
                    album_count: 0,
                    track_count: 0,
                },
                row.get::<_, i64>(4)?,
                PathBuf::from(row.get::<_, String>(5)?),
            ))
        })?;

        let mut artists = HashMap::<i64, ArtistAggregate>::new();
        for row in rows.filter_map(|row| row.ok()) {
            let (artist, album_id, path) = row;
            if !path_in_roots(&path, roots) {
                continue;
            }

            let entry = artists
                .entry(artist.artist_id)
                .or_insert_with(|| ArtistAggregate {
                    artist,
                    album_ids: HashSet::new(),
                });
            entry.artist.track_count += 1;
            entry.album_ids.insert(album_id);
        }

        let mut artists = artists
            .into_values()
            .map(|mut aggregate| {
                aggregate.artist.album_count = aggregate.album_ids.len();
                aggregate.artist
            })
            .collect::<Vec<_>>();
        artists.sort_by_key(|artist| artist.name.to_ascii_lowercase());
        Ok(artists)
    }

    pub fn load_albums(&self, roots: &[PathBuf]) -> Result<Vec<CatalogAlbum>> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT
                albums.id,
                albums.artist_id,
                albums.title,
                albums.artist_name,
                albums.year,
                COALESCE(cover_assets.cache_path, tracks.artwork_path),
                files.path
             FROM albums
             JOIN tracks ON tracks.album_id = albums.id
             JOIN files ON files.id = tracks.file_id
             LEFT JOIN assets AS cover_assets ON cover_assets.id = albums.cover_asset_id
             WHERE files.status = 'present'",
        )?;
        let rows = statement.query_map([], |row| {
            let artwork_path: Option<String> = row.get(5)?;
            Ok((
                CatalogAlbum {
                    album_id: row.get(0)?,
                    artist_id: row.get(1)?,
                    title: row.get(2)?,
                    artist: row.get(3)?,
                    year: row.get(4)?,
                    artwork_path: artwork_path.map(PathBuf::from),
                    track_count: 0,
                },
                PathBuf::from(row.get::<_, String>(6)?),
            ))
        })?;

        let mut albums = HashMap::<i64, CatalogAlbum>::new();
        for row in rows.filter_map(|row| row.ok()) {
            let (album, path) = row;
            if !path_in_roots(&path, roots) {
                continue;
            }

            let entry = albums
                .entry(album.album_id)
                .or_insert_with(|| album.clone());
            if entry.artwork_path.is_none() {
                entry.artwork_path = album.artwork_path;
            }
            entry.track_count += 1;
        }

        let mut albums = albums.into_values().collect::<Vec<_>>();
        albums.sort_by_key(|album| {
            (
                album.artist.to_ascii_lowercase(),
                album.year.clone().unwrap_or_default(),
                album.title.to_ascii_lowercase(),
            )
        });
        Ok(albums)
    }

    fn persist_artwork(
        &self,
        connection: &Connection,
        track: &Track,
        now: i64,
    ) -> Result<(Option<i64>, Option<PathBuf>)> {
        let Some(artwork) = &track.artwork else {
            return Ok((None, None));
        };

        match artwork {
            Artwork::File(path) => Ok((None, Some(path.clone()))),
            Artwork::Embedded { mime_type, data } if data.is_empty() => Ok((None, None)),
            Artwork::Embedded { mime_type, data } => {
                let hash = fnv1a_hex(data);
                let extension = artwork_extension(mime_type.as_deref(), data);
                let artwork_dir = self.cache_dir.join("artwork");
                fs::create_dir_all(&artwork_dir)?;
                let cache_path = artwork_dir.join(format!("{hash}.{extension}"));
                if !cache_path.exists() {
                    fs::write(&cache_path, data)?;
                }

                let cache_path_label = cache_path.display().to_string();
                connection.execute(
                    "INSERT INTO assets(kind, source, cache_path, content_hash, mime_type, status, fetched_at)
                     VALUES('album_art', 'embedded', ?1, ?2, ?3, 'ready', ?4)
                     ON CONFLICT(cache_path) DO UPDATE SET
                        content_hash = excluded.content_hash,
                        mime_type = excluded.mime_type,
                        status = 'ready',
                        fetched_at = excluded.fetched_at",
                    params![cache_path_label, hash, mime_type.as_deref(), now],
                )?;
                let asset_id =
                    select_id_by_text(connection, "assets", "cache_path", &cache_path_label)?;
                Ok((Some(asset_id), Some(cache_path)))
            }
        }
    }
}

fn upsert_artist(connection: &Connection, name: &str, now: i64) -> Result<i64> {
    let normalized = normalize_key(name);
    connection.execute(
        "INSERT INTO artists(name, normalized_name, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?3)
         ON CONFLICT(normalized_name) DO UPDATE SET
            name = excluded.name,
            updated_at = excluded.updated_at",
        params![name, normalized, now],
    )?;
    select_id_by_text(connection, "artists", "normalized_name", &normalized)
}

fn upsert_album(
    connection: &Connection,
    title: &str,
    artist_name: &str,
    artist_id: i64,
    year: Option<&str>,
    now: i64,
) -> Result<i64> {
    let normalized = normalize_key(title);
    connection.execute(
        "INSERT INTO albums(title, normalized_title, artist_id, artist_name, year, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(normalized_title, artist_id) DO UPDATE SET
            title = excluded.title,
            artist_name = excluded.artist_name,
            year = COALESCE(excluded.year, albums.year),
            updated_at = excluded.updated_at",
        params![title, normalized, artist_id, artist_name, year, now],
    )?;

    connection
        .query_row(
            "SELECT id FROM albums WHERE normalized_title = ?1 AND artist_id = ?2",
            params![normalized, artist_id],
            |row| row.get(0),
        )
        .context("failed to select album id")
}

fn select_id_by_text(
    connection: &Connection,
    table: &str,
    column: &str,
    value: &str,
) -> Result<i64> {
    let sql = format!("SELECT id FROM {table} WHERE {column} = ?1");
    connection
        .query_row(&sql, params![value], |row| row.get(0))
        .with_context(|| format!("failed to select id from {table}"))
}

fn select_id_by_i64(connection: &Connection, table: &str, column: &str, value: i64) -> Result<i64> {
    let sql = format!("SELECT id FROM {table} WHERE {column} = ?1");
    connection
        .query_row(&sql, params![value], |row| row.get(0))
        .with_context(|| format!("failed to select id from {table}"))
}

fn normalize_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn now_millis() -> i64 {
    system_time_to_millis(SystemTime::now()).unwrap_or_default()
}

fn system_time_to_millis(time: SystemTime) -> Option<i64> {
    let millis = time.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(millis.min(i64::MAX as u128) as i64)
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn path_in_roots(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn option_matches_if_present(left: Option<i64>, right: Option<i64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn artwork_extension(mime_type: Option<&str>, data: &[u8]) -> &'static str {
    match mime_type.unwrap_or_default().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" | "image/tif" => "tiff",
        _ if data.starts_with(b"\x89PNG\r\n\x1a\n") => "png",
        _ if data.starts_with(&[0xff, 0xd8, 0xff]) => "jpg",
        _ if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") => "webp",
        _ if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") => "gif",
        _ if data.starts_with(b"BM") => "bmp",
        _ => "img",
    }
}

fn fnv1a_hex(data: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(unix)]
fn device_inode(metadata: &fs::Metadata) -> (Option<i64>, Option<i64>) {
    use std::os::unix::fs::MetadataExt;

    (Some(metadata.dev() as i64), Some(metadata.ino() as i64))
}

#[cfg(not(unix))]
fn device_inode(_metadata: &fs::Metadata) -> (Option<i64>, Option<i64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_keys_for_matching() {
        assert_eq!(normalize_key("  The   Cure "), "the cure");
    }

    #[test]
    fn detects_artwork_extension_from_magic_bytes() {
        assert_eq!(artwork_extension(None, b"\x89PNG\r\n\x1a\nrest"), "png");
        assert_eq!(artwork_extension(Some("image/jpeg"), b""), "jpg");
    }
}
