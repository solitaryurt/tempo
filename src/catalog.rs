use std::{
    collections::{HashMap, HashSet},
    fs,
    mem::{size_of, size_of_val},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};

use crate::{
    library::{Artwork, Track},
    perf,
};

const APP_DIR: &str = "tempo";
const TRACK_METADATA_VERSION: i64 = 2;
/// Bump this whenever the SQL schema or any migration step in `migrate()`
/// changes. Startup short-circuits when the SQLite `user_version` already
/// matches, so this is the only knob that controls whether DDL re-runs on
/// app launch.
// v4 bump rationale: an earlier v3 build advertised SCHEMA_VERSION=3 but
// could leave the `liked` column unadded on a subset of upgraded
// databases (the fast path at `migrate()` short-circuits on
// `user_version >= SCHEMA_VERSION`, so the column-add never re-ran on
// machines that landed at user_version=3 without the ALTER). Bumping to
// 4 forces the migration body -- entirely composed of idempotent
// `CREATE ... IF NOT EXISTS` and `add_column_if_missing` calls -- to
// re-execute exactly once and repair the schema without disturbing
// existing data.
//
// v5 bump rationale: extends the metadata-enrichment surface in
// preparation for multi-source fallbacks (TheAudioDB search, Wikipedia,
// Discogs) and for unified error categorization on the new Errors page.
// New nullable columns:
//   - `albums.description` + `albums.description_source`
//   - `albums.discogs_master_id` (unique index)
//   - `artists.discogs_id`
//   - `metadata_jobs.error_kind` + `metadata_jobs.tried_sources`
//   - `discography_items.discogs_master_id` (unique index)
//   - `discography_items.source` (default 'musicbrainz')
//   - `discography_items.role` (default 'Main')
//   - `discography_items.format`
// Plus the `discography_items` UNIQUE constraint expands from
// `(artist_id, normalized_title, release_type)` to
// `(artist_id, normalized_title, release_type, role)` so that Discogs
// can contribute Main + Appearance rows for the same album without
// colliding with each other. The migration replaces the old unique
// index with the new compound one in place; existing rows default to
// `role = 'Main'` so the new key remains satisfied.
//
// v6 bump rationale: the v5 schema kept a column-level
// `UNIQUE(musicbrainz_release_group_id)` from the original schema,
// which collides with the v5 multi-row design. With `role` now part of
// the natural key, the same release-group MBID can legitimately appear
// in more than one row per artist (Main + Appearance). The column
// constraint forced a global one-row-per-MBID rule that broke the
// MusicBrainz discography job whenever an MBID was already known on
// another artist. The v6 rebuild drops that column-level UNIQUE and
// replaces it with a partial unique index keyed on
// `(artist_id, musicbrainz_release_group_id)` so each artist may have
// at most one row per MBID, and across artists the MBID can repeat
// freely. The same change applies to `discogs_master_id`.
const SCHEMA_VERSION: i64 = 6;

#[derive(Clone, Debug)]
pub struct CatalogStore {
    db_path: PathBuf,
    cache_dir: PathBuf,
    /// Single pooled SQLite connection. Wrapped in `Arc<Mutex<_>>` so the
    /// store remains cheaply cloneable (background threads, scoped scans)
    /// while every caller serializes through one PRAGMA-tuned handle.
    /// PRAGMAs run exactly once in [`CatalogStore::initialize_connection`].
    connection: Arc<Mutex<Connection>>,
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
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub year: Option<String>,
    pub date_added: SystemTime,
    pub duration: Duration,
    pub codec: String,
    pub bitrate: Option<u32>,
    pub file_size: u64,
    pub play_count: u32,
    pub liked: bool,
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
    pub description: Option<String>,
    /// Tri-state description state for the UI: when the description is
    /// `None` we still need to know whether enrichment is "pending"
    /// (no terminal answer yet) versus "unavailable" (every fallback
    /// source has been exhausted). Populated from the underlying
    /// `metadata_status` + `error_kind` columns.
    pub description_state: AlbumDescriptionState,
}

/// Snapshot of the `(metadata_status, error_kind)` pair on an album row,
/// reduced to a tri-state the album hero can render directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AlbumDescriptionState {
    /// Either online metadata is disabled, or no terminal status has
    /// been reached yet. Consumers should optimistically render a
    /// placeholder + queue an on-demand enrichment job.
    #[default]
    Pending,
    /// A description is on hand (`description` is `Some`) and the row
    /// status is `resolved`.
    Available,
    /// Every fallback source was tried and produced no description.
    /// Render the "no online description available" copy.
    Unavailable,
}

impl AlbumDescriptionState {
    /// Stable wire encoding for the binary startup snapshot. Keep these
    /// values stable across snapshot version bumps; the snapshot loader
    /// invalidates the file on `from_u8` mismatch via the `Pending`
    /// fallback rather than refusing the whole snapshot.
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::Available => 1,
            Self::Unavailable => 2,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Available,
            2 => Self::Unavailable,
            _ => Self::Pending,
        }
    }
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

/// One failed metadata-job row joined with the underlying entity for
/// display on the unified Errors page. Cheaply
/// produced by [`CatalogStore::load_metadata_errors`]; the UI groups
/// rows by category and renders them inline with scan errors.
#[derive(Clone, Debug)]
pub struct CatalogMetadataError {
    pub job_id: i64,
    pub entity_type: String,
    pub entity_id: i64,
    pub job_type: String,
    /// "musicbrainz" / "theaudiodb" / "discogs" / "wikipedia" /
    /// "coverartarchive". `None` when the failure happened before any
    /// HTTP call (e.g. the worker bailed because an MBID was missing
    /// before we tried any source). Populated from
    /// `metadata_jobs.tried_sources`'s last entry.
    pub source: Option<String>,
    /// Stable classifier produced by the worker's HTTP helpers:
    /// `network` / `http_4xx` / `http_5xx` / `parse` / `no_match`.
    /// `None` for jobs that failed without going through the helpers.
    pub error_kind: Option<String>,
    /// Most recent `last_error` value -- the persisted message text
    /// that produced the failure. Includes status + URL excerpts for
    /// HTTP errors.
    pub message: String,
    /// Human-readable description of the entity, like "Brian Eno" or
    /// "Kid A — Radiohead". Used in the row's PATH column.
    pub entity_label: String,
    /// Last-update timestamp from the metadata_jobs row, milliseconds
    /// since the unix epoch.
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug)]
pub struct CatalogMetadataArtist {
    pub artist_id: i64,
    pub name: String,
    pub normalized_name: String,
    pub musicbrainz_id: Option<String>,
    /// Discogs artist id, populated by
    /// `resolve_artist_discogs_search`. Independent of
    /// `musicbrainz_id`; the worker chains may resolve either or both
    /// over the lifetime of an artist row.
    pub discogs_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CatalogMetadataAlbum {
    pub album_id: i64,
    pub artist_id: i64,
    pub title: String,
    pub normalized_title: String,
    /// Display name of the album's primary artist. Surfaced for
    /// metadata-source lookups (e.g. TheAudioDB album search) that
    /// need both the artist name and the album title.
    pub artist_name: String,
    pub artist_musicbrainz_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    /// Discogs master id resolved by `resolve_album_discogs_search`.
    /// Independent of `musicbrainz_release_group_id`.
    pub discogs_master_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CatalogMetadataActivity {
    pub pending: usize,
    pub running: usize,
    pub failed: usize,
    pub pending_artist_resolve: usize,
    pub pending_artist_profile: usize,
    pub pending_artist_discography: usize,
    pub pending_album_resolve: usize,
    pub pending_album_cover: usize,
    pub pending_album_profile: usize,
    /// Pending `fetch_artist_discogs_releases` jobs. Counted separately
    /// from `pending_artist_discography` (the MusicBrainz release-group
    /// crawl) so the activity panel can surface Discogs progress
    /// alongside MB progress without conflating them.
    pub pending_artist_discogs_releases: usize,
    /// Pending `fetch_discogs_thumb` jobs. These are per-row thumbnail
    /// downloads enqueued lazily by `fetch_artist_discogs_releases`,
    /// and tend to dominate the queue volume for an artist with a
    /// large discography. Surfaced as its own counter so the UI can
    /// distinguish "thumbnails downloading" from "still resolving
    /// metadata".
    pub pending_thumb_fetch: usize,
    // ---- Phase 2 / Phase 3 fallback chain counters ------------------
    pub pending_artist_audiodb_search: usize,
    pub pending_album_audiodb_search: usize,
    pub pending_artist_wikipedia_summary: usize,
    pub pending_album_wikipedia_summary: usize,
    pub pending_artist_discogs_search: usize,
    pub pending_album_discogs_search: usize,
    pub pending_artist_discogs_profile: usize,
    pub pending_album_discogs_image: usize,
    // ---- Lidarr (tier 0) primary fetch counters ---------------------
    pub pending_artist_lidarr: usize,
    pub pending_album_lidarr: usize,
}

impl CatalogMetadataActivity {
    pub fn is_active(&self) -> bool {
        self.pending > 0 || self.running > 0
    }
}

/// Result of [`CatalogStore::load_discogs_thumb_target`]. Carries the
/// `discography_items.id` to update + the URL to download. Returned
/// only when the row has a non-empty `thumb_url` and no cover asset
/// yet; the thumb-fetch worker treats `None` as a no-op success.
#[derive(Clone, Debug)]
pub struct DiscogsThumbTarget {
    pub item_id: i64,
    pub source_url: String,
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
        Self::open_at(data_dir.join("tempo.sqlite"), cache_dir)
    }

    /// Construct a `CatalogStore` at explicit paths. Used by tests and any
    /// callers that need a non-default location.
    pub fn open_at(db_path: PathBuf, cache_dir: PathBuf) -> Result<Self> {
        let connection = Self::initialize_connection(&db_path)?;
        let store = Self {
            db_path,
            cache_dir,
            connection: Arc::new(Mutex::new(connection)),
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

    /// Open and PRAGMA-tune a fresh connection. Intended to be called once
    /// per `CatalogStore` instance; the resulting handle is then pooled in
    /// `self.connection` and reused for every catalog operation.
    fn initialize_connection(db_path: &Path) -> Result<Connection> {
        let _span = perf::slow_span(
            "catalog.initialize_connection",
            Duration::from_millis(8),
            format!("db={}", db_path.display()),
        );
        let connection = Connection::open(db_path)
            .with_context(|| format!("failed to open {}", db_path.display()))?;
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

    /// Lock the pooled connection. Every catalog method routes through this
    /// helper so PRAGMAs and the page cache are reused across calls. The
    /// guard is held for the duration of one operation; transactions are
    /// expected to commit before the guard drops.
    fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("catalog connection mutex poisoned"))
    }

    fn migrate(&self) -> Result<()> {
        let _span = perf::span("catalog.migrate_inner", "");
        let connection = self.lock_connection()?;
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
                genre TEXT,
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
                metadata_version INTEGER NOT NULL DEFAULT 0,
                play_count INTEGER NOT NULL DEFAULT 0,
                liked INTEGER NOT NULL DEFAULT 0,
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
                musicbrainz_release_group_id TEXT,
                discogs_master_id TEXT,
                cover_asset_id INTEGER REFERENCES assets(id),
                local_album_id INTEGER REFERENCES albums(id),
                is_local INTEGER NOT NULL DEFAULT 0,
                source TEXT NOT NULL DEFAULT 'musicbrainz',
                role TEXT NOT NULL DEFAULT 'Main',
                format TEXT,
                sort_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(artist_id, normalized_title, release_type, role),
                UNIQUE(artist_id, musicbrainz_release_group_id)
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
        add_column_if_missing(&connection, "tracks", "genre", "TEXT")?;
        add_column_if_missing(
            &connection,
            "tracks",
            "metadata_version",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        add_column_if_missing(&connection, "tracks", "date_added", "INTEGER")?;
        add_column_if_missing(
            &connection,
            "tracks",
            "play_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        add_column_if_missing(&connection, "tracks", "first_played_at", "INTEGER")?;
        add_column_if_missing(&connection, "tracks", "last_played_at", "INTEGER")?;
        add_column_if_missing(&connection, "tracks", "liked", "INTEGER NOT NULL DEFAULT 0")?;
        connection.execute(
            "UPDATE tracks SET date_added = created_at WHERE date_added IS NULL",
            [],
        )?;

        // ---- v5 additions (idempotent) -------------------------------
        // Albums: description + provenance + Discogs identity.
        add_column_if_missing(&connection, "albums", "description", "TEXT")?;
        add_column_if_missing(&connection, "albums", "description_source", "TEXT")?;
        add_column_if_missing(&connection, "albums", "discogs_master_id", "TEXT")?;
        connection.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS albums_discogs_master_idx
             ON albums(discogs_master_id) WHERE discogs_master_id IS NOT NULL;",
        )?;

        // Artists: Discogs identity.
        add_column_if_missing(&connection, "artists", "discogs_id", "TEXT")?;

        // Metadata jobs: error classification + tried-sources audit log.
        add_column_if_missing(&connection, "metadata_jobs", "error_kind", "TEXT")?;
        add_column_if_missing(&connection, "metadata_jobs", "tried_sources", "TEXT")?;

        // Discography items: Discogs identity, source + role + format.
        add_column_if_missing(
            &connection,
            "discography_items",
            "discogs_master_id",
            "TEXT",
        )?;
        add_column_if_missing(
            &connection,
            "discography_items",
            "source",
            "TEXT NOT NULL DEFAULT 'musicbrainz'",
        )?;
        add_column_if_missing(
            &connection,
            "discography_items",
            "role",
            "TEXT NOT NULL DEFAULT 'Main'",
        )?;
        add_column_if_missing(&connection, "discography_items", "format", "TEXT")?;
        // `thumb_url` stores the per-row Discogs thumb so the
        // `fetch_discogs_thumb` worker can re-derive the source URL
        // without a second `/artists/{id}/releases` round trip.
        add_column_if_missing(&connection, "discography_items", "thumb_url", "TEXT")?;
        connection.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS discography_items_discogs_master_idx
             ON discography_items(discogs_master_id) WHERE discogs_master_id IS NOT NULL;",
        )?;

        // Replace the old `(artist_id, normalized_title, release_type)`
        // UNIQUE on `discography_items` with the new compound key
        // including `role`. Existing rows default to `role = 'Main'` so
        // nothing collides on the new index. We can't drop an inline
        // table-level UNIQUE without rebuilding the table, but we can
        // rebuild it once on upgrade and leave the new schema clean for
        // fresh installs (the CREATE TABLE above already has the new
        // 4-column UNIQUE).
        rebuild_discography_items_unique(&connection)?;

        // Recover metadata-jobs that were poisoned by the pre-v6
        // `UNIQUE constraint failed: discography_items.musicbrainz_release_group_id`
        // bug. The constraint no longer exists; clear those rows back
        // to a fresh `pending` so the worker retries them on the next
        // tick. Also clears the entity-side `metadata_status='error'`
        // markers that the failure path wrote.
        connection.execute(
            "UPDATE metadata_jobs
             SET status = 'pending',
                 next_attempt_at = 0,
                 attempts = 0,
                 last_error = NULL,
                 error_kind = NULL,
                 updated_at = ?1
             WHERE last_error LIKE '%UNIQUE constraint failed: discography_items.musicbrainz_release_group_id%'",
            params![now_millis()],
        )?;
        connection.execute(
            "UPDATE artists
             SET metadata_status = CASE
                    WHEN musicbrainz_id IS NULL THEN 'missing'
                    ELSE 'resolved'
                 END,
                 metadata_error = NULL,
                 updated_at = ?1
             WHERE metadata_status = 'error'
               AND (metadata_error IS NULL
                    OR metadata_error LIKE '%UNIQUE constraint failed: discography_items.musicbrainz_release_group_id%')",
            params![now_millis()],
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
        let mut connection = self.lock_connection()?;
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

    pub fn finish_scan(&self, scan_id: i64, roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
        let _span = perf::span(
            "catalog.finish_scan",
            format!("scan_id={scan_id} roots={}", roots.len()),
        );
        if roots.is_empty() {
            return Ok(Vec::new());
        }

        let mut connection = self.lock_connection()?;
        let now = now_millis();
        let transaction = connection.transaction()?;

        for root in roots {
            transaction.execute(
                "UPDATE library_roots SET last_scan_finished_at = ?1 WHERE path = ?2",
                params![now, root.display().to_string()],
            )?;
        }

        let missing_paths = {
            let mut statement = transaction.prepare_cached(
                "SELECT path FROM files
                 WHERE status = 'present'
                   AND (last_seen_scan_id IS NULL OR last_seen_scan_id <> ?1)
                 ORDER BY path",
            )?;
            statement
                .query_map(params![scan_id], |row| row.get::<_, String>(0))?
                .filter_map(|row| row.ok())
                .map(PathBuf::from)
                .filter(|path| path_in_roots(path, roots))
                .collect::<Vec<_>>()
        };

        {
            let mut statement = transaction.prepare_cached(
                "UPDATE files
                 SET status = 'missing', missing_since = COALESCE(missing_since, ?1), updated_at = ?1
                 WHERE path = ?2",
            )?;
            for path in &missing_paths {
                statement.execute(params![now, path.display().to_string()])?;
            }
        }
        transaction.execute(
            "UPDATE scan_runs SET finished_at = ?1, status = 'finished' WHERE id = ?2",
            params![now, scan_id],
        )?;

        transaction.commit()?;
        Ok(missing_paths)
    }

    /// Single-track upsert, kept for the file-watcher path that processes
    /// one notify event at a time. For batch scans, prefer
    /// [`Self::upsert_tracks_batch`] which amortizes the transaction +
    /// statement-cache overhead across many tracks in one DB round trip.
    pub fn upsert_track(&self, track: &Track, scan_id: Option<i64>) -> Result<CatalogTrack> {
        let _span = perf::slow_span(
            "catalog.upsert_track",
            Duration::from_millis(25),
            format!("path={}", track.path.display()),
        );
        let mut results = self.upsert_tracks_batch(std::slice::from_ref(track), scan_id)?;
        results
            .pop()
            .context("upsert_tracks_batch returned empty result")
    }

    /// Bulk-upsert N tracks inside a single SQLite transaction, reusing
    /// `prepare_cached` statement handles for every per-track step. This
    /// is the hot path for cold scans: one transaction = one fsync, and
    /// statement cache hits eliminate ~80% of the SQL parsing cost.
    ///
    /// Returns the resulting [`CatalogTrack`] for each input track in the
    /// same order. Failure aborts the entire batch (the transaction is not
    /// committed); callers should retry track-by-track if partial progress
    /// is required.
    pub fn upsert_tracks_batch(
        &self,
        tracks: &[Track],
        scan_id: Option<i64>,
    ) -> Result<Vec<CatalogTrack>> {
        let _span = perf::slow_span(
            "catalog.upsert_tracks_batch",
            Duration::from_millis(25),
            format!("count={}", tracks.len()),
        );
        if tracks.is_empty() {
            return Ok(Vec::new());
        }

        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let mut results = Vec::with_capacity(tracks.len());

        for track in tracks {
            results.push(Self::upsert_track_in_transaction(
                &transaction,
                track,
                scan_id,
                now,
                &self.cache_dir,
            )?);
        }

        transaction.commit()?;
        Ok(results)
    }

    /// Per-track work performed inside an outer batched transaction. All
    /// SQL goes through `prepare_cached` so a long-running batch only
    /// parses each statement once.
    fn upsert_track_in_transaction(
        transaction: &rusqlite::Transaction<'_>,
        track: &Track,
        scan_id: Option<i64>,
        now: i64,
        cache_dir: &Path,
    ) -> Result<CatalogTrack> {
        let primary_artist = primary_artist_name(&track.artist);
        let artist_id = upsert_artist(transaction, &primary_artist, now)?;
        let album_id = upsert_album(
            transaction,
            &track.album,
            &primary_artist,
            artist_id,
            track.year.as_deref(),
            now,
        )?;
        let (artwork_asset_id, artwork_path) = persist_artwork(transaction, cache_dir, track, now)?;
        let artist_musicbrainz_id: Option<String> = transaction
            .prepare_cached("SELECT musicbrainz_id FROM artists WHERE id = ?1")?
            .query_row(params![artist_id], |row| row.get(0))
            .optional()?
            .flatten();
        if artist_musicbrainz_id.is_none() {
            enqueue_metadata_job(
                transaction,
                "artist",
                artist_id,
                "resolve_artist_musicbrainz",
                now,
            )?;
        }
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

        transaction
            .prepare_cached(
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
            )?
            .execute(params![
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
            ])?;
        let file_id = select_id_by_text(transaction, "files", "path", &path)?;
        let search_blob = format!(
            "{} {} {} {} {} {} {}",
            track.title,
            track.artist,
            track.album,
            track.genre.as_deref().unwrap_or_default(),
            track.year.as_deref().unwrap_or_default(),
            track.codec,
            path
        )
        .to_lowercase();
        let duration_ms = track.duration.as_millis().min(i64::MAX as u128) as i64;

        transaction
            .prepare_cached(
                "INSERT INTO tracks(
                    file_id, artist_id, album_id, title, artist_name, album_name, genre, track_number, year,
                    date_added, duration_ms,
                    codec, bitrate, sample_rate, channels, file_size, modified_at, artwork_asset_id,
                    artwork_path, metadata_version, search_blob, created_at, updated_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?22)
                 ON CONFLICT(file_id) DO UPDATE SET
                    artist_id = excluded.artist_id,
                    album_id = excluded.album_id,
                    title = excluded.title,
                    artist_name = excluded.artist_name,
                    album_name = excluded.album_name,
                    genre = excluded.genre,
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
                    metadata_version = excluded.metadata_version,
                    search_blob = excluded.search_blob,
                    updated_at = excluded.updated_at",
            )?
            .execute(params![
                file_id,
                artist_id,
                album_id,
                &track.title,
                &track.artist,
                &track.album,
                track.genre.as_deref(),
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
                TRACK_METADATA_VERSION,
                &search_blob,
                now,
            ])?;
        let track_id = select_id_by_i64(transaction, "tracks", "file_id", file_id)?;
        let (date_added_ms, play_count, liked): (i64, i64, i64) = transaction
            .prepare_cached("SELECT date_added, play_count, liked FROM tracks WHERE id = ?1")?
            .query_row(params![track_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;

        Ok(CatalogTrack {
            track_id,
            file_id,
            artist_id,
            album_id,
            path: track.path.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            genre: track.genre.clone(),
            track_number: track.track_number,
            year: track.year.clone(),
            date_added: millis_to_system_time(date_added_ms),
            duration: track.duration,
            codec: track.codec.clone(),
            bitrate: track.bitrate,
            file_size: size_bytes,
            play_count: play_count.max(0) as u32,
            liked: liked != 0,
            artwork_path,
        })
    }

    pub fn mark_file_removed(&self, path: &Path) -> Result<()> {
        let _span = perf::span(
            "catalog.mark_file_removed",
            format!("path={}", path.display()),
        );
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE files
             SET status = 'missing', missing_since = COALESCE(missing_since, ?1), removed_at = ?1, updated_at = ?1
             WHERE path = ?2",
            params![now, path.display().to_string()],
        )?;
        Ok(())
    }

    pub fn mark_folder_removed(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let _span = perf::span(
            "catalog.mark_folder_removed",
            format!("path={}", path.display()),
        );
        let mut connection = self.lock_connection()?;
        let now = now_millis();
        let transaction = connection.transaction()?;
        let folder = path.display().to_string();
        let folder = folder.trim_end_matches('/');
        let prefix = if folder.is_empty() || folder == "/" {
            "/".to_string()
        } else {
            format!("{}/", escape_like(folder))
        };

        let removed_paths = {
            let mut statement = transaction.prepare_cached(
                "SELECT path FROM files
                 WHERE status = 'present'
                   AND (path = ?1 OR path LIKE ?2 ESCAPE '\\')
                 ORDER BY path",
            )?;
            statement
                .query_map(params![folder, format!("{prefix}%")], |row| {
                    row.get::<_, String>(0)
                })?
                .filter_map(|row| row.ok())
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        };

        {
            let mut statement = transaction.prepare_cached(
                "UPDATE files
                 SET status = 'missing', missing_since = COALESCE(missing_since, ?1), removed_at = ?1, updated_at = ?1
                 WHERE path = ?2",
            )?;
            for path in &removed_paths {
                statement.execute(params![now, path.display().to_string()])?;
            }
        }

        transaction.commit()?;
        Ok(removed_paths)
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

        let connection = self.lock_connection()?;
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

        let connection = self.lock_connection()?;
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

        let connection = self.lock_connection()?;
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

        let connection = self.lock_connection()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare_cached(&format!(
            "SELECT
                tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                tracks.artist_name, tracks.album_name, tracks.genre, tracks.track_number, tracks.year,
                tracks.date_added, tracks.duration_ms, tracks.codec, tracks.bitrate,
                tracks.file_size, tracks.play_count, tracks.liked, tracks.artwork_path
             FROM tracks
             JOIN files ON files.id = tracks.file_id
             WHERE files.status = 'present' AND ({root_filter})
             ORDER BY files.path",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let path: String = row.get(4)?;
            let track_number: Option<i64> = row.get(9)?;
            let date_added_ms: i64 = row.get(11)?;
            let duration_ms: i64 = row.get(12)?;
            let bitrate: Option<i64> = row.get(14)?;
            let file_size: i64 = row.get(15)?;
            let play_count: i64 = row.get(16)?;
            let liked: i64 = row.get(17)?;
            let artwork_path: Option<String> = row.get(18)?;
            Ok(CatalogTrack {
                track_id: row.get(0)?,
                file_id: row.get(1)?,
                artist_id: row.get(2)?,
                album_id: row.get(3)?,
                path: PathBuf::from(path),
                title: row.get(5)?,
                artist: row.get(6)?,
                album: row.get(7)?,
                genre: row.get(8)?,
                track_number: track_number.map(|track_number| track_number as u32),
                year: row.get(10)?,
                date_added: millis_to_system_time(date_added_ms),
                duration: Duration::from_millis(duration_ms.max(0) as u64),
                codec: row.get(13)?,
                bitrate: bitrate.map(|bitrate| bitrate as u32),
                file_size: file_size.max(0) as u64,
                play_count: play_count.max(0) as u32,
                liked: liked != 0,
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

        let connection = self.lock_connection()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare_cached(&format!(
            "SELECT
                tracks.id, files.id, tracks.artist_id, tracks.album_id, files.path, tracks.title,
                tracks.artist_name, tracks.album_name, tracks.genre, tracks.track_number, tracks.year,
                tracks.date_added, tracks.duration_ms, tracks.codec, tracks.bitrate,
                tracks.file_size, tracks.play_count, tracks.liked, tracks.artwork_path,
                files.size_bytes, files.modified_at, files.device_id, files.inode
             FROM tracks
             JOIN files ON files.id = tracks.file_id
             WHERE files.status = 'present'
               AND tracks.metadata_version = ?
               AND ({root_filter})",
        ))?;
        let mut params = Vec::with_capacity(root_params.len() + 1);
        params.push(Value::Integer(TRACK_METADATA_VERSION));
        params.extend(root_params.into_iter().map(Value::Text));
        let rows = statement.query_map(params_from_iter(params), |row| {
            let path: String = row.get(4)?;
            let track_number: Option<i64> = row.get(9)?;
            let date_added_ms: i64 = row.get(11)?;
            let duration_ms: i64 = row.get(12)?;
            let bitrate: Option<i64> = row.get(14)?;
            let file_size: i64 = row.get(15)?;
            let play_count: i64 = row.get(16)?;
            let liked: i64 = row.get(17)?;
            let artwork_path: Option<String> = row.get(18)?;
            let path = PathBuf::from(path);
            Ok((
                path.clone(),
                CatalogFileFingerprint {
                    size_bytes: row.get::<_, i64>(19)?.max(0) as u64,
                    modified_at: row.get(20)?,
                    device_id: row.get(21)?,
                    inode: row.get(22)?,
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
                    genre: row.get(8)?,
                    track_number: track_number.map(|track_number| track_number as u32),
                    year: row.get(10)?,
                    date_added: millis_to_system_time(date_added_ms),
                    duration: Duration::from_millis(duration_ms.max(0) as u64),
                    codec: row.get(13)?,
                    bitrate: bitrate.map(|bitrate| bitrate as u32),
                    file_size: file_size.max(0) as u64,
                    play_count: play_count.max(0) as u32,
                    liked: liked != 0,
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

    /// Persist the `liked` flag for the given track. The Liked column /
    /// Liked page in the UI both flow through here so the in-memory
    /// `Track::liked` field and the on-disk `tracks.liked` column stay
    /// in sync. Returns Ok(()) even if no rows match (the in-memory
    /// state is the source of truth for the UI; this is just durability).
    pub fn set_liked(&self, path: &Path, liked: bool) -> Result<()> {
        let _span = perf::span(
            "catalog.set_liked",
            format!("path={} liked={liked}", path.display()),
        );
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE tracks
             SET liked = ?1,
                 updated_at = ?2
             WHERE file_id = (SELECT id FROM files WHERE path = ?3)",
            params![if liked { 1 } else { 0 }, now, path.display().to_string()],
        )?;
        Ok(())
    }

    pub fn increment_play_count(&self, path: &Path) -> Result<u32> {
        let _span = perf::span(
            "catalog.increment_play_count",
            format!("path={}", path.display()),
        );
        let connection = self.lock_connection()?;
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

        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        {
            let mut statement = transaction.prepare_cached(
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
        let connection = self.lock_connection()?;
        enqueue_metadata_job(&connection, entity_type, entity_id, job_type, now_millis())
    }

    /// Run `PRAGMA optimize` on the pooled connection. SQLite uses this
    /// hook to update internal statistics and maintain stat indexes
    /// when needed. Cheap on no-op runs (the recommended pattern is to
    /// invoke it at shutdown). Failures are ignored on purpose -- there
    /// is nothing useful to do with the error during teardown.
    pub fn run_optimize(&self) {
        let _span = perf::span("catalog.run_optimize", "");
        let Ok(connection) = self.lock_connection() else {
            return;
        };
        let _ = connection.execute_batch("PRAGMA optimize;");
    }

    pub fn reset_stale_metadata_jobs(&self) -> Result<usize> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        let stale_before = now - 5 * 60 * 1000;
        let reset = connection.execute(
            "UPDATE metadata_jobs
             SET status = 'pending', next_attempt_at = ?1, updated_at = ?1
             WHERE status = 'running' AND updated_at < ?2",
            params![now, stale_before],
        )?;
        Ok(reset)
    }

    /// Aggressive one-shot resync of every artist/album that is short
    /// of a bio, photo, description, or cover. Used by the
    /// "Resync metadata" Settings button and once on first launch
    /// after the v6 schema migration to pull existing libraries
    /// forward through the multi-source fallback chain.
    ///
    /// Concretely, for each artist still missing a bio:
    ///
    /// - if `fetch_artist_profile` is `complete` but bio is empty,
    ///   enqueue `fetch_artist_wikipedia_summary`.
    /// - if Wikipedia already ran complete and bio is still empty,
    ///   enqueue `resolve_artist_discogs_search` (Discogs profile
    ///   chains automatically on resolution).
    ///
    /// Mirror logic applies to albums (description, cover).
    ///
    /// Also clears exponential backoff on every `pending` job so the
    /// worker tackles them immediately, and recovers any artist row
    /// stuck at `metadata_status = 'error'`.
    ///
    /// Returns the number of jobs (re-)enqueued.
    pub fn resync_metadata_enrichment(&self) -> Result<usize> {
        let _span = perf::span("catalog.resync_metadata_enrichment", "");
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let mut enqueued = 0usize;

        // 1. Clear exponential backoff on every pending job + reset
        //    attempts. The worker will re-pick them up on the next
        //    tick. We don't touch `complete` rows here -- those are
        //    re-armed selectively below.
        transaction.execute(
            "UPDATE metadata_jobs
             SET next_attempt_at = 0,
                 attempts = 0,
                 last_error = NULL,
                 error_kind = NULL,
                 updated_at = ?1
             WHERE status = 'pending'",
            params![now],
        )?;

        // 2. Recover artist rows that landed in `error` (or were
        //    marked `missing` by a bygone failure) so the activity
        //    panel reads cleanly.
        transaction.execute(
            "UPDATE artists
             SET metadata_status = CASE
                    WHEN musicbrainz_id IS NULL THEN 'missing'
                    ELSE 'resolved'
                 END,
                 metadata_error = NULL,
                 updated_at = ?1
             WHERE metadata_status IN ('error', 'missing')",
            params![now],
        )?;
        transaction.execute(
            "UPDATE albums
             SET metadata_status = CASE
                    WHEN musicbrainz_release_group_id IS NULL THEN 'missing'
                    ELSE 'resolved'
                 END,
                 metadata_error = NULL,
                 updated_at = ?1
             WHERE metadata_status IN ('error', 'missing')",
            params![now],
        )?;

        // 3. Walk artists and enqueue the *next* link in the chain.
        //    The query surfaces, per artist, whether each step in the
        //    chain has previously completed so we can skip ahead.
        let artist_rows = {
            let mut statement = transaction.prepare(
                "SELECT
                    artists.id,
                    artists.musicbrainz_id,
                    artists.discogs_id,
                    artists.bio,
                    artists.photo_asset_id,
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='resolve_artist_musicbrainz' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='fetch_artist_lidarr' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='fetch_artist_profile' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='fetch_artist_wikipedia_summary' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='resolve_artist_audiodb_search' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='resolve_artist_discogs_search' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='artist' AND j.entity_id=artists.id
                              AND j.job_type='fetch_artist_discogs_profile' AND j.status='complete')
                 FROM artists",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, i64>(8)? != 0,
                    row.get::<_, i64>(9)? != 0,
                    row.get::<_, i64>(10)? != 0,
                    row.get::<_, i64>(11)? != 0,
                ))
            })?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        };

        for (
            artist_id,
            musicbrainz_id,
            discogs_id,
            bio,
            photo_asset_id,
            mb_resolved,
            lidarr_done,
            profile_done,
            wikipedia_done,
            audiodb_search_done,
            discogs_search_done,
            discogs_profile_done,
        ) in artist_rows
        {
            let bio_present = bio.as_deref().is_some_and(|s| !s.trim().is_empty());

            // Identity gap: no MBID, no Discogs id. Walk the chain in
            // order: MB search -> TADb search -> Discogs search.
            if musicbrainz_id.is_none() && discogs_id.is_none() {
                if !mb_resolved {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "resolve_artist_musicbrainz",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !audiodb_search_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "resolve_artist_audiodb_search",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !discogs_search_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "resolve_artist_discogs_search",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                }
                continue;
            }

            // Bio gap: Lidarr first (single round-trip), then walk
            // TADb profile -> Wikipedia -> Discogs profile.
            if !bio_present || photo_asset_id.is_none() {
                if !lidarr_done && musicbrainz_id.is_some() {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "fetch_artist_lidarr",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !profile_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "fetch_artist_profile",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !wikipedia_done && !bio_present {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "fetch_artist_wikipedia_summary",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !bio_present {
                    if discogs_id.is_some() {
                        if !discogs_profile_done {
                            enqueue_metadata_job_at(
                                &transaction,
                                "artist",
                                artist_id,
                                "fetch_artist_discogs_profile",
                                0,
                                now,
                            )?;
                            enqueued += 1;
                        }
                    } else if !discogs_search_done {
                        enqueue_metadata_job_at(
                            &transaction,
                            "artist",
                            artist_id,
                            "resolve_artist_discogs_search",
                            0,
                            now,
                        )?;
                        enqueued += 1;
                    }
                }
            }

            // Always re-arm discography so the artist single view fills
            // even if the bio chain is exhausted.
            enqueue_metadata_job_at(
                &transaction,
                "artist",
                artist_id,
                "fetch_artist_discography",
                0,
                now,
            )?;
            enqueued += 1;
        }

        // 4. Walk albums and route around exhausted description jobs
        //    the same way. Cover follow-up too.
        let album_rows = {
            let mut statement = transaction.prepare(
                "SELECT
                    albums.id,
                    albums.musicbrainz_release_group_id,
                    albums.discogs_master_id,
                    albums.description,
                    albums.cover_asset_id,
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='resolve_album_musicbrainz' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='fetch_album_lidarr' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='fetch_album_profile' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='fetch_album_wikipedia_summary' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='resolve_album_audiodb_search' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='resolve_album_discogs_search' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='fetch_album_cover' AND j.status='complete'),
                    EXISTS (SELECT 1 FROM metadata_jobs j
                            WHERE j.entity_type='album' AND j.entity_id=albums.id
                              AND j.job_type='fetch_album_discogs_image' AND j.status='complete')
                 FROM albums",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, i64>(8)? != 0,
                    row.get::<_, i64>(9)? != 0,
                    row.get::<_, i64>(10)? != 0,
                    row.get::<_, i64>(11)? != 0,
                    row.get::<_, i64>(12)? != 0,
                ))
            })?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        };

        for (
            album_id,
            release_group_id,
            discogs_master_id,
            description,
            cover_asset_id,
            mb_resolved,
            lidarr_done,
            profile_done,
            wikipedia_done,
            audiodb_search_done,
            discogs_search_done,
            cover_done,
            discogs_image_done,
        ) in album_rows
        {
            let description_present = description.as_deref().is_some_and(|s| !s.trim().is_empty());

            // Album identity gap: walk MB -> TADb search -> Discogs.
            if release_group_id.is_none() && discogs_master_id.is_none() {
                if !mb_resolved {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "resolve_album_musicbrainz",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !audiodb_search_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "resolve_album_audiodb_search",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !discogs_search_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "resolve_album_discogs_search",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                }
                continue;
            }

            // Description gap: Lidarr first, then TADb album-mb -> Wikipedia.
            if !description_present {
                if !lidarr_done && release_group_id.is_some() {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "fetch_album_lidarr",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !profile_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "fetch_album_profile",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !wikipedia_done {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "fetch_album_wikipedia_summary",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                }
            }

            // Cover gap: Lidarr first (its album payload often
            // includes the CAA-cached cover URL), then CAA -> Discogs
            // image.
            if cover_asset_id.is_none() {
                if !lidarr_done && release_group_id.is_some() {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "fetch_album_lidarr",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !cover_done && release_group_id.is_some() {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "fetch_album_cover",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else if !discogs_image_done && discogs_master_id.is_some() {
                    enqueue_metadata_job_at(
                        &transaction,
                        "album",
                        album_id,
                        "fetch_album_discogs_image",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                }
            }
        }

        transaction.commit()?;
        perf::event(
            "catalog.resync_metadata_enrichment.done",
            format!("enqueued={enqueued}"),
        );
        Ok(enqueued)
    }

    pub fn load_metadata_activity(&self) -> Result<CatalogMetadataActivity> {
        let connection = self.lock_connection()?;
        let mut activity = CatalogMetadataActivity::default();
        let mut statement = connection.prepare_cached(
            "SELECT status, COUNT(*)
             FROM metadata_jobs
             GROUP BY status",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        for row in rows.filter_map(|row| row.ok()) {
            match row.0.as_str() {
                "pending" => activity.pending = row.1,
                "running" => activity.running = row.1,
                "failed" => activity.failed = row.1,
                _ => {}
            }
        }

        let mut statement = connection.prepare_cached(
            "SELECT job_type, COUNT(*)
             FROM metadata_jobs
             WHERE status = 'pending'
             GROUP BY job_type",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        for row in rows.filter_map(|row| row.ok()) {
            match row.0.as_str() {
                "resolve_artist_musicbrainz" => activity.pending_artist_resolve = row.1,
                "fetch_artist_profile" => activity.pending_artist_profile = row.1,
                "fetch_artist_discography" => activity.pending_artist_discography = row.1,
                "resolve_album_musicbrainz" => activity.pending_album_resolve = row.1,
                "fetch_album_cover" => activity.pending_album_cover = row.1,
                "fetch_album_profile" => activity.pending_album_profile = row.1,
                "fetch_artist_discogs_releases" => activity.pending_artist_discogs_releases = row.1,
                "fetch_discogs_thumb" => activity.pending_thumb_fetch = row.1,
                "resolve_artist_audiodb_search" => activity.pending_artist_audiodb_search = row.1,
                "resolve_album_audiodb_search" => activity.pending_album_audiodb_search = row.1,
                "fetch_artist_wikipedia_summary" => {
                    activity.pending_artist_wikipedia_summary = row.1;
                }
                "fetch_album_wikipedia_summary" => {
                    activity.pending_album_wikipedia_summary = row.1;
                }
                "resolve_artist_discogs_search" => activity.pending_artist_discogs_search = row.1,
                "resolve_album_discogs_search" => activity.pending_album_discogs_search = row.1,
                "fetch_artist_discogs_profile" => activity.pending_artist_discogs_profile = row.1,
                "fetch_album_discogs_image" => activity.pending_album_discogs_image = row.1,
                "fetch_artist_lidarr" => activity.pending_artist_lidarr = row.1,
                "fetch_album_lidarr" => activity.pending_album_lidarr = row.1,
                _ => {}
            }
        }
        Ok(activity)
    }

    /// Snapshot every metadata-job row whose status is `'failed'` AND
    /// has a non-null `last_error`, joined with the artist or album it
    /// belongs to so the UI can render an entity label without a
    /// follow-up query. Capped at 500 rows; the Errors page is meant
    /// for triage, not full-history archeology.
    pub fn load_metadata_errors(&self) -> Result<Vec<CatalogMetadataError>> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare_cached(
            "SELECT
                metadata_jobs.id,
                metadata_jobs.entity_type,
                metadata_jobs.entity_id,
                metadata_jobs.job_type,
                metadata_jobs.error_kind,
                metadata_jobs.tried_sources,
                metadata_jobs.last_error,
                metadata_jobs.updated_at,
                COALESCE(artists.name, '') AS artist_name,
                COALESCE(albums.title, '') AS album_title,
                COALESCE(albums.artist_name, '') AS album_artist
             FROM metadata_jobs
             LEFT JOIN artists
                ON metadata_jobs.entity_type = 'artist'
                AND artists.id = metadata_jobs.entity_id
             LEFT JOIN albums
                ON metadata_jobs.entity_type = 'album'
                AND albums.id = metadata_jobs.entity_id
             WHERE metadata_jobs.last_error IS NOT NULL
                AND metadata_jobs.last_error <> ''
             ORDER BY metadata_jobs.updated_at DESC
             LIMIT 500",
        )?;
        let rows = statement.query_map([], |row| {
            let entity_type: String = row.get(1)?;
            let artist_name: String = row.get(8)?;
            let album_title: String = row.get(9)?;
            let album_artist: String = row.get(10)?;
            let entity_label = match entity_type.as_str() {
                "artist" => {
                    if artist_name.is_empty() {
                        format!("artist #{}", row.get::<_, i64>(2)?)
                    } else {
                        artist_name
                    }
                }
                "album" => {
                    if album_title.is_empty() {
                        format!("album #{}", row.get::<_, i64>(2)?)
                    } else if album_artist.is_empty() {
                        album_title
                    } else {
                        format!("{album_title} — {album_artist}")
                    }
                }
                "discography_item" => format!("discography #{}", row.get::<_, i64>(2)?),
                other => format!("{other} #{}", row.get::<_, i64>(2)?),
            };
            let tried_sources: Option<String> = row.get(5)?;
            let source = tried_sources
                .as_deref()
                .and_then(|sources| sources.rsplit(',').next())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Ok(CatalogMetadataError {
                job_id: row.get(0)?,
                entity_type,
                entity_id: row.get(2)?,
                job_type: row.get(3)?,
                error_kind: row.get(4)?,
                source,
                message: row.get::<_, String>(6)?,
                entity_label,
                updated_at_ms: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    pub fn enqueue_missing_online_metadata_jobs(&self) -> Result<usize> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let mut enqueued = 0usize;

        // Surface whether `fetch_artist_profile` already ran for each
        // artist so we can route bio-less artists straight to the
        // Wikipedia fallback instead of bouncing them through TADb
        // again (which won't re-run; the upsert leaves `complete` jobs
        // alone). One-shot subquery built inline.
        let artist_ids = {
            let mut statement = transaction.prepare_cached(
                "SELECT
                    artists.id,
                    artists.musicbrainz_id,
                    artists.bio,
                    artists.photo_asset_id,
                    EXISTS (
                        SELECT 1 FROM metadata_jobs
                        WHERE metadata_jobs.entity_type = 'artist'
                          AND metadata_jobs.entity_id = artists.id
                          AND metadata_jobs.job_type = 'fetch_artist_profile'
                          AND metadata_jobs.status = 'complete'
                    )
                 FROM artists
                 ORDER BY artists.name",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)? != 0,
                ))
            })?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        };

        for (artist_id, musicbrainz_id, bio, photo_asset_id, profile_completed) in artist_ids {
            if musicbrainz_id.is_none() {
                enqueue_metadata_job(
                    &transaction,
                    "artist",
                    artist_id,
                    "resolve_artist_musicbrainz",
                    now,
                )?;
                enqueued += 1;
                continue;
            }

            if bio.is_none() || photo_asset_id.is_none() {
                // Tier-0: Lidarr's hosted proxy fills bio + photo +
                // discography in one round-trip. Always enqueue it
                // first when there's an MBID and we're missing
                // anything; the dispatcher's priority ordering ensures
                // it runs before the legacy chain.
                enqueue_metadata_job_at(
                    &transaction,
                    "artist",
                    artist_id,
                    "fetch_artist_lidarr",
                    0,
                    now,
                )?;
                enqueued += 1;

                if profile_completed && bio.is_none() {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "fetch_artist_wikipedia_summary",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                } else {
                    enqueue_metadata_job_at(
                        &transaction,
                        "artist",
                        artist_id,
                        "fetch_artist_profile",
                        0,
                        now,
                    )?;
                    enqueued += 1;
                }
            }
            enqueue_metadata_job_at(
                &transaction,
                "artist",
                artist_id,
                "fetch_artist_discography",
                0,
                now,
            )?;
            enqueued += 1;
        }

        enqueued += enqueue_missing_album_art_jobs(&transaction, None, now)?;
        enqueued += enqueue_missing_album_description_jobs(&transaction, None, now)?;

        transaction.commit()?;
        Ok(enqueued)
    }

    pub fn enqueue_missing_album_art_jobs_for_artist(&self, artist_id: i64) -> Result<usize> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let enqueued = enqueue_missing_album_art_jobs(&transaction, Some(artist_id), now_millis())?;
        transaction.commit()?;
        Ok(enqueued)
    }

    pub fn enqueue_artist_metadata_demand(&self, artist_id: i64) -> Result<usize> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let artist = transaction
            .query_row(
                "SELECT musicbrainz_id, bio, photo_asset_id FROM artists WHERE id = ?1",
                params![artist_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()?;
        let mut enqueued = 0usize;
        if let Some((musicbrainz_id, bio, photo_asset_id)) = artist {
            // Did `fetch_artist_profile` already run to completion?
            // If so, enqueueing it again is a no-op (the job stays
            // `complete` per the upsert's status guard), so we have to
            // route the on-demand request straight to the next link in
            // the fallback chain (Wikipedia, then Discogs).
            let profile_completed = transaction
                .query_row(
                    "SELECT status FROM metadata_jobs
                     WHERE entity_type = 'artist'
                       AND entity_id = ?1
                       AND job_type = 'fetch_artist_profile'",
                    params![artist_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .as_deref()
                == Some("complete");

            if musicbrainz_id.is_none() {
                enqueue_metadata_job(
                    &transaction,
                    "artist",
                    artist_id,
                    "resolve_artist_musicbrainz",
                    now,
                )?;
                enqueued += 1;
            } else {
                // Lidarr-first: when we already have an MBID, the
                // single-call Lidarr endpoint usually returns the bio
                // + photo + discography we need. Always re-arm it,
                // even when bio/photo are already populated, because
                // Lidarr can also fill in the discography on a row
                // that has a bio from another source.
                enqueue_metadata_job(
                    &transaction,
                    "artist",
                    artist_id,
                    "fetch_artist_lidarr",
                    now,
                )?;
                enqueued += 1;

                if bio.is_none() || photo_asset_id.is_none() {
                    if profile_completed && bio.is_none() {
                        // TADb already returned -- skip to the Wikipedia
                        // fallback so a second open of the artist page
                        // actually advances the chain.
                        enqueue_metadata_job(
                            &transaction,
                            "artist",
                            artist_id,
                            "fetch_artist_wikipedia_summary",
                            now,
                        )?;
                        enqueued += 1;
                    } else {
                        enqueue_metadata_job(
                            &transaction,
                            "artist",
                            artist_id,
                            "fetch_artist_profile",
                            now,
                        )?;
                        enqueued += 1;
                    }
                }
            }

            enqueue_metadata_job(
                &transaction,
                "artist",
                artist_id,
                "fetch_artist_discography",
                now,
            )?;
            enqueued += 1;
            enqueued += enqueue_missing_album_art_jobs(&transaction, Some(artist_id), now)?;
            enqueued += enqueue_missing_album_description_jobs(&transaction, Some(artist_id), now)?;
        }
        transaction.commit()?;
        Ok(enqueued)
    }

    pub fn enqueue_album_cover_demand(&self, album_id: i64) -> Result<usize> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let album = transaction
            .query_row(
                "SELECT albums.cover_asset_id, albums.musicbrainz_release_group_id, artists.musicbrainz_id,
                    EXISTS(
                        SELECT 1 FROM tracks
                        WHERE tracks.album_id = albums.id
                          AND tracks.artwork_path IS NOT NULL
                          AND tracks.artwork_path <> ''
                    )
                 FROM albums
                 JOIN artists ON artists.id = albums.artist_id
                 WHERE albums.id = ?1",
                params![album_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)? != 0,
                    ))
                },
            )
            .optional()?;

        let mut enqueued = 0usize;
        if let Some((cover_asset_id, release_group_id, artist_musicbrainz_id, has_track_artwork)) =
            album
            && cover_asset_id.is_none()
            && !has_track_artwork
        {
            if release_group_id.is_some() {
                // Lidarr ships covers in its album payload; try it first.
                enqueue_metadata_job_at(
                    &transaction,
                    "album",
                    album_id,
                    "fetch_album_lidarr",
                    0,
                    now,
                )?;
                enqueued += 1;
                enqueue_metadata_job_at(
                    &transaction,
                    "album",
                    album_id,
                    "fetch_album_cover",
                    0,
                    now,
                )?;
                enqueued += 1;
            } else if artist_musicbrainz_id.is_some() {
                enqueue_metadata_job_at(
                    &transaction,
                    "album",
                    album_id,
                    "resolve_album_musicbrainz",
                    0,
                    now,
                )?;
                enqueued += 1;
            }
        }

        transaction.commit()?;
        Ok(enqueued)
    }

    /// On-demand companion to [`enqueue_album_cover_demand`]: enqueues
    /// `fetch_album_profile` (TheAudioDB album description) for the
    /// specified album when the album has a release-group MBID and no
    /// description yet. If the release-group MBID is missing, the
    /// release-group resolver is enqueued first; the worker chains
    /// `fetch_album_profile` on success. No-op when neither prerequisite
    /// is reachable (e.g. the album's artist also has no MBID).
    pub fn enqueue_album_profile_demand(&self, album_id: i64) -> Result<usize> {
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        let album = transaction
            .query_row(
                "SELECT
                    albums.description,
                    albums.musicbrainz_release_group_id,
                    artists.musicbrainz_id
                 FROM albums
                 JOIN artists ON artists.id = albums.artist_id
                 WHERE albums.id = ?1",
                params![album_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;

        let mut enqueued = 0usize;
        if let Some((description, release_group_id, artist_musicbrainz_id)) = album
            && description.as_deref().is_none_or(|d| d.trim().is_empty())
        {
            if release_group_id.is_some() {
                // Try Lidarr first -- one round-trip returns the
                // overview directly, no TADb/Wikipedia chaining needed
                // for cached entries.
                enqueue_metadata_job_at(
                    &transaction,
                    "album",
                    album_id,
                    "fetch_album_lidarr",
                    0,
                    now,
                )?;
                enqueued += 1;
                enqueue_metadata_job_at(
                    &transaction,
                    "album",
                    album_id,
                    "fetch_album_profile",
                    0,
                    now,
                )?;
                enqueued += 1;
            } else if artist_musicbrainz_id.is_some() {
                enqueue_metadata_job_at(
                    &transaction,
                    "album",
                    album_id,
                    "resolve_album_musicbrainz",
                    0,
                    now,
                )?;
                enqueued += 1;
            }
        }

        transaction.commit()?;
        Ok(enqueued)
    }

    pub fn claim_next_metadata_job(
        &self,
        supported_job_types: &[&str],
    ) -> Result<Option<CatalogMetadataJob>> {
        if supported_job_types.is_empty() {
            return Ok(None);
        }

        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction()?;
        let now = now_millis();
        // Push the `job_type IN (...)` filter into SQL so we don't pay
        // the per-row deserialization cost for jobs we can't handle. The
        // previous implementation `LIMIT 50`-d, then filtered in Rust,
        // which would re-scan the same prefix forever if all top
        // candidates were unsupported.
        let placeholders = (1..=supported_job_types.len())
            .map(|ix| format!("?{}", ix + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, entity_type, entity_id, job_type, attempts
             FROM metadata_jobs
             WHERE status = 'pending'
               AND next_attempt_at <= ?1
               AND job_type IN ({placeholders})
             ORDER BY CASE job_type
                -- Tier 0: Lidarr's hosted proxy. Single round-trip
                -- returns overview/bio + images + discography + genres
                -- per artist or album. Highest priority so we always
                -- try it before falling back to per-source enrichment.
                WHEN 'fetch_artist_lidarr' THEN 0
                WHEN 'fetch_album_lidarr' THEN 0
                -- Tier 1: MusicBrainz identity resolution. Required to
                -- have an MBID before any of the other sources work.
                WHEN 'resolve_artist_musicbrainz' THEN 1
                WHEN 'resolve_album_musicbrainz' THEN 1
                -- Tier 2: legacy primary jobs (cover art archive,
                -- TheAudioDB profile/album).
                WHEN 'fetch_album_cover' THEN 2
                WHEN 'fetch_artist_profile' THEN 2
                WHEN 'fetch_album_profile' THEN 2
                -- Tier 3: discography + Wikipedia fallbacks.
                WHEN 'fetch_artist_discography' THEN 3
                WHEN 'fetch_artist_wikipedia_summary' THEN 3
                WHEN 'fetch_album_wikipedia_summary' THEN 3
                -- Tier 4: TheAudioDB search fallbacks.
                WHEN 'resolve_artist_audiodb_search' THEN 4
                WHEN 'resolve_album_audiodb_search' THEN 4
                -- Tier 5: Discogs identity + profile.
                WHEN 'resolve_artist_discogs_search' THEN 5
                WHEN 'resolve_album_discogs_search' THEN 5
                WHEN 'fetch_artist_discogs_profile' THEN 5
                WHEN 'fetch_album_discogs_image' THEN 5
                -- Tier 6: Discogs bulk discography crawl.
                WHEN 'fetch_artist_discogs_releases' THEN 6
                -- Tier 9: per-row thumb downloads. Lowest so they
                -- don't starve other work.
                WHEN 'fetch_discogs_thumb' THEN 9
                ELSE 7
             END, next_attempt_at, created_at
             LIMIT 1"
        );
        let job = {
            let mut statement = transaction.prepare_cached(&sql)?;
            let mut params: Vec<&dyn rusqlite::ToSql> =
                Vec::with_capacity(1 + supported_job_types.len());
            params.push(&now);
            for job_type in supported_job_types {
                params.push(job_type);
            }
            statement
                .query_row(rusqlite::params_from_iter(params.iter().copied()), |row| {
                    Ok(CatalogMetadataJob {
                        job_id: row.get(0)?,
                        entity_type: row.get(1)?,
                        entity_id: row.get(2)?,
                        job_type: row.get(3)?,
                        attempts: row.get::<_, i64>(4)?.max(0) as u32,
                    })
                })
                .optional()?
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
        let connection = self.lock_connection()?;
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
        self.fail_metadata_job_classified(job_id, error, None, None)
    }

    /// Like [`fail_metadata_job`], but also persists an `error_kind`
    /// classifier on the job row and appends `tried_source` to the
    /// comma-separated `tried_sources` audit list (deduplicated).
    /// `tried_source` is the symbolic source name (e.g. "musicbrainz",
    /// "theaudiodb", "wikipedia", "discogs", "coverartarchive") and is
    /// only appended when not already present.
    pub fn fail_metadata_job_classified(
        &self,
        job_id: i64,
        error: &str,
        error_kind: Option<&str>,
        tried_source: Option<&str>,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        // We use SQL CASE so the COALESCE/append semantics are
        // expressed in one round-trip. The `tried_sources` column is a
        // comma-separated string; appending preserves history without a
        // separate junction table.
        connection.execute(
            "UPDATE metadata_jobs
             SET status = 'pending',
                 next_attempt_at = ?1 + (60000 * (1 << MIN(attempts, 8))),
                 last_error = ?2,
                 error_kind = COALESCE(?3, error_kind),
                 tried_sources = CASE
                    WHEN ?4 IS NULL THEN tried_sources
                    WHEN tried_sources IS NULL OR tried_sources = '' THEN ?4
                    WHEN INSTR(',' || tried_sources || ',', ',' || ?4 || ',') > 0
                        THEN tried_sources
                    ELSE tried_sources || ',' || ?4
                 END,
                 updated_at = ?1
             WHERE id = ?5",
            params![now, error, error_kind, tried_source, job_id],
        )?;
        Ok(())
    }

    pub fn load_metadata_artist(&self, artist_id: i64) -> Result<Option<CatalogMetadataArtist>> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT id, name, normalized_name, musicbrainz_id, discogs_id
                 FROM artists
                 WHERE id = ?1",
                params![artist_id],
                |row| {
                    Ok(CatalogMetadataArtist {
                        artist_id: row.get(0)?,
                        name: row.get(1)?,
                        normalized_name: row.get(2)?,
                        musicbrainz_id: row.get(3)?,
                        discogs_id: row.get(4)?,
                    })
                },
            )
            .optional()
            .context("failed to load metadata artist")
    }

    pub fn load_metadata_album(&self, album_id: i64) -> Result<Option<CatalogMetadataAlbum>> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT
                    albums.id,
                    albums.artist_id,
                    albums.title,
                    albums.normalized_title,
                    artists.name,
                    artists.musicbrainz_id,
                    albums.musicbrainz_release_group_id,
                    albums.discogs_master_id
                 FROM albums
                 JOIN artists ON artists.id = albums.artist_id
                 WHERE albums.id = ?1",
                params![album_id],
                |row| {
                    Ok(CatalogMetadataAlbum {
                        album_id: row.get(0)?,
                        artist_id: row.get(1)?,
                        title: row.get(2)?,
                        normalized_title: row.get(3)?,
                        artist_name: row.get(4)?,
                        artist_musicbrainz_id: row.get(5)?,
                        musicbrainz_release_group_id: row.get(6)?,
                        discogs_master_id: row.get(7)?,
                    })
                },
            )
            .optional()
            .context("failed to load metadata album")
    }

    pub fn resolve_artist_musicbrainz_id(
        &self,
        artist_id: i64,
        musicbrainz_id: &str,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE artists
             SET musicbrainz_id = ?1,
                 metadata_status = 'resolved',
                 metadata_checked_at = ?2,
                 metadata_error = NULL,
                 updated_at = ?2
             WHERE id = ?3",
            params![musicbrainz_id, now, artist_id],
        )?;
        Ok(())
    }

    pub fn mark_artist_metadata_checked(
        &self,
        artist_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE artists
             SET metadata_status = ?1,
                 metadata_checked_at = ?2,
                 metadata_error = ?3,
                 updated_at = ?2
             WHERE id = ?4",
            params![status, now, error, artist_id],
        )?;
        Ok(())
    }

    /// Persist an artist profile update from any external source. The
    /// `bio_source` argument labels where the bio came from
    /// (`"theaudiodb"`, `"wikipedia"`, etc.) and is only stored when a
    /// non-NULL `bio` is supplied alongside it; this preserves the
    /// previous source label when the caller is only updating
    /// `audiodb_id` or `photo_asset_id`.
    pub fn save_artist_profile(
        &self,
        artist_id: i64,
        audiodb_id: Option<&str>,
        bio: Option<&str>,
        photo_asset_id: Option<i64>,
        bio_source: Option<&str>,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE artists
             SET audiodb_id = COALESCE(?1, audiodb_id),
                 bio = COALESCE(?2, bio),
                 bio_source = CASE
                     WHEN ?2 IS NULL THEN bio_source
                     WHEN ?5 IS NULL THEN bio_source
                     ELSE ?5
                 END,
                 photo_asset_id = COALESCE(?3, photo_asset_id),
                 metadata_status = 'resolved',
                 metadata_checked_at = ?4,
                 metadata_error = NULL,
                 updated_at = ?4
             WHERE id = ?6",
            params![audiodb_id, bio, photo_asset_id, now, bio_source, artist_id],
        )?;
        Ok(())
    }

    pub fn resolve_album_musicbrainz_release_group_id(
        &self,
        album_id: i64,
        musicbrainz_release_group_id: &str,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE albums
             SET musicbrainz_release_group_id = ?1,
                 metadata_status = 'resolved',
                 metadata_checked_at = ?2,
                 metadata_error = NULL,
                 updated_at = ?2
             WHERE id = ?3",
            params![musicbrainz_release_group_id, now, album_id],
        )?;
        Ok(())
    }

    pub fn mark_album_metadata_checked(
        &self,
        album_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE albums
             SET metadata_status = ?1,
                 metadata_checked_at = ?2,
                 metadata_error = ?3,
                 updated_at = ?2
             WHERE id = ?4",
            params![status, now, error, album_id],
        )?;
        Ok(())
    }

    /// Persist a Discogs artist id on an existing artist row. Mirrors
    /// [`Self::resolve_artist_musicbrainz_id`] but for the Discogs side;
    /// does NOT touch `musicbrainz_id`. Sets `metadata_status =
    /// 'resolved'` so the activity panel reflects forward progress;
    /// downstream jobs (`fetch_artist_discogs_profile`,
    /// `fetch_artist_discogs_releases`) can be enqueued safely on top
    /// of this.
    pub fn set_artist_discogs_id(&self, artist_id: i64, discogs_id: &str) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE artists
             SET discogs_id = ?1,
                 metadata_status = 'resolved',
                 metadata_checked_at = ?2,
                 metadata_error = NULL,
                 updated_at = ?2
             WHERE id = ?3",
            params![discogs_id, now, artist_id],
        )?;
        Ok(())
    }

    /// Persist a Discogs master id on an existing album row. Does NOT
    /// touch `musicbrainz_release_group_id`.
    pub fn set_album_discogs_master_id(
        &self,
        album_id: i64,
        discogs_master_id: &str,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE albums
             SET discogs_master_id = ?1,
                 metadata_status = 'resolved',
                 metadata_checked_at = ?2,
                 metadata_error = NULL,
                 updated_at = ?2
             WHERE id = ?3",
            params![discogs_master_id, now, album_id],
        )?;
        Ok(())
    }

    /// Attach a cover asset to a single `discography_items` row by id.
    /// Used by `fetch_discogs_thumb` when the per-row thumb has been
    /// downloaded into an asset.
    pub fn set_discography_item_cover(&self, item_id: i64, cover_asset_id: i64) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE discography_items
             SET cover_asset_id = ?1, updated_at = ?2
             WHERE id = ?3",
            params![cover_asset_id, now, item_id],
        )?;
        Ok(())
    }

    /// Returns the source URL to fetch + the destination row id the
    /// downloaded asset should be written into. The thumb worker uses
    /// `entity_id = discography_items.id`; if the row already has a
    /// cover or is missing a thumb URL, this returns `None` and the
    /// worker treats that as a no-op success.
    pub fn load_discogs_thumb_target(&self, item_id: i64) -> Result<Option<DiscogsThumbTarget>> {
        let connection = self.lock_connection()?;
        connection
            .query_row(
                "SELECT id, thumb_url FROM discography_items
                 WHERE id = ?1
                   AND thumb_url IS NOT NULL
                   AND thumb_url <> ''
                   AND cover_asset_id IS NULL",
                params![item_id],
                |row| {
                    Ok(DiscogsThumbTarget {
                        item_id: row.get(0)?,
                        source_url: row.get(1)?,
                    })
                },
            )
            .optional()
            .context("failed to load discogs thumb target")
    }

    pub fn save_album_cover(&self, album_id: i64, cover_asset_id: i64) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE albums
             SET cover_asset_id = ?1,
                 metadata_status = 'resolved',
                 metadata_checked_at = ?2,
                 metadata_error = NULL,
                 updated_at = ?2
             WHERE id = ?3",
            params![cover_asset_id, now, album_id],
        )?;
        connection.execute(
            "UPDATE discography_items
             SET cover_asset_id = ?1, updated_at = ?2
             WHERE musicbrainz_release_group_id = (
                SELECT musicbrainz_release_group_id FROM albums WHERE id = ?3
             )",
            params![cover_asset_id, now, album_id],
        )?;
        Ok(())
    }

    /// Mirror of [`save_artist_profile`] for albums: persists the
    /// optional TheAudioDB album id and description on the row, tags
    /// `description_source` as `'theaudiodb'`, and clears any prior
    /// metadata error. Note that this does NOT bump
    /// `albums.metadata_status`; that column tracks identity (cover /
    /// release-group resolution) and is independent of description
    /// availability.
    pub fn save_album_profile(
        &self,
        album_id: i64,
        audiodb_id: Option<&str>,
        description: Option<&str>,
        description_source: Option<&str>,
    ) -> Result<()> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        connection.execute(
            "UPDATE albums
             SET audiodb_id = COALESCE(?1, audiodb_id),
                 description = COALESCE(?2, description),
                 description_source = CASE
                     WHEN ?2 IS NULL THEN description_source
                     WHEN ?4 IS NULL THEN description_source
                     ELSE ?4
                 END,
                 updated_at = ?3
             WHERE id = ?5",
            params![audiodb_id, description, now, description_source, album_id],
        )?;
        Ok(())
    }

    pub fn save_album_cover_file(
        &self,
        album_id: i64,
        source_url: &str,
        mime_type: Option<&str>,
        data: &[u8],
    ) -> Result<PathBuf> {
        let connection = self.lock_connection()?;
        let album_dir = connection
            .query_row(
                "SELECT files.path
                 FROM tracks
                 JOIN files ON files.id = tracks.file_id
                 WHERE tracks.album_id = ?1 AND files.status = 'present'
                 ORDER BY files.path
                 LIMIT 1",
                params![album_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(PathBuf::from)
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .context("album has no local folder for cover art")?;

        let extension = artwork_extension(mime_type, data);
        let cover_path = available_cover_path(&album_dir, extension);
        fs::write(&cover_path, data)
            .with_context(|| format!("failed to write album cover to {}", cover_path.display()))?;

        let now = now_millis();
        let hash = fnv1a_hex(data);
        let cover_path_label = cover_path.display().to_string();
        connection.execute(
            "INSERT INTO assets(kind, source, source_url, cache_path, content_hash, mime_type, status, fetched_at)
             VALUES('album_art', 'coverartarchive', ?1, ?2, ?3, ?4, 'ready', ?5)
             ON CONFLICT(cache_path) DO UPDATE SET
                source_url = COALESCE(excluded.source_url, assets.source_url),
                content_hash = excluded.content_hash,
                mime_type = excluded.mime_type,
                status = 'ready',
                error = NULL,
                fetched_at = excluded.fetched_at",
            params![source_url, cover_path_label, hash, mime_type, now],
        )?;
        let asset_id = select_id_by_text(&connection, "assets", "cache_path", &cover_path_label)?;
        drop(connection);
        self.save_album_cover(album_id, asset_id)?;
        Ok(cover_path)
    }

    pub fn save_external_asset(
        &self,
        kind: &str,
        source: &str,
        source_url: &str,
        mime_type: Option<&str>,
        data: &[u8],
    ) -> Result<i64> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        let hash = fnv1a_hex(data);
        let extension = artwork_extension(mime_type, data);
        let asset_dir = self.cache_dir.join("external-artwork").join(source);
        fs::create_dir_all(&asset_dir)?;
        let cache_path = asset_dir.join(format!("{hash}.{extension}"));
        if !cache_path.exists() {
            fs::write(&cache_path, data)?;
        }

        let cache_path_label = cache_path.display().to_string();
        connection.execute(
            "INSERT INTO assets(kind, source, source_url, cache_path, content_hash, mime_type, status, fetched_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'ready', ?7)
             ON CONFLICT(cache_path) DO UPDATE SET
                source_url = COALESCE(excluded.source_url, assets.source_url),
                content_hash = excluded.content_hash,
                mime_type = excluded.mime_type,
                status = 'ready',
                error = NULL,
                fetched_at = excluded.fetched_at",
            params![
                kind,
                source,
                source_url,
                cache_path_label,
                hash,
                mime_type,
                now,
            ],
        )?;
        select_id_by_text(&connection, "assets", "cache_path", &cache_path_label)
    }

    pub fn upsert_discography_item(
        &self,
        artist_id: i64,
        title: &str,
        year: Option<&str>,
        release_type: &str,
        musicbrainz_release_group_id: Option<&str>,
    ) -> Result<i64> {
        // Backwards-compatible thin wrapper. The legacy 5-arg call
        // signature is preserved for the MusicBrainz-based discography
        // job; new code uses [`upsert_discography_item_full`] to set
        // `role`, `format`, `source`, `discogs_master_id`, and
        // `thumb_url`.
        self.upsert_discography_item_full(
            artist_id,
            title,
            year,
            release_type,
            musicbrainz_release_group_id,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Extended upsert that lets callers populate `role`, `format`,
    /// `source`, `discogs_master_id`, and `thumb_url`. `role` defaults
    /// to `"Main"`, `source` defaults to `"musicbrainz"` to preserve
    /// the prior behaviour. `thumb_url` is preserved on conflict via
    /// COALESCE so a later MB pass doesn't clobber a Discogs-sourced
    /// thumb URL.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_discography_item_full(
        &self,
        artist_id: i64,
        title: &str,
        year: Option<&str>,
        release_type: &str,
        musicbrainz_release_group_id: Option<&str>,
        discogs_master_id: Option<&str>,
        role: Option<&str>,
        format: Option<&str>,
        source: Option<&str>,
        thumb_url: Option<&str>,
    ) -> Result<i64> {
        let connection = self.lock_connection()?;
        let now = now_millis();
        let normalized_title = normalize_key(title);
        let sort_key = discography_sort_key(year, title);
        let role = role.unwrap_or("Main");
        let source = source.unwrap_or("musicbrainz");
        connection.execute(
            "INSERT INTO discography_items(
                artist_id, title, normalized_title, year, release_type,
                musicbrainz_release_group_id, discogs_master_id,
                role, format, source, thumb_url,
                sort_key, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
             ON CONFLICT(artist_id, normalized_title, release_type, role) DO UPDATE SET
                title = excluded.title,
                year = COALESCE(
                    CASE
                        WHEN discography_items.year IS NULL THEN excluded.year
                        WHEN excluded.year IS NULL THEN discography_items.year
                        WHEN excluded.year < discography_items.year THEN excluded.year
                        ELSE discography_items.year
                    END,
                    discography_items.year
                ),
                musicbrainz_release_group_id = COALESCE(
                    excluded.musicbrainz_release_group_id,
                    discography_items.musicbrainz_release_group_id
                ),
                discogs_master_id = COALESCE(
                    excluded.discogs_master_id,
                    discography_items.discogs_master_id
                ),
                format = COALESCE(excluded.format, discography_items.format),
                thumb_url = COALESCE(excluded.thumb_url, discography_items.thumb_url),
                source = CASE
                    WHEN discography_items.source = excluded.source THEN discography_items.source
                    ELSE 'musicbrainz+discogs'
                END,
                sort_key = excluded.sort_key,
                updated_at = excluded.updated_at",
            params![
                artist_id,
                title,
                normalized_title,
                year,
                release_type,
                musicbrainz_release_group_id,
                discogs_master_id,
                role,
                format,
                source,
                thumb_url,
                sort_key,
                now,
            ],
        )?;
        connection
            .query_row(
                "SELECT id FROM discography_items
                 WHERE artist_id = ?1 AND normalized_title = ?2
                   AND release_type = ?3 AND role = ?4",
                params![artist_id, normalized_title, release_type, role],
                |row| row.get(0),
            )
            .context("failed to select discography item id")
    }

    pub fn load_discography(&self, artist_id: i64) -> Result<Vec<CatalogDiscographyItem>> {
        let connection = self.lock_connection()?;
        let mut statement = connection.prepare_cached(
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
                    tracks.artist_name, tracks.album_name, tracks.genre, tracks.track_number, tracks.year,
                    tracks.date_added, tracks.duration_ms, tracks.codec, tracks.bitrate,
                    tracks.file_size, tracks.play_count, tracks.liked, tracks.artwork_path
                 FROM tracks
                 JOIN files ON files.id = tracks.file_id
                 WHERE files.path = ?1 AND files.status = 'present'",
                params![path.display().to_string()],
                |row| {
                    let path: String = row.get(4)?;
                    let track_number: Option<i64> = row.get(9)?;
                    let date_added_ms: i64 = row.get(11)?;
                    let duration_ms: i64 = row.get(12)?;
                    let bitrate: Option<i64> = row.get(14)?;
                    let file_size: i64 = row.get(15)?;
                    let play_count: i64 = row.get(16)?;
                    let liked: i64 = row.get(17)?;
                    let artwork_path: Option<String> = row.get(18)?;
                    Ok(CatalogTrack {
                        track_id: row.get(0)?,
                        file_id: row.get(1)?,
                        artist_id: row.get(2)?,
                        album_id: row.get(3)?,
                        path: PathBuf::from(path),
                        title: row.get(5)?,
                        artist: row.get(6)?,
                        album: row.get(7)?,
                        genre: row.get(8)?,
                        track_number: track_number.map(|track_number| track_number as u32),
                        year: row.get(10)?,
                        date_added: millis_to_system_time(date_added_ms),
                        duration: Duration::from_millis(duration_ms.max(0) as u64),
                        codec: row.get(13)?,
                        bitrate: bitrate.map(|bitrate| bitrate as u32),
                        file_size: file_size.max(0) as u64,
                        play_count: play_count.max(0) as u32,
                        liked: liked != 0,
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
            album_keys: HashSet<String>,
            track_count: usize,
        }

        // SQL-side GROUP BY collapses per-track rows down to one row
        // per `(artist_id, album_name, track_artist_credit)` tuple. For
        // typical libraries (many tracks share the same primary artist
        // and album), the returned row count drops by 5-20x compared
        // to the previous per-track query, which used to fan out N rows
        // and aggregate everything in Rust.
        //
        // The `track_artist_credit` (raw `tracks.artist_name` string) is
        // kept in the GROUP BY because we still need to split featured-
        // artist credits in Rust -- "A$AP Rocky feat/ Skepta" produces
        // entries for both "A$AP Rocky" and "Skepta". Once a proper
        // `track_artist_credits` join table exists we can drop this and
        // group purely by (artists.id, albums.id).
        let connection = self.lock_connection()?;
        let (root_filter, root_params) = root_path_filter(roots);
        // `MAX(COALESCE(cover, art))` is used to pick *any* available
        // album cover / track artwork as a fallback for artists without
        // a dedicated photo asset, mirroring the prior per-row
        // `COALESCE(cover_assets.cache_path, tracks.artwork_path)`
        // behaviour but without the per-track fan-out.
        let mut statement = connection.prepare_cached(&format!(
            "SELECT
                artists.id,
                artists.name,
                artists.bio,
                photo_assets.cache_path,
                MAX(COALESCE(cover_assets.cache_path, tracks.artwork_path)) AS art_fallback,
                tracks.album_name,
                tracks.artist_name,
                COUNT(*) AS track_count
             FROM artists
             JOIN tracks ON tracks.artist_id = artists.id
             JOIN files ON files.id = tracks.file_id
             JOIN albums ON albums.id = tracks.album_id
             LEFT JOIN assets AS photo_assets ON photo_assets.id = artists.photo_asset_id
             LEFT JOIN assets AS cover_assets ON cover_assets.id = albums.cover_asset_id
             WHERE files.status = 'present' AND ({root_filter})
             GROUP BY artists.id, tracks.album_name, tracks.artist_name
             ORDER BY artists.normalized_name COLLATE NOCASE, tracks.album_name COLLATE NOCASE",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let photo_path: Option<String> = row.get(3)?;
            let fallback_photo_path: Option<String> = row.get(4)?;
            let track_count: i64 = row.get(7)?;
            Ok((
                row.get::<_, i64>(0)?,            // artist_id
                row.get::<_, String>(1)?,         // name
                row.get::<_, Option<String>>(2)?, // bio
                photo_path.or(fallback_photo_path).map(PathBuf::from),
                row.get::<_, String>(5)?, // album_name
                row.get::<_, String>(6)?, // track_artist
                track_count.max(0) as usize,
            ))
        })?;

        let mut artists = HashMap::<String, ArtistAggregate>::new();
        for row in rows.filter_map(|row| row.ok()) {
            let (artist_id, name, bio, photo_path, album_name, track_artist, track_count) = row;
            let album_key = normalize_key(&album_name);
            for artist_name in individual_artist_names(&track_artist) {
                let key = normalize_key(&artist_name);
                let aggregate = artists.entry(key).or_insert_with(|| ArtistAggregate {
                    artist_id: synthetic_artist_id(&artist_name),
                    name: artist_name.clone(),
                    bio: None,
                    photo_path: None,
                    album_keys: HashSet::new(),
                    track_count: 0,
                });

                if normalize_key(&name) == normalize_key(&aggregate.name) {
                    aggregate.artist_id = artist_id;
                    aggregate.name = name.clone();
                }
                if aggregate.bio.is_none() {
                    aggregate.bio = bio.clone();
                }
                if aggregate.photo_path.is_none() {
                    aggregate.photo_path = photo_path.clone();
                }
                aggregate.album_keys.insert(album_key.clone());
                aggregate.track_count += track_count;
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
        // Final sort uses the normalized lowercase name to match what
        // `ORDER BY ... COLLATE NOCASE` produced from SQL; the synthetic
        // (featured-artist) entries don't have a SQL position so we
        // re-sort the merged result.
        artists.sort_by_key(|left| left.name.to_lowercase());
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
            description: Option<String>,
            description_state: AlbumDescriptionState,
        }

        // SQL-side aggregation: one row per
        // `(albums.id, tracks.artist_name)` instead of one per track.
        // The remaining featured-credit dedup is done by
        // `primary_artist_name()` in Rust on the much smaller result
        // set. `MAX(COALESCE(cover, art))` mirrors the prior per-row
        // fallback for albums without a dedicated cover asset.
        // `albums.description` is also one-per-album, so we just MAX it
        // to keep the GROUP BY satisfied. The description-state lookup
        // joins the latest `metadata_jobs` row (job_type =
        // 'fetch_album_profile' or 'fetch_album_wikipedia_summary') so
        // we can tell "tried and failed" apart from "not yet checked".
        let connection = self.lock_connection()?;
        let (root_filter, root_params) = root_path_filter(roots);
        let mut statement = connection.prepare_cached(&format!(
            "SELECT
                albums.id,
                albums.artist_id,
                albums.title,
                tracks.artist_name,
                albums.year,
                MAX(COALESCE(cover_assets.cache_path, tracks.artwork_path)) AS artwork_path,
                COUNT(*) AS track_count,
                MAX(albums.description) AS description,
                MAX(albums.metadata_status) AS metadata_status,
                MAX(profile_job.error_kind) AS profile_error_kind,
                MAX(profile_job.status) AS profile_job_status
             FROM albums
             JOIN tracks ON tracks.album_id = albums.id
             JOIN files ON files.id = tracks.file_id
             LEFT JOIN assets AS cover_assets ON cover_assets.id = albums.cover_asset_id
             LEFT JOIN metadata_jobs AS profile_job
                ON profile_job.entity_type = 'album'
                AND profile_job.entity_id = albums.id
                AND profile_job.job_type = 'fetch_album_profile'
             WHERE files.status = 'present' AND ({root_filter})
             GROUP BY albums.id, tracks.artist_name
             ORDER BY tracks.artist_name COLLATE NOCASE, albums.year, albums.normalized_title",
        ))?;
        let rows = statement.query_map(params_from_iter(root_params), |row| {
            let artwork_path: Option<String> = row.get(5)?;
            let track_count: i64 = row.get(6)?;
            Ok((
                row.get::<_, i64>(0)?,            // album_id
                row.get::<_, i64>(1)?,            // artist_id
                row.get::<_, String>(2)?,         // title
                row.get::<_, String>(3)?,         // track_artist
                row.get::<_, Option<String>>(4)?, // year
                artwork_path.map(PathBuf::from),
                track_count.max(0) as usize,
                row.get::<_, Option<String>>(7)?,  // description
                row.get::<_, Option<String>>(8)?,  // metadata_status
                row.get::<_, Option<String>>(9)?,  // profile_error_kind
                row.get::<_, Option<String>>(10)?, // profile_job_status
            ))
        })?;

        let mut albums = HashMap::<String, AlbumAggregate>::new();
        for row in rows.filter_map(|row| row.ok()) {
            let (
                album_id,
                artist_id,
                title,
                track_artist,
                year,
                artwork_path,
                track_count,
                description,
                metadata_status,
                profile_error_kind,
                profile_job_status,
            ) = row;
            let primary_artist = primary_artist_name(&track_artist);
            let key = format!(
                "{}:{}",
                normalize_key(&primary_artist),
                normalize_key(&title)
            );
            let description_state = derive_album_description_state(
                description.as_deref(),
                metadata_status.as_deref(),
                profile_error_kind.as_deref(),
                profile_job_status.as_deref(),
            );
            let aggregate = albums.entry(key).or_insert_with(|| AlbumAggregate {
                album_id,
                artist_id,
                title,
                artist: primary_artist,
                year,
                artwork_path: None,
                track_count: 0,
                description: None,
                description_state: AlbumDescriptionState::Pending,
            });

            if aggregate.artwork_path.is_none() {
                aggregate.artwork_path = artwork_path;
            }
            aggregate.track_count += track_count;
            if aggregate.description.is_none() {
                aggregate.description = description;
            }
            // Promote description_state to the most informative variant
            // we've seen across the GROUP BY (Available > Unavailable >
            // Pending).
            aggregate.description_state =
                merge_description_state(aggregate.description_state, description_state);
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
                description: album.description,
                description_state: album.description_state,
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
}

/// Free-standing version of artwork persistence so it can be invoked
/// from the static `upsert_track_in_transaction` helper. Functionally
/// identical to the original method; the only change is taking
/// `cache_dir: &Path` explicitly instead of capturing `self.cache_dir`.
fn persist_artwork(
    connection: &Connection,
    cache_dir: &Path,
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
            let artwork_dir = cache_dir.join("artwork");
            fs::create_dir_all(&artwork_dir)?;
            let cache_path = artwork_dir.join(format!("{hash}.{extension}"));
            if !cache_path.exists() {
                fs::write(&cache_path, data)?;
            }

            let cache_path_label = cache_path.display().to_string();
            connection
                .prepare_cached(
                    "INSERT INTO assets(kind, source, cache_path, content_hash, mime_type, status, fetched_at)
                     VALUES('album_art', 'embedded', ?1, ?2, ?3, 'ready', ?4)
                     ON CONFLICT(cache_path) DO UPDATE SET
                        content_hash = excluded.content_hash,
                        mime_type = excluded.mime_type,
                        status = 'ready',
                        fetched_at = excluded.fetched_at",
                )?
                .execute(params![cache_path_label, hash, mime_type.as_deref(), now])?;
            let asset_id =
                select_id_by_text(connection, "assets", "cache_path", &cache_path_label)?;
            Ok((Some(asset_id), Some(cache_path)))
        }
    }
}

fn upsert_artist(connection: &Connection, name: &str, now: i64) -> Result<i64> {
    let normalized = normalize_key(name);
    connection
        .prepare_cached(
            "INSERT INTO artists(name, normalized_name, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?3)
             ON CONFLICT(normalized_name) DO UPDATE SET
                name = excluded.name,
                updated_at = excluded.updated_at",
        )?
        .execute(params![name, normalized, now])?;
    connection
        .prepare_cached("SELECT id FROM artists WHERE normalized_name = ?1")?
        .query_row(params![normalized], |row| row.get(0))
        .context("failed to select artist id")
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
    connection
        .prepare_cached(
            "INSERT INTO albums(title, normalized_title, artist_id, artist_name, year, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(normalized_title, artist_id) DO UPDATE SET
                title = excluded.title,
                artist_name = excluded.artist_name,
                year = COALESCE(excluded.year, albums.year),
                updated_at = excluded.updated_at",
        )?
        .execute(params![title, normalized, artist_id, artist_name, year, now])?;

    connection
        .prepare_cached("SELECT id FROM albums WHERE normalized_title = ?1 AND artist_id = ?2")?
        .query_row(params![normalized, artist_id], |row| row.get(0))
        .context("failed to select album id")
}

fn enqueue_metadata_job(
    connection: &Connection,
    entity_type: &str,
    entity_id: i64,
    job_type: &str,
    now: i64,
) -> Result<()> {
    enqueue_metadata_job_at(connection, entity_type, entity_id, job_type, now, now)
}

fn enqueue_metadata_job_at(
    connection: &Connection,
    entity_type: &str,
    entity_id: i64,
    job_type: &str,
    next_attempt_at: i64,
    now: i64,
) -> Result<()> {
    connection
        .prepare_cached(
            "INSERT INTO metadata_jobs(
                entity_type, entity_id, job_type, status, next_attempt_at, created_at, updated_at
             ) VALUES(?1, ?2, ?3, 'pending', ?4, ?5, ?5)
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
        )?
        .execute(params![entity_type, entity_id, job_type, next_attempt_at, now])?;
    Ok(())
}

fn enqueue_missing_album_art_jobs(
    connection: &Connection,
    artist_id: Option<i64>,
    now: i64,
) -> Result<usize> {
    let mut sql = String::from(
        "SELECT albums.id, albums.musicbrainz_release_group_id, artists.musicbrainz_id
         FROM albums
         JOIN artists ON artists.id = albums.artist_id
         WHERE albums.cover_asset_id IS NULL
           AND NOT EXISTS (
              SELECT 1 FROM tracks
              WHERE tracks.album_id = albums.id
                AND tracks.artwork_path IS NOT NULL
                AND tracks.artwork_path <> ''
           )",
    );
    if artist_id.is_some() {
        sql.push_str(" AND albums.artist_id = ?1");
    }
    sql.push_str(" ORDER BY artists.name, albums.title");

    let album_rows = {
        let mut statement = connection.prepare_cached(&sql)?;
        if let Some(artist_id) = artist_id {
            let rows = statement.query_map(params![artist_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        } else {
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        }
    };

    let mut enqueued = 0usize;
    for (album_id, release_group_id, artist_musicbrainz_id) in album_rows {
        match (release_group_id, artist_musicbrainz_id) {
            (Some(_), _) => {
                // Tier-0 Lidarr first; the legacy CAA cover job stays
                // as fallback.
                enqueue_metadata_job(connection, "album", album_id, "fetch_album_lidarr", now)?;
                enqueued += 1;
                enqueue_metadata_job(connection, "album", album_id, "fetch_album_cover", now)?;
                enqueued += 1;
            }
            (None, Some(_)) => {
                enqueue_metadata_job(
                    connection,
                    "album",
                    album_id,
                    "resolve_album_musicbrainz",
                    now,
                )?;
                enqueued += 1;
            }
            (None, None) => {}
        }
    }

    Ok(enqueued)
}

/// Companion backfill for album descriptions. Enqueues
/// `fetch_album_profile` for any album that has a release-group MBID
/// but no description yet. Albums without an MBID are handled by
/// [`enqueue_missing_album_art_jobs`], which enqueues
/// `resolve_album_musicbrainz`; once the MBID lands, the worker chains
/// `fetch_album_profile` automatically.
fn enqueue_missing_album_description_jobs(
    connection: &Connection,
    artist_id: Option<i64>,
    now: i64,
) -> Result<usize> {
    let mut sql = String::from(
        "SELECT albums.id
         FROM albums
         WHERE albums.musicbrainz_release_group_id IS NOT NULL
           AND (albums.description IS NULL OR albums.description = '')",
    );
    if artist_id.is_some() {
        sql.push_str(" AND albums.artist_id = ?1");
    }
    sql.push_str(" ORDER BY albums.title");

    let album_ids = {
        let mut statement = connection.prepare_cached(&sql)?;
        if let Some(artist_id) = artist_id {
            let rows = statement.query_map(params![artist_id], |row| row.get::<_, i64>(0))?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        } else {
            let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
            rows.filter_map(|row| row.ok()).collect::<Vec<_>>()
        }
    };

    let mut enqueued = 0usize;
    for album_id in album_ids {
        // Tier-0 Lidarr first; legacy TADb album profile stays as fallback.
        enqueue_metadata_job(connection, "album", album_id, "fetch_album_lidarr", now)?;
        enqueued += 1;
        enqueue_metadata_job(connection, "album", album_id, "fetch_album_profile", now)?;
        enqueued += 1;
    }
    Ok(enqueued)
}

fn available_cover_path(album_dir: &Path, extension: &str) -> PathBuf {
    let preferred = album_dir.join(format!("cover.{extension}"));
    if !preferred.exists() {
        return preferred;
    }

    for ix in 1..1000 {
        let candidate = album_dir.join(format!("cover-tempo-{ix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }

    album_dir.join(format!("cover-tempo.{extension}"))
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
    // Statement cache key is the SQL string, so the same `(table, column)`
    // tuple will reuse a single prepared statement across batches.
    connection
        .prepare_cached(&sql)?
        .query_row(params![value], |row| row.get(0))
        .with_context(|| format!("failed to select id from {table}"))
}

fn select_id_by_i64(connection: &Connection, table: &str, column: &str, value: i64) -> Result<i64> {
    let sql = format!("SELECT id FROM {table} WHERE {column} = ?1");
    connection
        .prepare_cached(&sql)?
        .query_row(params![value], |row| row.get(0))
        .with_context(|| format!("failed to select id from {table}"))
}

/// Reduce the persisted `(metadata_status, error_kind, job_status)`
/// triple to the tri-state the album hero needs.
///
/// - "Available": we have a description string in hand.
/// - "Unavailable": every viable source has been exhausted (the
///   `fetch_album_profile` job is `complete` AND the album row's
///   metadata_status reports a terminal `missing`/`no_match`-style
///   outcome). UI should show "No online description available."
/// - "Pending": fall-through. Either online metadata is disabled, the
///   job is still queued, or it has retried with `pending`/`running`
///   status.
fn derive_album_description_state(
    description: Option<&str>,
    metadata_status: Option<&str>,
    profile_error_kind: Option<&str>,
    profile_job_status: Option<&str>,
) -> AlbumDescriptionState {
    if description.is_some_and(|d| !d.trim().is_empty()) {
        return AlbumDescriptionState::Available;
    }

    let job_complete = matches!(profile_job_status, Some("complete"));
    let exhausted_via_status = matches!(metadata_status, Some("missing"));
    let exhausted_via_kind = matches!(profile_error_kind, Some("no_match"));

    if (job_complete && exhausted_via_status) || exhausted_via_kind {
        return AlbumDescriptionState::Unavailable;
    }

    AlbumDescriptionState::Pending
}

fn merge_description_state(
    current: AlbumDescriptionState,
    incoming: AlbumDescriptionState,
) -> AlbumDescriptionState {
    match (current, incoming) {
        (AlbumDescriptionState::Available, _) | (_, AlbumDescriptionState::Available) => {
            AlbumDescriptionState::Available
        }
        (AlbumDescriptionState::Unavailable, _) | (_, AlbumDescriptionState::Unavailable) => {
            AlbumDescriptionState::Unavailable
        }
        _ => AlbumDescriptionState::Pending,
    }
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare_cached(&pragma)?;
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

/// One-shot rebuild of `discography_items`. Two pre-v6 shapes need
/// fixing:
///
/// 1. Pre-v5 tables had a 3-column `UNIQUE(artist_id, normalized_title,
///    release_type)` that doesn't admit per-role rows.
/// 2. v5-shipped tables had column-level `UNIQUE` on
///    `musicbrainz_release_group_id` and `discogs_master_id`. With
///    `role` now part of the natural key, the same MBID can legitimately
///    appear in more than one row across artists; the column-level
///    UNIQUE rejected those upserts and surfaced as the
///    "UNIQUE constraint failed: discography_items.musicbrainz_release_group_id"
///    we observed in the wild.
///
/// The v6 shape:
/// - `UNIQUE(artist_id, normalized_title, release_type, role)`
/// - `UNIQUE(artist_id, musicbrainz_release_group_id)` (table-level,
///   admits NULLs)
/// - Partial unique index on `discogs_master_id` (per artist scope is
///   not enforced; Discogs ids are globally unique).
///
/// SQLite can't drop column-level UNIQUE in place, so we rebuild the
/// table when either pre-v6 shape is detected. Idempotent on already-v6
/// tables.
fn rebuild_discography_items_unique(connection: &Connection) -> Result<()> {
    let mut indexes_stmt =
        connection.prepare("SELECT name, [unique] FROM pragma_index_list('discography_items')")?;
    let unique_index_names: Vec<String> = indexes_stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let is_unique: i64 = row.get(1)?;
            Ok((name, is_unique))
        })?
        .filter_map(|row| row.ok())
        .filter(|(_, unique)| *unique != 0)
        .map(|(name, _)| name)
        .collect();

    let mut has_legacy_3col_unique = false;
    let mut has_role_unique = false;
    let mut has_artist_mbid_unique = false;
    let mut has_column_mbid_unique = false;
    let mut has_column_discogs_unique = false;
    for index_name in &unique_index_names {
        let mut info_stmt = connection.prepare(&format!(
            "SELECT name FROM pragma_index_info({})",
            quote_sql_string(index_name)
        ))?;
        let columns: Vec<String> = info_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|row| row.ok())
            .collect();
        match columns.as_slice() {
            cols if cols.len() == 3
                && cols[0] == "artist_id"
                && cols[1] == "normalized_title"
                && cols[2] == "release_type" =>
            {
                has_legacy_3col_unique = true;
            }
            cols if cols.len() == 4
                && cols[0] == "artist_id"
                && cols[1] == "normalized_title"
                && cols[2] == "release_type"
                && cols[3] == "role" =>
            {
                has_role_unique = true;
            }
            cols if cols.len() == 2
                && cols[0] == "artist_id"
                && cols[1] == "musicbrainz_release_group_id" =>
            {
                has_artist_mbid_unique = true;
            }
            cols if cols.len() == 1 && cols[0] == "musicbrainz_release_group_id" => {
                // The auto-index for the column-level UNIQUE is named
                // `sqlite_autoindex_*`; the named partial index we
                // create separately doesn't show in pragma_index_list
                // with a single column at this point. Treat any
                // single-column unique on this field as the bad one.
                has_column_mbid_unique = true;
            }
            cols if cols.len() == 1 && cols[0] == "discogs_master_id" => {
                // Same treatment for discogs_master_id; we'll rebuild
                // even if the user-defined partial index exists, since
                // the column-level UNIQUE that came with v5 is what we
                // need to drop.
                has_column_discogs_unique = true;
            }
            _ => {}
        }
    }
    drop(indexes_stmt);

    // The user-defined partial unique index on `discogs_master_id` (
    // `discography_items_discogs_master_idx`) shows up as a 1-column
    // UNIQUE too. Distinguish it from the column-level constraint by
    // inspecting `sql` from `sqlite_master` -- the auto-index has no
    // SQL stored.
    let column_mbid_is_auto = if has_column_mbid_unique {
        connection
            .query_row(
                "SELECT 1 FROM sqlite_master
                 WHERE type = 'index'
                   AND tbl_name = 'discography_items'
                   AND name LIKE 'sqlite_autoindex_%'
                   AND sql IS NULL
                   AND name IN (SELECT name FROM pragma_index_list('discography_items')
                                WHERE [unique] = 1)
                   AND EXISTS (SELECT 1 FROM pragma_index_info(name)
                               WHERE name = 'musicbrainz_release_group_id')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
    } else {
        false
    };
    let column_discogs_is_auto = if has_column_discogs_unique {
        connection
            .query_row(
                "SELECT 1 FROM sqlite_master
                 WHERE type = 'index'
                   AND tbl_name = 'discography_items'
                   AND name LIKE 'sqlite_autoindex_%'
                   AND sql IS NULL
                   AND name IN (SELECT name FROM pragma_index_list('discography_items')
                                WHERE [unique] = 1)
                   AND EXISTS (SELECT 1 FROM pragma_index_info(name)
                               WHERE name = 'discogs_master_id')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some()
    } else {
        false
    };

    let needs_rebuild = has_legacy_3col_unique
        || !has_role_unique
        || column_mbid_is_auto
        || column_discogs_is_auto
        || !has_artist_mbid_unique;

    if !needs_rebuild {
        return Ok(());
    }

    connection.execute_batch(
        "PRAGMA foreign_keys = OFF;
         BEGIN;

         CREATE TABLE discography_items_new (
            id INTEGER PRIMARY KEY,
            artist_id INTEGER NOT NULL REFERENCES artists(id),
            title TEXT NOT NULL,
            normalized_title TEXT NOT NULL,
            year TEXT,
            release_type TEXT NOT NULL,
            musicbrainz_release_group_id TEXT,
            discogs_master_id TEXT,
            cover_asset_id INTEGER REFERENCES assets(id),
            local_album_id INTEGER REFERENCES albums(id),
            is_local INTEGER NOT NULL DEFAULT 0,
            source TEXT NOT NULL DEFAULT 'musicbrainz',
            role TEXT NOT NULL DEFAULT 'Main',
            format TEXT,
            thumb_url TEXT,
            sort_key TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            UNIQUE(artist_id, normalized_title, release_type, role),
            UNIQUE(artist_id, musicbrainz_release_group_id)
         );

         INSERT INTO discography_items_new(
            id, artist_id, title, normalized_title, year, release_type,
            musicbrainz_release_group_id, discogs_master_id, cover_asset_id,
            local_album_id, is_local, source, role, format, thumb_url,
            sort_key, created_at, updated_at
         )
         SELECT
            id, artist_id, title, normalized_title, year, release_type,
            musicbrainz_release_group_id,
            COALESCE(discogs_master_id, NULL),
            cover_asset_id,
            local_album_id, is_local,
            COALESCE(source, 'musicbrainz'),
            COALESCE(role, 'Main'),
            COALESCE(format, NULL),
            COALESCE(thumb_url, NULL),
            sort_key, created_at, updated_at
         FROM discography_items;

         DROP TABLE discography_items;
         ALTER TABLE discography_items_new RENAME TO discography_items;

         CREATE INDEX IF NOT EXISTS discography_artist_idx
            ON discography_items(artist_id, sort_key);
         CREATE UNIQUE INDEX IF NOT EXISTS discography_items_discogs_master_idx
            ON discography_items(discogs_master_id) WHERE discogs_master_id IS NOT NULL;

         COMMIT;
         PRAGMA foreign_keys = ON;",
    )?;
    perf::event(
        "catalog.migrate.rebuilt",
        "table=discography_items reason=v6_drop_column_unique",
    );
    Ok(())
}

/// SQL string literal escaper for identifiers passed via SQL string
/// (`pragma_index_info('name')`). Wraps the value in single quotes and
/// doubles any embedded single quotes.
fn quote_sql_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push('\'');
        }
        quoted.push(ch);
    }
    quoted.push('\'');
    quoted
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

    // First try a "feat./ft./featuring" split -- the part before the
    // marker is the primary credit, the rest is featured artists.
    let lower = artist.to_ascii_lowercase();
    if let Some((split_at, _)) = feature_artist_marker(&lower) {
        let primary = artist[..split_at].trim();
        if !primary.is_empty() {
            // Even the "primary" half can itself be a collaboration
            // (e.g. "John Lennon/Yoko Ono feat. The Plastic Ono Band"),
            // so recurse-by-cases through the collaboration split.
            return first_collaborator(primary).to_string();
        }
    }

    // No feat marker: check for a collaboration separator (slash or
    // semicolon). Pick the first listed name as the primary credit so
    // album/artist grouping is deterministic.
    first_collaborator(artist).to_string()
}

pub fn individual_artist_names(artist: &str) -> Vec<String> {
    let artist = artist.trim();
    if artist.is_empty() {
        return Vec::new();
    }

    let mut artists = Vec::new();
    let push_unique = |list: &mut Vec<String>, name: &str| {
        let name = name.trim();
        if !name.is_empty() && !list.iter().any(|existing| existing == name) {
            list.push(name.to_string());
        }
    };

    let lower = artist.to_ascii_lowercase();
    if let Some((split_at, marker_len)) = feature_artist_marker(&lower) {
        // The primary half before the feat marker can itself be a
        // slash/semicolon collaboration ("John Lennon/Yoko Ono feat. ..."),
        // so split it the same way as a pure collaboration string.
        let primary = artist[..split_at].trim();
        for collaborator in split_collaborators(primary) {
            push_unique(&mut artists, collaborator);
        }

        let featured = artist[split_at + marker_len..].trim_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch, '.' | '/' | ':' | '-' | '(' | '[')
        });
        for name in featured.split([',', ';']) {
            for name in name.split(" & ") {
                for name in name.split(" and ") {
                    push_unique(&mut artists, name);
                }
            }
        }
    } else {
        // No feat marker: split by collaboration separators only.
        for collaborator in split_collaborators(artist) {
            push_unique(&mut artists, collaborator);
        }
    }

    if artists.is_empty() {
        vec![artist.to_string()]
    } else {
        artists
    }
}

/// First name in a collaboration string. Used by `primary_artist_name`
/// when no feat-marker is present; for "John Lennon/Yoko Ono" returns
/// "John Lennon".
fn first_collaborator(artist: &str) -> &str {
    split_collaborators(artist).next().unwrap_or(artist).trim()
}

/// Split a credit string on collaboration separators -- slash (with or
/// without surrounding whitespace) and semicolon. Deliberately does NOT
/// split on ampersand: "Simon & Garfunkel" / "Hall & Oates" /
/// "Earth, Wind & Fire" are common single-entity band names where the
/// ampersand is part of the name, not a separator.
fn split_collaborators(artist: &str) -> impl Iterator<Item = &str> {
    artist
        .split(['/', '\\', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
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

fn path_in_roots(path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
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
    fn description_state_is_available_when_text_present() {
        assert_eq!(
            derive_album_description_state(Some("a great record"), None, None, None,),
            AlbumDescriptionState::Available,
        );
    }

    #[test]
    fn description_state_is_unavailable_when_complete_and_missing() {
        assert_eq!(
            derive_album_description_state(None, Some("missing"), None, Some("complete")),
            AlbumDescriptionState::Unavailable,
        );
    }

    #[test]
    fn description_state_is_unavailable_on_no_match() {
        assert_eq!(
            derive_album_description_state(None, None, Some("no_match"), None),
            AlbumDescriptionState::Unavailable,
        );
    }

    #[test]
    fn description_state_is_pending_when_in_flight() {
        assert_eq!(
            derive_album_description_state(None, Some("running"), None, Some("running")),
            AlbumDescriptionState::Pending,
        );
        assert_eq!(
            derive_album_description_state(None, None, None, None),
            AlbumDescriptionState::Pending,
        );
    }

    #[test]
    fn description_state_promotes_available_over_unavailable_on_merge() {
        assert_eq!(
            merge_description_state(
                AlbumDescriptionState::Unavailable,
                AlbumDescriptionState::Available,
            ),
            AlbumDescriptionState::Available,
        );
        assert_eq!(
            merge_description_state(
                AlbumDescriptionState::Available,
                AlbumDescriptionState::Pending,
            ),
            AlbumDescriptionState::Available,
        );
        assert_eq!(
            merge_description_state(
                AlbumDescriptionState::Pending,
                AlbumDescriptionState::Unavailable,
            ),
            AlbumDescriptionState::Unavailable,
        );
    }

    #[test]
    fn save_album_profile_round_trip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let now = now_millis();
        let (artist_id, album_id) = {
            let connection = store.lock_connection().unwrap();
            let artist_id = upsert_artist(&connection, "Brian Eno", now).unwrap();
            connection
                .execute(
                    "INSERT INTO albums(
                        title, normalized_title, artist_id, artist_name, year,
                        created_at, updated_at
                     ) VALUES('Another Green World', 'another green world', ?1, 'Brian Eno', '1975', ?2, ?2)",
                    params![artist_id, now],
                )
                .unwrap();
            let album_id: i64 = connection
                .query_row(
                    "SELECT id FROM albums WHERE artist_id = ?1",
                    params![artist_id],
                    |row| row.get(0),
                )
                .unwrap();
            (artist_id, album_id)
        };
        let _ = artist_id;

        store
            .save_album_profile(
                album_id,
                Some("audiodb-id-123"),
                Some("an ambient classic"),
                Some("theaudiodb"),
            )
            .unwrap();

        let connection = store.lock_connection().unwrap();
        let (audiodb_id, description, source): (Option<String>, Option<String>, Option<String>) =
            connection
                .query_row(
                    "SELECT audiodb_id, description, description_source FROM albums WHERE id = ?1",
                    params![album_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
        assert_eq!(audiodb_id.as_deref(), Some("audiodb-id-123"));
        assert_eq!(description.as_deref(), Some("an ambient classic"));
        assert_eq!(source.as_deref(), Some("theaudiodb"));
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
    fn splits_slash_separated_collaborations_into_individual_artists() {
        assert_eq!(
            individual_artist_names("John Lennon/Yoko Ono"),
            vec!["John Lennon", "Yoko Ono"]
        );
        assert_eq!(
            individual_artist_names("John Lennon / Yoko Ono"),
            vec!["John Lennon", "Yoko Ono"]
        );
        // Three-way collaborations.
        assert_eq!(
            individual_artist_names("Bowie/Eno/Visconti"),
            vec!["Bowie", "Eno", "Visconti"]
        );
        // Semicolons (Picard/MusicBrainz multi-value joiner).
        assert_eq!(
            individual_artist_names("Pärt; Hilliard Ensemble"),
            vec!["Pärt", "Hilliard Ensemble"]
        );
    }

    #[test]
    fn primary_artist_uses_first_listed_collaborator() {
        assert_eq!(primary_artist_name("John Lennon/Yoko Ono"), "John Lennon");
        assert_eq!(primary_artist_name("John Lennon / Yoko Ono"), "John Lennon");
    }

    #[test]
    fn slash_collaboration_with_feat_credit() {
        assert_eq!(
            individual_artist_names("John Lennon/Yoko Ono feat. The Plastic Ono Band"),
            vec!["John Lennon", "Yoko Ono", "The Plastic Ono Band"]
        );
        assert_eq!(
            primary_artist_name("John Lennon/Yoko Ono feat. The Plastic Ono Band"),
            "John Lennon"
        );
    }

    #[test]
    fn ampersand_band_names_are_not_split() {
        // Common false-positive separator: many real band names contain
        // " & " as part of the name, not as a collaboration marker.
        assert_eq!(
            individual_artist_names("Simon & Garfunkel"),
            vec!["Simon & Garfunkel"]
        );
        assert_eq!(
            individual_artist_names("Hall & Oates"),
            vec!["Hall & Oates"]
        );
        assert_eq!(
            primary_artist_name("Simon & Garfunkel"),
            "Simon & Garfunkel"
        );
    }

    #[test]
    fn detects_artwork_extension_from_magic_bytes() {
        assert_eq!(artwork_extension(None, b"\x89PNG\r\n\x1a\nrest"), "png");
        assert_eq!(artwork_extension(Some("image/jpeg"), b""), "jpg");
    }

    #[test]
    fn finish_scan_returns_missing_paths_for_scanned_roots_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        let other_root = temp_dir.path().join("other-library");
        let one = root.join("album").join("one.flac");
        let two = root.join("album").join("two.flac");
        let outside = other_root.join("outside.flac");
        let scan_id = store.begin_scan(std::slice::from_ref(&root)).unwrap();
        for path in [&one, &two, &outside] {
            store
                .upsert_track(
                    &Track {
                        path: path.clone(),
                        title: path.file_stem().unwrap().to_string_lossy().to_string(),
                        artist: "Alice".to_string(),
                        album: "First".to_string(),
                        genre: None,
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
                    Some(scan_id),
                )
                .unwrap();
        }
        store
            .finish_scan(scan_id, std::slice::from_ref(&root))
            .unwrap();

        let next_scan_id = store.begin_scan(std::slice::from_ref(&root)).unwrap();
        store
            .mark_paths_seen(next_scan_id, std::slice::from_ref(&one))
            .unwrap();

        let missing = store
            .finish_scan(next_scan_id, std::slice::from_ref(&root))
            .unwrap();

        assert_eq!(missing, vec![two.clone()]);
        assert_eq!(
            store
                .load_tracks(std::slice::from_ref(&root))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .load_tracks(std::slice::from_ref(&other_root))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn mark_folder_removed_returns_child_track_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        let album = root.join("album");
        let one = album.join("one.flac");
        let two = album.join("two.flac");
        let sibling = root.join("sibling.flac");
        for path in [&one, &two, &sibling] {
            store
                .upsert_track(
                    &Track {
                        path: path.clone(),
                        title: path.file_stem().unwrap().to_string_lossy().to_string(),
                        artist: "Alice".to_string(),
                        album: "First".to_string(),
                        genre: None,
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
        }

        let removed = store.mark_folder_removed(&album).unwrap();

        assert_eq!(removed, vec![one, two]);
        let remaining = store.load_tracks(std::slice::from_ref(&root)).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, sibling);
    }

    #[test]
    fn stores_discography_items_and_metadata_jobs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let artist_id = {
            let connection = store.lock_connection().unwrap();
            upsert_artist(&connection, "Brian Eno", now_millis()).unwrap()
        };

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
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

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
                    genre: Some("Rock".to_string()),
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
                    genre: Some("Rock".to_string()),
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
                    genre: None,
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
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

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
                    genre: None,
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
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

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
                    genre: None,
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
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        store
            .upsert_track(
                &Track {
                    path: root.join("solo.flac"),
                    title: "Solo".to_string(),
                    artist: "A$AP Rocky".to_string(),
                    album: "Testing".to_string(),
                    genre: None,
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
                    genre: None,
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

    #[test]
    fn splits_slash_collaboration_into_individual_artists_in_browse() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        // "John Lennon/Yoko Ono" should produce two distinct artists in
        // the Artists view, each with the album credited to them, even
        // though the underlying track tag string is a single value.
        store
            .upsert_track(
                &Track {
                    path: root.join("imagine.flac"),
                    title: "Imagine".to_string(),
                    artist: "John Lennon/Yoko Ono".to_string(),
                    album: "Imagine".to_string(),
                    genre: None,
                    track_number: None,
                    year: Some("1971".to_string()),
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
                    path: root.join("oh-yoko.flac"),
                    title: "Oh Yoko!".to_string(),
                    artist: "John Lennon / Yoko Ono".to_string(),
                    album: "Imagine".to_string(),
                    genre: None,
                    track_number: None,
                    year: Some("1971".to_string()),
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
        assert_eq!(
            artists.len(),
            2,
            "expected John Lennon and Yoko Ono as separate artists, got {:?}",
            artists.iter().map(|a| &a.name).collect::<Vec<_>>()
        );
        let lennon = artists
            .iter()
            .find(|artist| artist.name == "John Lennon")
            .expect("John Lennon should appear as an individual artist");
        assert_eq!(lennon.album_count, 1);
        assert_eq!(lennon.track_count, 2);
        let yoko = artists
            .iter()
            .find(|artist| artist.name == "Yoko Ono")
            .expect("Yoko Ono should appear as an individual artist");
        assert_eq!(yoko.album_count, 1);
        assert_eq!(yoko.track_count, 2);

        // The album is grouped under the primary (first-listed)
        // collaborator, so there should be a single "Imagine" album
        // credited to John Lennon.
        let albums = store.load_albums(std::slice::from_ref(&root)).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist, "John Lennon");
        assert_eq!(albums[0].track_count, 2);

        // The original tag string is preserved on the track itself --
        // only the browse-view aggregation is split.
        let tracks = store.load_tracks(&[root]).unwrap();
        assert!(
            tracks
                .iter()
                .any(|track| track.artist == "John Lennon/Yoko Ono")
        );
    }

    /// Build N synthetic tracks for the upsert benchmarks.
    fn synthetic_tracks(root: &Path, count: usize) -> Vec<Track> {
        (0..count)
            .map(|ix| Track {
                path: root.join(format!("track-{ix:05}.flac")),
                title: format!("Track {ix}"),
                artist: format!("Artist {}", ix % 100),
                album: format!("Album {}", ix % 200),
                genre: Some("Synthetic".to_string()),
                track_number: Some((ix % 20 + 1) as u32),
                year: Some("2024".to_string()),
                date_added: UNIX_EPOCH,
                duration: Duration::from_secs(180),
                codec: "FLAC".to_string(),
                sample_rate: Some(44_100),
                channels: Some(2),
                bitrate: Some(900),
                file_size: 5_000_000,
                modified: None,
                artwork: None,
            })
            .collect()
    }

    /// Microbenchmark: per-track upsert (single transaction per call).
    /// Run with `rtk cargo test --release bench_bulk_upsert_per_track -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_bulk_upsert_per_track() {
        const N: usize = 2_000;
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        let tracks = synthetic_tracks(&root, N);

        let scan_id = store.begin_scan(std::slice::from_ref(&root)).unwrap();
        let start = std::time::Instant::now();
        for track in &tracks {
            store.upsert_track(track, Some(scan_id)).unwrap();
        }
        let elapsed = start.elapsed();
        store
            .finish_scan(scan_id, std::slice::from_ref(&root))
            .unwrap();
        let per_track = elapsed / N as u32;
        eprintln!(
            "bench_bulk_upsert_per_track: {N} tracks in {elapsed:?} ({per_track:?} per track, \
             {:.1} tracks/sec)",
            N as f64 / elapsed.as_secs_f64()
        );

        let loaded = store.load_tracks(std::slice::from_ref(&root)).unwrap();
        assert_eq!(loaded.len(), N);
    }

    /// Microbenchmark: full pipeline -- batched upsert of N tracks
    /// followed by `load_tracks`/`load_artists`/`load_albums` queries.
    /// Captures end-to-end SQL aggregation cost which the audit
    /// targeted with the GROUP BY rewrite.
    #[test]
    #[ignore]
    fn bench_browse_load_after_upsert() {
        const N: usize = 2_000;
        const BATCH: usize = 256;
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        let tracks = synthetic_tracks(&root, N);

        let scan_id = store.begin_scan(std::slice::from_ref(&root)).unwrap();
        for chunk in tracks.chunks(BATCH) {
            store.upsert_tracks_batch(chunk, Some(scan_id)).unwrap();
        }
        store
            .finish_scan(scan_id, std::slice::from_ref(&root))
            .unwrap();

        let load_tracks_start = std::time::Instant::now();
        let loaded = store.load_tracks(std::slice::from_ref(&root)).unwrap();
        let load_tracks_elapsed = load_tracks_start.elapsed();
        assert_eq!(loaded.len(), N);

        let load_artists_start = std::time::Instant::now();
        let artists = store.load_artists(std::slice::from_ref(&root)).unwrap();
        let load_artists_elapsed = load_artists_start.elapsed();

        let load_albums_start = std::time::Instant::now();
        let albums = store.load_albums(std::slice::from_ref(&root)).unwrap();
        let load_albums_elapsed = load_albums_start.elapsed();

        eprintln!(
            "bench_browse_load_after_upsert ({N} tracks, {} artists, {} albums):\n  \
             load_tracks  = {load_tracks_elapsed:?}\n  \
             load_artists = {load_artists_elapsed:?}\n  \
             load_albums  = {load_albums_elapsed:?}",
            artists.len(),
            albums.len()
        );
    }

    /// Microbenchmark: batched upsert (single transaction for the whole
    /// batch). Mimics the cold-scan hot path.
    #[test]
    #[ignore]
    fn bench_bulk_upsert_batched() {
        const N: usize = 2_000;
        const BATCH: usize = 256;
        let temp_dir = tempfile::tempdir().unwrap();
        let store = CatalogStore::open_at(
            temp_dir.path().join("tempo.sqlite"),
            temp_dir.path().join("cache"),
        )
        .unwrap();

        let root = temp_dir.path().join("library");
        let tracks = synthetic_tracks(&root, N);

        let scan_id = store.begin_scan(std::slice::from_ref(&root)).unwrap();
        let start = std::time::Instant::now();
        for chunk in tracks.chunks(BATCH) {
            store.upsert_tracks_batch(chunk, Some(scan_id)).unwrap();
        }
        let elapsed = start.elapsed();
        store
            .finish_scan(scan_id, std::slice::from_ref(&root))
            .unwrap();
        let per_track = elapsed / N as u32;
        eprintln!(
            "bench_bulk_upsert_batched: {N} tracks (batch={BATCH}) in {elapsed:?} \
             ({per_track:?} per track, {:.1} tracks/sec)",
            N as f64 / elapsed.as_secs_f64()
        );

        let loaded = store.load_tracks(std::slice::from_ref(&root)).unwrap();
        assert_eq!(loaded.len(), N);
    }
}
