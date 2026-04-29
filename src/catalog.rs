use std::{
    collections::HashMap,
    fs,
    mem::{size_of, size_of_val},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::{
    library::{Artwork, Track},
    perf,
};

const APP_DIR: &str = "tempo";
/// Bump this whenever the SQL schema or any migration step in `migrate()`
/// changes. Startup short-circuits when the SQLite `user_version` already
/// matches, so this is the only knob that controls whether DDL re-runs on
/// app launch.
const SCHEMA_VERSION: i64 = 1;

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
    pub track_number: Option<u32>,
    pub year: Option<String>,
    pub date_added: SystemTime,
    pub duration: Duration,
    pub codec: String,
    pub bitrate: Option<u32>,
    pub file_size: u64,
    pub play_count: u32,
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
pub struct CatalogDiscographyItem {
    pub item_id: i64,
    pub artist_id: i64,
    pub title: String,
    pub year: Option<String>,
    pub release_type: String,
    pub musicbrainz_release_group_id: Option<String>,
    pub cover_path: Option<PathBuf>,
    pub local_album_id: Option<i64>,
    pub is_local: bool,
}

#[derive(Clone, Debug)]
pub struct CatalogMetadataJob {
    pub job_id: i64,
    pub entity_type: String,
    pub entity_id: i64,
    pub job_type: String,
    pub attempts: u32,
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
        let _span = perf::span("catalog.open_default", "");
        let data_dir = data_home().join(APP_DIR);
        let cache_dir = cache_home().join(APP_DIR);
        fs::create_dir_all(&data_dir).context("failed to create Tempo data directory")?;
        fs::create_dir_all(&cache_dir).context("failed to create Tempo cache directory")?;

        let store = Self {
            db_path: data_dir.join("tempo.sqlite"),
            cache_dir,
        };
        perf::time_result("catalog.migrate", "", || store.migrate())?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    fn connect(&self) -> Result<Connection> {
        let _span = perf::slow_span(
            "catalog.connect",
            Duration::from_millis(8),
            format!("db={}", self.db_path.display()),
        );
        let connection = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open {}", self.db_path.display()))?;
        // PRAGMA tuning rationale:
        //   journal_mode=WAL: required for concurrent readers + writer.
        //   synchronous=NORMAL: WAL-safe, faster than FULL.
        //   foreign_keys=ON: enforce schema integrity.
        //   busy_timeout: avoid SQLITE_BUSY under contention.
        //   temp_store=MEMORY: keep temp tables/indexes in RAM.
        //   cache_size=-32768: ~32 MiB page cache (negative = KiB).
        //   mmap_size=268435456: 256 MiB memory-mapped reads (faster cold reads).
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -32768;
             PRAGMA mmap_size = 268435456;",
        )?;
        Ok(connection)
    }

    fn migrate(&self) -> Result<()> {
        let _span = perf::span("catalog.migrate_inner", "");
        let connection = self.connect()?;
        // Fast path: each migration bump increments SCHEMA_VERSION; if the
        // database already advertises that version, skip all DDL/ALTERs.
        // Even on no-op runs the `CREATE TABLE IF NOT EXISTS` + multiple
        // `ALTER TABLE` calls cost ~7 ms because SQLite still parses every
        // statement.
        let stored_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if stored_version >= SCHEMA_VERSION {
            perf::event(
                "catalog.migrate.skip",
                format!("user_version={stored_version}"),
            );
            return Ok(());
        }
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
                track_number INTEGER,
                year TEXT,
                date_added INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                codec TEXT NOT NULL,
                bitrate INTEGER,
                sample_rate INTEGER,
                channels INTEGER,
                file_size INTEGER NOT NULL,
                modified_at INTEGER,
                artwork_asset_id INTEGER REFERENCES assets(id),
                artwork_path TEXT,
                play_count INTEGER NOT NULL DEFAULT 0,
                first_played_at INTEGER,
                last_played_at INTEGER,
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

              CREATE TABLE IF NOT EXISTS waveform_cache (
                 id INTEGER PRIMARY KEY,
                 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 segments INTEGER NOT NULL,
                 version INTEGER NOT NULL,
                 size_bytes INTEGER NOT NULL,
                 modified_at INTEGER,
                 device_id INTEGER,
                 inode INTEGER,
                 peaks BLOB NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 UNIQUE(file_id, segments, version)
              );

              CREATE TABLE IF NOT EXISTS discography_items (
                 id INTEGER PRIMARY KEY,
                 artist_id INTEGER NOT NULL REFERENCES artists(id),
                title TEXT NOT NULL,
                normalized_title TEXT NOT NULL,
                year TEXT,
                release_type TEXT NOT NULL,
                musicbrainz_release_group_id TEXT UNIQUE,
                cover_asset_id INTEGER REFERENCES assets(id),
                local_album_id INTEGER REFERENCES albums(id),
                is_local INTEGER NOT NULL DEFAULT 0,
                sort_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(artist_id, normalized_title, release_type)
              );

              CREATE INDEX IF NOT EXISTS files_root_seen_idx ON files(root_id, last_seen_scan_id);
              CREATE INDEX IF NOT EXISTS files_inode_idx ON files(device_id, inode);
              CREATE INDEX IF NOT EXISTS files_status_idx ON files(status);
              CREATE INDEX IF NOT EXISTS tracks_artist_idx ON tracks(artist_id);
              CREATE INDEX IF NOT EXISTS tracks_album_idx ON tracks(album_id);
              CREATE INDEX IF NOT EXISTS albums_artist_idx ON albums(artist_id);
              CREATE INDEX IF NOT EXISTS artists_name_idx ON artists(normalized_name);
              CREATE INDEX IF NOT EXISTS metadata_jobs_pending_idx ON metadata_jobs(status, next_attempt_at);
              CREATE INDEX IF NOT EXISTS waveform_cache_file_idx ON waveform_cache(file_id);
              CREATE INDEX IF NOT EXISTS discography_artist_idx ON discography_items(artist_id, sort_key);",
        )?;
        add_column_if_missing(&connection, "tracks", "track_number", "INTEGER")?;
        add_column_if_missing(&connection, "tracks", "date_added", "INTEGER")?;
        add_column_if_missing(
            &connection,
            "tracks",
            "play_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        add_column_if_missing(&connection, "tracks", "first_played_at", "INTEGER")?;
        add_column_if_missing(&connection, "tracks", "last_played_at", "INTEGER")?;
        connection.execute(
            "UPDATE tracks SET date_added = created_at WHERE date_added IS NULL",
            [],
        )?;
        connection.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        perf::event(
            "catalog.migrate.applied",
            format!("user_version={SCHEMA_VERSION}"),
        );
        Ok(())
    }

    pub fn begin_scan(&self, roots: &[PathBuf]) -> Result<i64> {
        let _span = perf::span("catalog.begin_scan", format!("roots={}", roots.len()));
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
        let _span = perf::span(
            "catalog.finish_scan",
            format!("scan_id={scan_id} roots={}", roots.len()),
        );
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
        let _span = perf::slow_span(
            "catalog.upsert_track",
            Duration::from_millis(25),
            format!("path={}", track.path.display()),
        );
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let primary_artist = primary_artist_name(&track.artist);
        let artist_id = upsert_artist(&transaction, &primary_artist, now)?;
        let album_id = upsert_album(
            &transaction,
            &track.album,
            &primary_artist,
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
                file_id, artist_id, album_id, title, artist_name, album_name, track_number, year,
                date_added, duration_ms,
                codec, bitrate, sample_rate, channels, file_size, modified_at, artwork_asset_id,
                artwork_path, search_blob, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?20)
             ON CONFLICT(file_id) DO UPDATE SET
                artist_id = excluded.artist_id,
                album_id = excluded.album_id,
                title = excluded.title,
                artist_name = excluded.artist_name,
                album_name = excluded.album_name,
                track_number = excluded.track_number,
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
                track.track_number.map(|track_number| track_number as i64),
                track.year.as_deref(),
                now,
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
        let (date_added_ms, play_count): (i64, i64) = transaction.query_row(
            "SELECT date_added, play_count FROM tracks WHERE id = ?1",
            params![track_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
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
            track_number: track.track_number,
            year: track.year.clone(),
            date_added: millis_to_system_time(date_added_ms),
            duration: track.duration,
            codec: track.codec.clone(),
            bitrate: track.bitrate,
            file_size: size_bytes,
            play_count: play_count.max(0) as u32,
            artwork_path,
        })
    }

    pub fn mark_file_removed(&self, path: &Path) -> Result<()> {
        let _span = perf::span(
            "catalog.mark_file_removed",
            format!("path={}", path.display()),
        );
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

    pub fn load_waveform(
        &self,
        path: &Path,
        segments: usize,
        version: u32,
    ) -> Result<Option<Vec<f32>>> {
        let _span = perf::slow_span(
            "catalog.load_waveform",
            Duration::from_millis(8),
            format!(
                "path={} segments={segments} version={version}",
                path.display()
            ),
        );
        let Some(current_fingerprint) = CatalogFileFingerprint::from_path(path) else {
            return Ok(None);
        };

        let connection = self.connect()?;
        let row = connection
            .query_row(
                "SELECT
                    waveform_cache.size_bytes, waveform_cache.modified_at,
                    waveform_cache.device_id, waveform_cache.inode, waveform_cache.peaks
                 FROM waveform_cache
                 JOIN files ON files.id = waveform_cache.file_id
                 WHERE files.path = ?1
                   AND files.status = 'present'
                   AND waveform_cache.segments = ?2
                   AND waveform_cache.version = ?3",
                params![
                    path.display().to_string(),
                    segments.min(i64::MAX as usize) as i64,
                    i64::from(version),
                ],
                |row| {
                    Ok((
                        CatalogFileFingerprint {
                            size_bytes: row.get::<_, i64>(0)?.max(0) as u64,
                            modified_at: row.get(1)?,
                            device_id: row.get(2)?,
                            inode: row.get(3)?,
                        },
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .optional()?;

        let Some((cached_fingerprint, peaks)) = row else {
            return Ok(None);
        };

        if !cached_fingerprint.matches(&current_fingerprint) {
            return Ok(None);
        }

        Ok(waveform_from_blob(&peaks, segments))
    }

    pub fn save_waveform(
        &self,
        path: &Path,
        segments: usize,
        version: u32,
        peaks: &[f32],
    ) -> Result<()> {
        let _span = perf::slow_span(
            "catalog.save_waveform",
            Duration::from_millis(8),
            format!(
                "path={} segments={segments} version={version}",
                path.display()
            ),
        );
        if peaks.len() != segments {
            return Ok(());
        }

        let Some(fingerprint) = CatalogFileFingerprint::from_path(path) else {
            return Ok(());
        };

        let connection = self.connect()?;
        let file_id = connection
            .query_row(
                "SELECT id FROM files WHERE path = ?1 AND status = 'present'",
                params![path.display().to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let Some(file_id) = file_id else {
            return Ok(());
        };

        let now = now_millis();
        connection.execute(
            "INSERT INTO waveform_cache(
                file_id, segments, version, size_bytes, modified_at, device_id, inode,
                peaks, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(file_id, segments, version) DO UPDATE SET
                size_bytes = excluded.size_bytes,
                modified_at = excluded.modified_at,
                device_id = excluded.device_id,
                inode = excluded.inode,
                peaks = excluded.peaks,
                updated_at = excluded.updated_at",
            params![
                file_id,
                segments.min(i64::MAX as usize) as i64,
                i64::from(version),
                fingerprint.size_bytes.min(i64::MAX as u64) as i64,
                fingerprint.modified_at,
                fingerprint.device_id,
                fingerprint.inode,
                waveform_to_blob(peaks),
                now,
            ],
        )?;

        Ok(())
    }

    pub fn cached_track_if_unchanged(
        &self,
        path: &Path,
        scan_id: Option<i64>,
    ) -> Result<Option<CatalogTrack>> {
        let _span = perf::slow_span(
            "catalog.cached_track_if_unchanged",
            Duration::from_millis(8),
            format!("path={}", path.display()),
        );
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
        let _span = perf::span("catalog.load_tracks", format!("roots={}", roots.len()));
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let connection = self.connect()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare(&format!(
            "SELECT
                tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                tracks.artist_name, tracks.album_name, tracks.track_number, tracks.year,
                tracks.date_added, tracks.duration_ms, tracks.codec, tracks.bitrate,
                tracks.file_size, tracks.play_count, tracks.artwork_path
             FROM tracks
             JOIN files ON files.id = tracks.file_id
             WHERE files.status = 'present' AND ({root_filter})
             ORDER BY files.path",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let path: String = row.get(4)?;
            let track_number: Option<i64> = row.get(8)?;
            let date_added_ms: i64 = row.get(10)?;
            let duration_ms: i64 = row.get(11)?;
            let bitrate: Option<i64> = row.get(13)?;
            let file_size: i64 = row.get(14)?;
            let play_count: i64 = row.get(15)?;
            let artwork_path: Option<String> = row.get(16)?;
            Ok(CatalogTrack {
                track_id: row.get(0)?,
                file_id: row.get(1)?,
                artist_id: row.get(2)?,
                album_id: row.get(3)?,
                path: PathBuf::from(path),
                title: row.get(5)?,
                artist: row.get(6)?,
                album: row.get(7)?,
                track_number: track_number.map(|track_number| track_number as u32),
                year: row.get(9)?,
                date_added: millis_to_system_time(date_added_ms),
                duration: Duration::from_millis(duration_ms.max(0) as u64),
                codec: row.get(12)?,
                bitrate: bitrate.map(|bitrate| bitrate as u32),
                file_size: file_size.max(0) as u64,
                play_count: play_count.max(0) as u32,
                artwork_path: artwork_path.map(PathBuf::from),
            })
        })?;

        let tracks: Vec<CatalogTrack> = rows.filter_map(|row| row.ok()).collect();
        perf::event(
            "catalog.load_tracks.count",
            format!("tracks={}", tracks.len()),
        );
        Ok(tracks)
    }

    pub fn load_track_fingerprints(
        &self,
        roots: &[PathBuf],
    ) -> Result<HashMap<PathBuf, (CatalogFileFingerprint, CatalogTrack)>> {
        let _span = perf::span(
            "catalog.load_track_fingerprints",
            format!("roots={}", roots.len()),
        );
        if roots.is_empty() {
            return Ok(HashMap::new());
        }

        let connection = self.connect()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare(&format!(
            "SELECT
                tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                tracks.artist_name, tracks.album_name, tracks.track_number, tracks.year,
                tracks.date_added, tracks.duration_ms, tracks.codec, tracks.bitrate,
                tracks.file_size, tracks.play_count, tracks.artwork_path,
                files.size_bytes, files.modified_at, files.device_id, files.inode
             FROM tracks
             JOIN files ON files.id = tracks.file_id
             WHERE files.status = 'present' AND ({root_filter})",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let path: String = row.get(4)?;
            let track_number: Option<i64> = row.get(8)?;
            let date_added_ms: i64 = row.get(10)?;
            let duration_ms: i64 = row.get(11)?;
            let bitrate: Option<i64> = row.get(13)?;
            let file_size: i64 = row.get(14)?;
            let play_count: i64 = row.get(15)?;
            let artwork_path: Option<String> = row.get(16)?;
            let path = PathBuf::from(path);
            Ok((
                path.clone(),
                CatalogFileFingerprint {
                    size_bytes: row.get::<_, i64>(17)?.max(0) as u64,
                    modified_at: row.get(18)?,
                    device_id: row.get(19)?,
                    inode: row.get(20)?,
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
                    track_number: track_number.map(|track_number| track_number as u32),
                    year: row.get(9)?,
                    date_added: millis_to_system_time(date_added_ms),
                    duration: Duration::from_millis(duration_ms.max(0) as u64),
                    codec: row.get(12)?,
                    bitrate: bitrate.map(|bitrate| bitrate as u32),
                    file_size: file_size.max(0) as u64,
                    play_count: play_count.max(0) as u32,
                    artwork_path: artwork_path.map(PathBuf::from),
                },
            ))
        })?;

        let mut tracks = HashMap::new();
        for row in rows.filter_map(|row| row.ok()) {
            let (path, fingerprint, track) = row;
            tracks.insert(path, (fingerprint, track));
        }
        perf::event(
            "catalog.load_track_fingerprints.count",
            format!("tracks={}", tracks.len()),
        );
        Ok(tracks)
    }

    pub fn increment_play_count(&self, path: &Path) -> Result<u32> {
        let _span = perf::span(
            "catalog.increment_play_count",
            format!("path={}", path.display()),
        );
        let connection = self.connect()?;
        let now = now_millis();
        connection.execute(
            "UPDATE tracks
             SET play_count = play_count + 1,
                 first_played_at = COALESCE(first_played_at, ?1),
                 last_played_at = ?1,
                 updated_at = ?1
             WHERE file_id = (SELECT id FROM files WHERE path = ?2)",
            params![now, path.display().to_string()],
        )?;

        let play_count = connection
            .query_row(
                "SELECT tracks.play_count
                 FROM tracks
                 JOIN files ON files.id = tracks.file_id
                 WHERE files.path = ?1",
                params![path.display().to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or_default();
        Ok(play_count.max(0) as u32)
    }

    pub fn mark_paths_seen(&self, scan_id: i64, paths: &[PathBuf]) -> Result<()> {
        let _span = perf::span(
            "catalog.mark_paths_seen",
            format!("scan_id={scan_id} paths={}", paths.len()),
        );
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

    pub fn enqueue_metadata_job(
        &self,
        entity_type: &str,
        entity_id: i64,
        job_type: &str,
    ) -> Result<()> {
        let connection = self.connect()?;
        enqueue_metadata_job(&connection, entity_type, entity_id, job_type, now_millis())
    }

    pub fn claim_next_metadata_job(
        &self,
        supported_job_types: &[&str],
    ) -> Result<Option<CatalogMetadataJob>> {
        if supported_job_types.is_empty() {
            return Ok(None);
        }

        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let job = {
            let mut statement = transaction.prepare(
                "SELECT id, entity_type, entity_id, job_type, attempts
                 FROM metadata_jobs
                 WHERE status = 'pending' AND next_attempt_at <= ?1
                 ORDER BY next_attempt_at, created_at
                 LIMIT 50",
            )?;
            let rows = statement.query_map(params![now], |row| {
                Ok(CatalogMetadataJob {
                    job_id: row.get(0)?,
                    entity_type: row.get(1)?,
                    entity_id: row.get(2)?,
                    job_type: row.get(3)?,
                    attempts: row.get::<_, i64>(4)?.max(0) as u32,
                })
            })?;

            rows.filter_map(|row| row.ok())
                .find(|job| supported_job_types.contains(&job.job_type.as_str()))
        };

        if let Some(job) = &job {
            transaction.execute(
                "UPDATE metadata_jobs
                 SET status = 'running', attempts = attempts + 1, updated_at = ?1
                 WHERE id = ?2 AND status = 'pending'",
                params![now, job.job_id],
            )?;
        }

        transaction.commit()?;
        Ok(job)
    }

    pub fn complete_metadata_job(&self, job_id: i64) -> Result<()> {
        let connection = self.connect()?;
        let now = now_millis();
        connection.execute(
            "UPDATE metadata_jobs
             SET status = 'complete', last_error = NULL, updated_at = ?1
             WHERE id = ?2",
            params![now, job_id],
        )?;
        Ok(())
    }

    pub fn fail_metadata_job(&self, job_id: i64, error: &str) -> Result<()> {
        let connection = self.connect()?;
        let now = now_millis();
        connection.execute(
            "UPDATE metadata_jobs
             SET status = 'pending',
                 next_attempt_at = ?1 + (60000 * (1 << MIN(attempts, 8))),
                 last_error = ?2,
                 updated_at = ?1
             WHERE id = ?3",
            params![now, error, job_id],
        )?;
        Ok(())
    }

    pub fn upsert_discography_item(
        &self,
        artist_id: i64,
        title: &str,
        year: Option<&str>,
        release_type: &str,
        musicbrainz_release_group_id: Option<&str>,
    ) -> Result<i64> {
        let connection = self.connect()?;
        let now = now_millis();
        let normalized_title = normalize_key(title);
        let sort_key = discography_sort_key(year, title);
        connection.execute(
            "INSERT INTO discography_items(
                artist_id, title, normalized_title, year, release_type,
                musicbrainz_release_group_id, sort_key, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT(artist_id, normalized_title, release_type) DO UPDATE SET
                title = excluded.title,
                year = COALESCE(excluded.year, discography_items.year),
                musicbrainz_release_group_id = COALESCE(
                    excluded.musicbrainz_release_group_id,
                    discography_items.musicbrainz_release_group_id
                ),
                sort_key = excluded.sort_key,
                updated_at = excluded.updated_at",
            params![
                artist_id,
                title,
                normalized_title,
                year,
                release_type,
                musicbrainz_release_group_id,
                sort_key,
                now,
            ],
        )?;
        connection
            .query_row(
                "SELECT id FROM discography_items
                 WHERE artist_id = ?1 AND normalized_title = ?2 AND release_type = ?3",
                params![artist_id, normalized_title, release_type],
                |row| row.get(0),
            )
            .context("failed to select discography item id")
    }

    pub fn load_discography(&self, artist_id: i64) -> Result<Vec<CatalogDiscographyItem>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT
                discography_items.id,
                discography_items.artist_id,
                discography_items.title,
                discography_items.year,
                discography_items.release_type,
                discography_items.musicbrainz_release_group_id,
                cover_assets.cache_path,
                discography_items.local_album_id,
                discography_items.is_local
             FROM discography_items
             LEFT JOIN assets AS cover_assets ON cover_assets.id = discography_items.cover_asset_id
             WHERE discography_items.artist_id = ?1
             ORDER BY discography_items.sort_key, discography_items.title",
        )?;
        let rows = statement.query_map(params![artist_id], |row| {
            let cover_path: Option<String> = row.get(6)?;
            Ok(CatalogDiscographyItem {
                item_id: row.get(0)?,
                artist_id: row.get(1)?,
                title: row.get(2)?,
                year: row.get(3)?,
                release_type: row.get(4)?,
                musicbrainz_release_group_id: row.get(5)?,
                cover_path: cover_path.map(PathBuf::from),
                local_album_id: row.get(7)?,
                is_local: row.get::<_, i64>(8)? != 0,
            })
        })?;

        Ok(rows.filter_map(|row| row.ok()).collect())
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
                    tracks.artist_name, tracks.album_name, tracks.track_number, tracks.year,
                    tracks.date_added, tracks.duration_ms, tracks.codec, tracks.bitrate,
                    tracks.file_size, tracks.play_count, tracks.artwork_path
                 FROM tracks
                 JOIN files ON files.id = tracks.file_id
                 WHERE files.path = ?1 AND files.status = 'present'",
                params![path.display().to_string()],
                |row| {
                    let path: String = row.get(4)?;
                    let track_number: Option<i64> = row.get(8)?;
                    let date_added_ms: i64 = row.get(10)?;
                    let duration_ms: i64 = row.get(11)?;
                    let bitrate: Option<i64> = row.get(13)?;
                    let file_size: i64 = row.get(14)?;
                    let play_count: i64 = row.get(15)?;
                    let artwork_path: Option<String> = row.get(16)?;
                    Ok(CatalogTrack {
                        track_id: row.get(0)?,
                        file_id: row.get(1)?,
                        artist_id: row.get(2)?,
                        album_id: row.get(3)?,
                        path: PathBuf::from(path),
                        title: row.get(5)?,
                        artist: row.get(6)?,
                        album: row.get(7)?,
                        track_number: track_number.map(|track_number| track_number as u32),
                        year: row.get(9)?,
                        date_added: millis_to_system_time(date_added_ms),
                        duration: Duration::from_millis(duration_ms.max(0) as u64),
                        codec: row.get(12)?,
                        bitrate: bitrate.map(|bitrate| bitrate as u32),
                        file_size: file_size.max(0) as u64,
                        play_count: play_count.max(0) as u32,
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
        let _span = perf::span("catalog.load_artists", format!("roots={}", roots.len()));
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        struct ArtistAggregate {
            artist_id: i64,
            name: String,
            bio: Option<String>,
            photo_path: Option<PathBuf>,
            album_keys: Vec<String>,
            track_count: usize,
        }
        struct ArtistRow {
            artist_id: i64,
            name: String,
            bio: Option<String>,
            photo_path: Option<PathBuf>,
            album_name: String,
            track_artist: String,
        }

        let connection = self.connect()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare(&format!(
            "SELECT
                artists.id,
                artists.name,
                artists.bio,
                photo_assets.cache_path,
                COALESCE(cover_assets.cache_path, tracks.artwork_path),
                tracks.album_name,
                tracks.artist_name
             FROM artists
             JOIN tracks ON tracks.artist_id = artists.id
             JOIN files ON files.id = tracks.file_id
             JOIN albums ON albums.id = tracks.album_id
             LEFT JOIN assets AS photo_assets ON photo_assets.id = artists.photo_asset_id
             LEFT JOIN assets AS cover_assets ON cover_assets.id = albums.cover_asset_id
             WHERE files.status = 'present' AND ({root_filter})
              ORDER BY lower(tracks.artist_name), tracks.album_name, tracks.title",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let photo_path: Option<String> = row.get(3)?;
            let fallback_photo_path: Option<String> = row.get(4)?;
            Ok(ArtistRow {
                artist_id: row.get(0)?,
                name: row.get(1)?,
                bio: row.get(2)?,
                photo_path: photo_path.or(fallback_photo_path).map(PathBuf::from),
                album_name: row.get::<_, String>(5)?,
                track_artist: row.get::<_, String>(6)?,
            })
        })?;

        let mut artists = HashMap::<String, ArtistAggregate>::new();
        for row in rows.filter_map(|row| row.ok()) {
            let album_key = normalize_key(&row.album_name);
            for artist_name in individual_artist_names(&row.track_artist) {
                let key = normalize_key(&artist_name);
                let aggregate = artists.entry(key).or_insert_with(|| ArtistAggregate {
                    artist_id: synthetic_artist_id(&artist_name),
                    name: artist_name,
                    bio: None,
                    photo_path: None,
                    album_keys: Vec::new(),
                    track_count: 0,
                });

                if normalize_key(&row.name) == normalize_key(&aggregate.name) {
                    aggregate.artist_id = row.artist_id;
                    aggregate.name = row.name.clone();
                }
                if aggregate.bio.is_none() {
                    aggregate.bio = row.bio.clone();
                }
                if aggregate.photo_path.is_none() {
                    aggregate.photo_path = row.photo_path.clone();
                }
                if !aggregate.album_keys.contains(&album_key) {
                    aggregate.album_keys.push(album_key.clone());
                }
                aggregate.track_count += 1;
            }
        }

        let mut artists = artists
            .into_values()
            .map(|artist| CatalogArtist {
                artist_id: artist.artist_id,
                name: artist.name,
                bio: artist.bio,
                photo_path: artist.photo_path,
                album_count: artist.album_keys.len(),
                track_count: artist.track_count,
            })
            .collect::<Vec<_>>();
        artists.sort_by_key(|artist| artist.name.to_lowercase());
        perf::event(
            "catalog.load_artists.count",
            format!("artists={}", artists.len()),
        );
        Ok(artists)
    }

    pub fn load_albums(&self, roots: &[PathBuf]) -> Result<Vec<CatalogAlbum>> {
        let _span = perf::span("catalog.load_albums", format!("roots={}", roots.len()));
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        struct AlbumAggregate {
            album_id: i64,
            artist_id: i64,
            title: String,
            artist: String,
            year: Option<String>,
            artwork_path: Option<PathBuf>,
            track_count: usize,
        }
        struct AlbumRow {
            album_id: i64,
            artist_id: i64,
            title: String,
            track_artist: String,
            year: Option<String>,
            artwork_path: Option<PathBuf>,
        }

        let connection = self.connect()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare(&format!(
            "SELECT
                albums.id,
                albums.artist_id,
                albums.title,
                tracks.artist_name,
                albums.year,
                COALESCE(cover_assets.cache_path, tracks.artwork_path)
             FROM albums
             JOIN tracks ON tracks.album_id = albums.id
             JOIN files ON files.id = tracks.file_id
             LEFT JOIN assets AS cover_assets ON cover_assets.id = albums.cover_asset_id
             WHERE files.status = 'present' AND ({root_filter})
              ORDER BY lower(tracks.artist_name), albums.year, lower(albums.title), tracks.title",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let artwork_path: Option<String> = row.get(5)?;
            Ok(AlbumRow {
                album_id: row.get(0)?,
                artist_id: row.get(1)?,
                title: row.get(2)?,
                track_artist: row.get::<_, String>(3)?,
                year: row.get(4)?,
                artwork_path: artwork_path.map(PathBuf::from),
            })
        })?;

        let mut albums = HashMap::<String, AlbumAggregate>::new();
        for row in rows.filter_map(|row| row.ok()) {
            let primary_artist = primary_artist_name(&row.track_artist);
            let key = format!(
                "{}:{}",
                normalize_key(&primary_artist),
                normalize_key(&row.title)
            );
            let aggregate = albums.entry(key).or_insert_with(|| AlbumAggregate {
                album_id: row.album_id,
                artist_id: row.artist_id,
                title: row.title,
                artist: primary_artist,
                year: row.year,
                artwork_path: None,
                track_count: 0,
            });

            if aggregate.artwork_path.is_none() {
                aggregate.artwork_path = row.artwork_path;
            }
            aggregate.track_count += 1;
        }

        let mut albums = albums
            .into_values()
            .map(|album| CatalogAlbum {
                album_id: album.album_id,
                artist_id: album.artist_id,
                title: album.title,
                artist: album.artist,
                year: album.year,
                artwork_path: album.artwork_path,
                track_count: album.track_count,
            })
            .collect::<Vec<_>>();
        albums.sort_by(|left, right| {
            left.artist
                .to_lowercase()
                .cmp(&right.artist.to_lowercase())
                .then(left.year.cmp(&right.year))
                .then(left.title.to_lowercase().cmp(&right.title.to_lowercase()))
        });
        perf::event(
            "catalog.load_albums.count",
            format!("albums={}", albums.len()),
        );
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

fn enqueue_metadata_job(
    connection: &Connection,
    entity_type: &str,
    entity_id: i64,
    job_type: &str,
    now: i64,
) -> Result<()> {
    connection.execute(
        "INSERT INTO metadata_jobs(
            entity_type, entity_id, job_type, status, next_attempt_at, created_at, updated_at
         ) VALUES(?1, ?2, ?3, 'pending', ?4, ?4, ?4)
         ON CONFLICT(entity_type, entity_id, job_type) DO UPDATE SET
            status = CASE
                WHEN metadata_jobs.status IN ('complete', 'running') THEN metadata_jobs.status
                ELSE 'pending'
            END,
            next_attempt_at = CASE
                WHEN metadata_jobs.status IN ('complete', 'running') THEN metadata_jobs.next_attempt_at
                ELSE MIN(metadata_jobs.next_attempt_at, excluded.next_attempt_at)
            END,
            updated_at = excluded.updated_at",
        params![entity_type, entity_id, job_type, now],
    )?;
    Ok(())
}

fn discography_sort_key(year: Option<&str>, title: &str) -> String {
    let year = year
        .map(|year| {
            year.chars()
                .filter(|ch| ch.is_ascii_digit())
                .take(4)
                .collect::<String>()
        })
        .filter(|year: &String| year.len() == 4)
        .unwrap_or_else(|| "9999".to_string());
    format!("{}:{}", year, normalize_key(title))
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

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&pragma)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let existing: String = row.get(1)?;
        if existing == column {
            return Ok(());
        }
    }

    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
    connection.execute(&sql, [])?;
    Ok(())
}

fn normalize_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn primary_artist_name(artist: &str) -> String {
    let artist = artist.trim();
    if artist.is_empty() {
        return String::new();
    }

    let lower = artist.to_ascii_lowercase();
    let split_at = feature_artist_marker(&lower)
        .map(|(split_at, _)| split_at)
        .unwrap_or(artist.len());
    let primary = artist[..split_at].trim();

    if primary.is_empty() {
        artist.to_string()
    } else {
        primary.to_string()
    }
}

pub fn individual_artist_names(artist: &str) -> Vec<String> {
    let artist = artist.trim();
    if artist.is_empty() {
        return Vec::new();
    }

    let lower = artist.to_ascii_lowercase();
    let Some((split_at, marker_len)) = feature_artist_marker(&lower) else {
        return vec![artist.to_string()];
    };

    let mut artists = Vec::new();
    let primary = artist[..split_at].trim();
    if !primary.is_empty() {
        artists.push(primary.to_string());
    }

    let featured = artist[split_at + marker_len..].trim_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, '.' | '/' | ':' | '-' | '(' | '[')
    });
    for name in featured.split([',', ';']) {
        for name in name.split(" & ") {
            for name in name.split(" and ") {
                let name = name.trim();
                if !name.is_empty() && !artists.iter().any(|artist| artist == name) {
                    artists.push(name.to_string());
                }
            }
        }
    }

    if artists.is_empty() {
        vec![artist.to_string()]
    } else {
        artists
    }
}

fn feature_artist_marker(lower_artist: &str) -> Option<(usize, usize)> {
    let markers = [
        " feat. ",
        " feat ",
        " feat/",
        " feat.",
        " ft. ",
        " ft ",
        " ft/",
        " ft.",
        " featuring ",
        " featuring/",
        " featuring.",
        " (feat",
        " [feat",
    ];

    markers
        .iter()
        .filter_map(|marker| lower_artist.find(marker).map(|ix| (ix, marker.len())))
        .min_by_key(|(ix, _)| *ix)
}

fn synthetic_artist_id(name: &str) -> i64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in normalize_key(name).bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }

    -((hash & 0x3fff_ffff_ffff_ffff) as i64).max(1)
}

fn now_millis() -> i64 {
    system_time_to_millis(SystemTime::now()).unwrap_or_default()
}

fn system_time_to_millis(time: SystemTime) -> Option<i64> {
    let millis = time.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(millis.min(i64::MAX as u128) as i64)
}

fn millis_to_system_time(millis: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis.max(0) as u64)
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

fn waveform_to_blob(peaks: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(size_of_val(peaks));
    for peak in peaks {
        blob.extend_from_slice(&peak.to_le_bytes());
    }
    blob
}

fn waveform_from_blob(blob: &[u8], segments: usize) -> Option<Vec<f32>> {
    if blob.len() != segments * size_of::<f32>() {
        return None;
    }

    Some(
        blob.chunks_exact(size_of::<f32>())
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn root_path_filter(roots: &[PathBuf]) -> (String, Vec<String>) {
    let mut clauses = Vec::with_capacity(roots.len());
    let mut params = Vec::with_capacity(roots.len() * 2);

    for root in roots {
        let root = root.display().to_string();
        let root = root.trim_end_matches('/');
        let root = if root.is_empty() { "/" } else { root };
        let prefix = if root == "/" {
            "/".to_string()
        } else {
            format!("{}/", escape_like(root))
        };

        clauses.push("(files.path = ? OR files.path LIKE ? ESCAPE '\\')");
        params.push(root.to_string());
        params.push(format!("{prefix}%"));
    }

    (clauses.join(" OR "), params)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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
    fn extracts_primary_artist_from_featured_credit() {
        assert_eq!(primary_artist_name("A$AP Rocky feat/ Skepta"), "A$AP Rocky");
        assert_eq!(primary_artist_name("A$AP Rocky feat. Skepta"), "A$AP Rocky");
        assert_eq!(primary_artist_name("A$AP Rocky ft Skepta"), "A$AP Rocky");
        assert_eq!(
            primary_artist_name("A$AP Rocky featuring Skepta"),
            "A$AP Rocky"
        );
        assert_eq!(primary_artist_name("The Features"), "The Features");
    }

    #[test]
    fn extracts_individual_artists_from_featured_credit() {
        assert_eq!(
            individual_artist_names("A$AP Rocky feat/ Skepta & FKA twigs"),
            vec!["A$AP Rocky", "Skepta", "FKA twigs"]
        );
        assert_eq!(
            individual_artist_names("The Features"),
            vec!["The Features"]
        );
    }

    #[test]
    fn detects_artwork_extension_from_magic_bytes() {
        assert_eq!(artwork_extension(None, b"\x89PNG\r\n\x1a\nrest"), "png");
        assert_eq!(artwork_extension(Some("image/jpeg"), b""), "jpg");
    }

    #[test]
    fn stores_discography_items_and_metadata_jobs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore {
            db_path: temp_dir.path().join("tempo.sqlite"),
            cache_dir: temp_dir.path().join("cache"),
        };
        store.migrate().unwrap();

        let connection = store.connect().unwrap();
        let artist_id = upsert_artist(&connection, "Brian Eno", now_millis()).unwrap();

        let item_id = store
            .upsert_discography_item(
                artist_id,
                "Another Green World",
                Some("1975"),
                "album",
                Some("release-group-mbid"),
            )
            .unwrap();
        let discography = store.load_discography(artist_id).unwrap();
        assert_eq!(discography.len(), 1);
        assert_eq!(discography[0].item_id, item_id);
        assert_eq!(discography[0].title, "Another Green World");

        store
            .enqueue_metadata_job("artist", artist_id, "resolve_artist_musicbrainz")
            .unwrap();
        let job = store
            .claim_next_metadata_job(&["resolve_artist_musicbrainz"])
            .unwrap()
            .unwrap();
        assert_eq!(job.entity_type, "artist");
        assert_eq!(job.entity_id, artist_id);
        store.complete_metadata_job(job.job_id).unwrap();
    }

    #[test]
    fn loads_browse_data_with_sql_aggregation_and_root_filtering() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore {
            db_path: temp_dir.path().join("tempo.sqlite"),
            cache_dir: temp_dir.path().join("cache"),
        };
        store.migrate().unwrap();

        let root = temp_dir.path().join("library");
        let other_root = temp_dir.path().join("other-library");
        let cover_path = root.join("cover.jpg");
        store
            .upsert_track(
                &Track {
                    path: root.join("one.flac"),
                    title: "One".to_string(),
                    artist: "Alice".to_string(),
                    album: "First".to_string(),
                    track_number: None,
                    year: Some("2024".to_string()),
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 10,
                    modified: None,
                    artwork: Some(Artwork::File(cover_path.clone())),
                },
                None,
            )
            .unwrap();
        store
            .upsert_track(
                &Track {
                    path: root.join("two.flac"),
                    title: "Two".to_string(),
                    artist: "Alice".to_string(),
                    album: "First".to_string(),
                    track_number: None,
                    year: Some("2024".to_string()),
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 10,
                    modified: None,
                    artwork: None,
                },
                None,
            )
            .unwrap();
        store
            .upsert_track(
                &Track {
                    path: other_root.join("three.flac"),
                    title: "Three".to_string(),
                    artist: "Bob".to_string(),
                    album: "Outside".to_string(),
                    track_number: None,
                    year: None,
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 10,
                    modified: None,
                    artwork: None,
                },
                None,
            )
            .unwrap();

        let artists = store.load_artists(std::slice::from_ref(&root)).unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Alice");
        assert_eq!(artists[0].photo_path.as_ref(), Some(&cover_path));
        assert_eq!(artists[0].album_count, 1);
        assert_eq!(artists[0].track_count, 2);

        let albums = store.load_albums(&[root]).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "First");
        assert_eq!(albums[0].artwork_path.as_ref(), Some(&cover_path));
        assert_eq!(albums[0].track_count, 2);
    }

    #[test]
    fn stores_and_loads_waveform_cache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore {
            db_path: temp_dir.path().join("tempo.sqlite"),
            cache_dir: temp_dir.path().join("cache"),
        };
        store.migrate().unwrap();

        let root = temp_dir.path().join("library");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("one.flac");
        fs::write(&path, b"audio").unwrap();
        store
            .upsert_track(
                &Track {
                    path: path.clone(),
                    title: "One".to_string(),
                    artist: "Alice".to_string(),
                    album: "First".to_string(),
                    track_number: None,
                    year: Some("2024".to_string()),
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 5,
                    modified: None,
                    artwork: None,
                },
                None,
            )
            .unwrap();

        let peaks = vec![8.0, 16.5, 58.0];
        store.save_waveform(&path, peaks.len(), 1, &peaks).unwrap();

        assert_eq!(
            store.load_waveform(&path, peaks.len(), 1).unwrap(),
            Some(peaks)
        );
        assert_eq!(store.load_waveform(&path, 4, 1).unwrap(), None);
        assert_eq!(store.load_waveform(&path, 3, 2).unwrap(), None);
    }

    #[test]
    fn invalidates_waveform_cache_when_file_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore {
            db_path: temp_dir.path().join("tempo.sqlite"),
            cache_dir: temp_dir.path().join("cache"),
        };
        store.migrate().unwrap();

        let root = temp_dir.path().join("library");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("one.flac");
        fs::write(&path, b"audio").unwrap();
        store
            .upsert_track(
                &Track {
                    path: path.clone(),
                    title: "One".to_string(),
                    artist: "Alice".to_string(),
                    album: "First".to_string(),
                    track_number: None,
                    year: Some("2024".to_string()),
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 5,
                    modified: None,
                    artwork: None,
                },
                None,
            )
            .unwrap();

        store
            .save_waveform(&path, 3, 1, &[8.0, 16.5, 58.0])
            .unwrap();
        fs::write(&path, b"changed audio").unwrap();

        assert_eq!(store.load_waveform(&path, 3, 1).unwrap(), None);
    }

    #[test]
    fn groups_featured_track_credits_under_primary_artist() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore {
            db_path: temp_dir.path().join("tempo.sqlite"),
            cache_dir: temp_dir.path().join("cache"),
        };
        store.migrate().unwrap();

        let root = temp_dir.path().join("library");
        store
            .upsert_track(
                &Track {
                    path: root.join("solo.flac"),
                    title: "Solo".to_string(),
                    artist: "A$AP Rocky".to_string(),
                    album: "Testing".to_string(),
                    track_number: None,
                    year: Some("2018".to_string()),
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 10,
                    modified: None,
                    artwork: None,
                },
                None,
            )
            .unwrap();
        store
            .upsert_track(
                &Track {
                    path: root.join("featured.flac"),
                    title: "Featured".to_string(),
                    artist: "A$AP Rocky feat/ Skepta".to_string(),
                    album: "Testing".to_string(),
                    track_number: None,
                    year: Some("2018".to_string()),
                    date_added: UNIX_EPOCH,
                    duration: Duration::from_secs(60),
                    codec: "FLAC".to_string(),
                    sample_rate: None,
                    channels: None,
                    bitrate: None,
                    file_size: 10,
                    modified: None,
                    artwork: None,
                },
                None,
            )
            .unwrap();

        let artists = store.load_artists(std::slice::from_ref(&root)).unwrap();
        assert_eq!(artists.len(), 2);
        let rocky = artists
            .iter()
            .find(|artist| artist.name == "A$AP Rocky")
            .unwrap();
        assert_eq!(rocky.album_count, 1);
        assert_eq!(rocky.track_count, 2);
        let skepta = artists
            .iter()
            .find(|artist| artist.name == "Skepta")
            .unwrap();
        assert_eq!(skepta.album_count, 1);
        assert_eq!(skepta.track_count, 1);

        let albums = store.load_albums(std::slice::from_ref(&root)).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist, "A$AP Rocky");
        assert_eq!(albums[0].track_count, 2);

        let tracks = store.load_tracks(&[root]).unwrap();
        assert!(
            tracks
                .iter()
                .any(|track| track.artist == "A$AP Rocky feat/ Skepta")
        );
    }
}
