use super::*;
use std::io::BufWriter;
use std::path::Path;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration as StdDuration, Instant};

const LIBRARY_EVENT_TICK: Duration = Duration::from_millis(100);
const LIBRARY_EVENT_BUDGET: StdDuration = StdDuration::from_millis(12);
const LIBRARY_EVENT_MAX_EVENTS: usize = 4;
const METADATA_EVENT_TICK: Duration = Duration::from_millis(500);
const METADATA_EVENT_MAX_EVENTS: usize = 16;
const METADATA_ACTIVITY_TICK: Duration = Duration::from_secs(2);

/// How long to wait after the last `request_save` call before actually
/// flushing state.json to disk. Tuned to absorb high-frequency callers
/// such as volume-slider drags and column reorder bursts.
const STATE_SAVE_DEBOUNCE: StdDuration = StdDuration::from_millis(500);

/// Background save loop: holds a single MPSC receiver, debounces snapshots
/// (keeping only the latest), and writes them to disk off the UI thread.
/// Initialized lazily on first use; the worker thread lives for the
/// lifetime of the process.
fn state_save_sender() -> &'static mpsc::Sender<(PathBuf, AppState)> {
    static SENDER: OnceLock<mpsc::Sender<(PathBuf, AppState)>> = OnceLock::new();
    SENDER.get_or_init(spawn_state_saver)
}

fn spawn_state_saver() -> mpsc::Sender<(PathBuf, AppState)> {
    let (tx, rx) = mpsc::channel::<(PathBuf, AppState)>();
    thread::Builder::new()
        .name("tempo-state-saver".into())
        .spawn(move || run_state_saver(rx))
        .expect("failed to spawn state saver thread");
    tx
}

fn run_state_saver(rx: mpsc::Receiver<(PathBuf, AppState)>) {
    // Hold at most one pending snapshot. Any subsequent send during the
    // debounce window simply overwrites it; only the latest snapshot is
    // ever written, which is correct because state.json is fully
    // overwritten on each save.
    let mut pending: Option<(PathBuf, AppState)> = None;
    loop {
        let next = if pending.is_some() {
            rx.recv_timeout(STATE_SAVE_DEBOUNCE).map(Some)
        } else {
            rx.recv()
                .map(Some)
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };

        match next {
            Ok(Some((path, state))) => {
                pending = Some((path, state));
            }
            Ok(None) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some((path, state)) = pending.take() {
                    write_state_to_disk(&path, &state);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some((path, state)) = pending.take() {
                    write_state_to_disk(&path, &state);
                }
                return;
            }
        }
    }
}

fn write_state_to_disk(path: &Path, state: &AppState) {
    let _span = perf::slow_span(
        "app_state.write",
        StdDuration::from_millis(4),
        format!("path={}", path.display()),
    );
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }

    // Atomic write: serialize to a temp sibling, then rename. Eliminates
    // the risk of leaving a half-written state.json after a crash mid-write.
    let tmp_path = path.with_extension("json.tmp");
    let file = match fs::File::create(&tmp_path) {
        Ok(file) => file,
        Err(_) => return,
    };
    let mut writer = BufWriter::new(file);
    if serde_json::to_writer(&mut writer, state).is_err() {
        let _ = fs::remove_file(&tmp_path);
        return;
    }
    use std::io::Write;
    if writer.flush().is_err() {
        let _ = fs::remove_file(&tmp_path);
        return;
    }
    drop(writer);
    let _ = fs::rename(&tmp_path, path);
}

impl TempoApp {
    pub(super) fn default_library_roots(saved_roots: &[PathBuf]) -> Vec<PathBuf> {
        if let Some(path) = env::var_os("TEMPO_MUSIC_DIR").map(PathBuf::from) {
            return vec![path];
        }

        if !saved_roots.is_empty() {
            return saved_roots.to_vec();
        }

        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Music"))
            .filter(|path| path.exists())
            .into_iter()
            .collect()
    }

    pub(super) fn load_app_state() -> AppState {
        let Some(path) = Self::app_state_path() else {
            return AppState::default();
        };

        let Ok(contents) = fs::read_to_string(path) else {
            return AppState::default();
        };

        serde_json::from_str(&contents).unwrap_or_default()
    }

    pub(super) fn app_state_path() -> Option<PathBuf> {
        if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
            return Some(config_home.join("tempo").join("state.json"));
        }

        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config").join("tempo").join("state.json"))
    }

    /// Build a serializable snapshot of the app's persistent state. Heavy
    /// `clone()` calls live here because `AppState` owns its data; the
    /// serialize + I/O work is performed by the background save thread,
    /// so the snapshot itself is the only cost paid on the UI thread.
    pub(super) fn build_app_state_snapshot(&self) -> AppState {
        // `volume_snapshot` and `output_device_snapshot` are mirrors
        // of authoritative state on `self.player`, kept here so the
        // snapshot can be built from `&self` only — that lets the
        // dozens of `save_app_state()` callsites scattered across
        // settings/playlist/tab mutations stay one-arg. The mirrors
        // are refreshed in `handle_player_event` whenever
        // `PlayerEvent::StateMutated` fires.
        AppState {
            library_roots: self.library_roots.clone(),
            playlists: self.playlists.clone(),
            theme_id: self.theme_id.clone(),
            output_device: self.output_device_snapshot.clone(),
            online_metadata_mode: self.online_metadata_mode,
            volume: self.volume_snapshot,
            visible_table_columns: self.visible_columns.clone(),
            visible_artist_table_columns: self.visible_artist_columns.clone(),
            visible_album_table_columns: self.visible_album_columns.clone(),
            visible_genre_table_columns: self.visible_genre_columns.clone(),
            page: self.page,
            tabs: self.saved_tabs(),
            active_tab_id: self.tabs.get(self.active_tab).map(|tab| tab.id),
            artist_view_mode: self.browse_view_mode(),
            album_view_mode: self.browse_view_mode(),
            genre_view_mode: self.browse_view_mode(),
            artist_table_sort_column: self.artist_table_sort_column,
            artist_table_sort_direction: self.artist_table_sort_direction,
            album_table_sort_column: self.album_table_sort_column,
            album_table_sort_direction: self.album_table_sort_direction,
            genre_table_sort_column: self.genre_table_sort_column,
            genre_table_sort_direction: self.genre_table_sort_direction,
            genre_grid_scroll_top: Self::uniform_list_scroll_top(&self.genre_grid_scroll_handle),
            artist_grid_scroll_top: Self::uniform_list_scroll_top(&self.artist_grid_scroll_handle),
            artist_table_scroll_top: Self::uniform_list_scroll_top(
                &self.artist_table_scroll_handle,
            ),
            album_grid_scroll_top: Self::uniform_list_scroll_top(&self.album_grid_scroll_handle),
            album_table_scroll_top: Self::uniform_list_scroll_top(&self.album_table_scroll_handle),
            playback_history: self.playback_history.clone(),
            playing_track_path: self
                .tracks
                .get(self.playing_track)
                .map(|track| track.path.clone()),
            // Always serialize as `true` after the app has booted -- the
            // startup path runs the migration before constructing
            // `TempoApp`, so by the time we're saving, the layout is
            // already in its post-migration form.
            liked_column_migrated: true,
        }
    }

    /// Request a debounced state save. Cheap on the calling thread: takes
    /// a snapshot of the persistent fields and hands it to a long-running
    /// background thread that coalesces high-frequency callers (volume
    /// drag, column reorder) into a single write per `STATE_SAVE_DEBOUNCE`
    /// window. Used everywhere except shutdown.
    pub(super) fn save_app_state(&self) {
        let _span = perf::slow_span("app_state.save_request", Duration::from_millis(4), "");
        let Some(path) = Self::app_state_path() else {
            return;
        };
        let state = self.build_app_state_snapshot();
        let _ = state_save_sender().send((path, state));
    }

    /// Synchronous, blocking save. Used at shutdown so the latest state is
    /// always flushed even if the debounce window hadn't elapsed.
    pub(super) fn save_app_state_now(&self) {
        let _span = perf::slow_span("app_state.save_now", Duration::from_millis(4), "");
        let Some(path) = Self::app_state_path() else {
            return;
        };
        let state = self.build_app_state_snapshot();
        write_state_to_disk(&path, &state);
    }

    pub(super) fn saved_tabs(&self) -> Vec<SavedBrowseTab> {
        self.tabs
            .iter()
            .map(|tab| {
                let base_handle = tab.table_scroll_handle.0.borrow().base_handle.clone();
                let has_rendered = f32::from(base_handle.bounds().size.height) > 0.0;
                let scroll_top = if has_rendered {
                    (-f32::from(base_handle.offset().y)).max(0.0)
                } else {
                    tab.table_scroll_top.max(0.0)
                };
                SavedBrowseTab {
                    id: tab.id,
                    source: tab.source.clone(),
                    search_query: tab.search_query.clone(),
                    sort_column: tab.sort_column,
                    sort_direction: tab.sort_direction,
                    selected_track: tab.selected_track,
                    table_scroll_top: scroll_top,
                    table_horizontal_scroll: tab.table_horizontal_scroll,
                }
            })
            .collect()
    }

    pub(super) fn uniform_list_scroll_top(handle: &UniformListScrollHandle) -> f32 {
        (-f32::from(handle.0.borrow().base_handle.offset().y)).max(0.0)
    }

    pub(super) fn library_root_label(roots: &[PathBuf]) -> String {
        match roots {
            [] => "No library root".to_string(),
            [root] => root.display().to_string(),
            roots => format!("{} folders", roots.len()),
        }
    }

    pub(super) fn start_watcher_for_roots(
        roots: &[PathBuf],
        event_tx: mpsc::Sender<LibraryEvent>,
        catalog: Option<CatalogStore>,
    ) -> (String, Option<LibraryWatcher>) {
        if roots.is_empty() {
            return (
                "No folders configured. Add a music folder in Settings.".to_string(),
                None,
            );
        }

        let library_root_label = Self::library_root_label(roots);
        let mut indexer = LibraryIndexer::new(roots.to_vec());
        if let Some(catalog) = catalog {
            indexer = indexer.with_catalog(catalog);
        }

        match indexer.start_watching(event_tx) {
            Ok(watcher) => (format!("Scanning {library_root_label}"), Some(watcher)),
            Err(error) => (format!("Library watcher failed: {error}"), None),
        }
    }

    pub(super) fn restart_library_watcher(&mut self, cx: &mut Context<Self>) {
        let _span = perf::span(
            "library.restart_watcher",
            format!("roots={}", self.library_roots.len()),
        );
        if let Some(watcher) = self._library_watcher.take() {
            perf::time("library.restart_watcher.stop_old", "", || watcher.stop());
        }

        self.player
            .update(cx, |player, cx| player.reset_for_library_reload(cx));
        self.library_root_label = Self::library_root_label(&self.library_roots);
        self.tracks = perf::time(
            "library.restart_watcher.load_cached_tracks",
            format!("roots={}", self.library_roots.len()),
            || {
                Self::load_cached_tracks(self.catalog.as_ref(), &self.library_roots)
                    .unwrap_or_default()
            },
        );
        self.track_path_index = build_track_path_index(&self.tracks);
        self.library_size_bytes = self.tracks.iter().map(|track| track.file_size).sum();
        perf::time("library.restart_watcher.reload_browse", "", || {
            self.reload_catalog_browse_data()
        });
        self.queue.clear();
        perf::time(
            "library.restart_watcher.rebuild_indices",
            format!("tracks={}", self.tracks.len()),
            || self.invalidate_track_indices(),
        );
        for tab in &mut self.tabs {
            tab.selected_track = 0;
        }
        self.playing_track = 0;
        self.context_menu_track = None;
        // Push a fresh snapshot so the player bar reflects the new
        // tracks[0] (or shows the empty placeholder if the new
        // library is empty).
        let new_snapshot = self
            .tracks
            .first()
            .map(player::PlayingTrackSnapshot::from_track);
        self.player
            .update(cx, |player, _| player.set_playing_track(new_snapshot));
        self.scan_progress = ScanProgress::default();
        self.is_scanning = false;
        self.last_scan_browse_reload = None;

        let (event_tx, event_rx) = mpsc::channel();
        let (status, watcher) = perf::time(
            "library.restart_watcher.start_new",
            format!("roots={}", self.library_roots.len()),
            || Self::start_watcher_for_roots(&self.library_roots, event_tx, self.catalog.clone()),
        );
        self.library_status = status;
        self._library_watcher = watcher;
        self.start_library_event_loop(event_rx, cx);
    }

    pub(super) fn add_library_roots(&mut self, roots: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut changed = false;

        for root in roots {
            if !root.exists()
                || !root.is_dir()
                || self.library_roots.iter().any(|path| path == &root)
            {
                continue;
            }

            self.library_roots.push(root);
            changed = true;
        }

        if changed {
            self.open_page(Page::Library);
            self.save_app_state();
            self.restart_library_watcher(cx);
        }
    }

    pub(super) fn remove_library_root(&mut self, root_ix: usize, cx: &mut Context<Self>) {
        if root_ix < self.library_roots.len() {
            self.library_roots.remove(root_ix);
            if self.library_roots.is_empty() {
                self.set_page_without_history(Page::Settings);
                self.back_history.clear();
                self.forward_history.clear();
            }
            self.save_app_state();
            self.restart_library_watcher(cx);
        }
    }

    pub(super) fn start_library_event_loop(
        &self,
        event_rx: mpsc::Receiver<LibraryEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(LIBRARY_EVENT_TICK).await;

                let drain_start = Instant::now();
                let mut event_count = 0_usize;
                let mut track_count = 0_usize;
                let mut error_count = 0_usize;
                let mut pending_tracks = Vec::new();
                let mut pending_removed_tracks = Vec::new();
                let mut pending_events = Vec::new();
                while event_count < LIBRARY_EVENT_MAX_EVENTS
                    && drain_start.elapsed() < LIBRARY_EVENT_BUDGET
                {
                    match event_rx.try_recv() {
                        Ok(event) => {
                            event_count += 1;
                            match event {
                                LibraryEvent::TracksIndexed(tracks) => {
                                    track_count += tracks.len();
                                    pending_tracks.extend(tracks);
                                }
                                LibraryEvent::TrackRemoved(path) => {
                                    pending_removed_tracks.push(path);
                                }
                                LibraryEvent::TracksRemoved(paths) => {
                                    pending_removed_tracks.extend(paths);
                                }
                                LibraryEvent::ScanError(error) => {
                                    error_count += 1;
                                    pending_events.push(LibraryEvent::ScanError(error));
                                }
                                event => pending_events.push(event),
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                if event_count > 0 {
                    if !pending_tracks.is_empty() {
                        pending_events.push(LibraryEvent::TracksIndexed(pending_tracks));
                    }
                    if !pending_removed_tracks.is_empty() {
                        pending_events.push(LibraryEvent::TracksRemoved(pending_removed_tracks));
                    }

                    if this
                        .update(cx, |app, cx| {
                            for event in pending_events {
                                app.apply_library_event(event, cx);
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }

                    perf::log_duration(
                        "library.event_drain",
                        drain_start.elapsed(),
                        format!("events={event_count} tracks={track_count} errors={error_count}"),
                    );
                }
            }
        })
        .detach();
    }

    pub(super) fn start_metadata_event_loop(
        &self,
        event_rx: mpsc::Receiver<MetadataEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(METADATA_EVENT_TICK).await;

                let mut updated_artists = Vec::new();
                let mut updated_albums = Vec::new();
                for _ in 0..METADATA_EVENT_MAX_EVENTS {
                    match event_rx.try_recv() {
                        Ok(MetadataEvent::ArtistUpdated(artist_id)) => {
                            updated_artists.push(artist_id);
                        }
                        Ok(MetadataEvent::AlbumUpdated(album_id)) => {
                            updated_albums.push(album_id);
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }

                if updated_artists.is_empty() && updated_albums.is_empty() {
                    continue;
                }

                if this
                    .update(cx, |app, cx| {
                        app.reload_catalog_browse_data();
                        app.spawn_snapshot_rebuild("metadata_updated");
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }

                perf::event(
                    "metadata.events.applied",
                    format!(
                        "artists={} albums={}",
                        updated_artists.len(),
                        updated_albums.len()
                    ),
                );
            }
        })
        .detach();
    }

    pub(super) fn start_metadata_activity_poll(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(METADATA_ACTIVITY_TICK).await;

                let Ok((mode, catalog)) = this.update(cx, |app, _cx| {
                    (app.online_metadata_mode, app.catalog.clone())
                }) else {
                    return;
                };
                if mode != OnlineMetadataMode::Automatic {
                    continue;
                }
                let Some(catalog) = catalog else {
                    continue;
                };
                let Ok(activity) = catalog.load_metadata_activity() else {
                    continue;
                };

                if this
                    .update(cx, |app, cx| {
                        app.metadata_activity = activity;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn load_cached_tracks(
        catalog: Option<&CatalogStore>,
        roots: &[PathBuf],
    ) -> anyhow::Result<Vec<Track>> {
        let _span = perf::span(
            "library.load_cached_tracks",
            format!("roots={}", roots.len()),
        );
        let Some(catalog) = catalog else {
            return Ok(Vec::new());
        };

        Ok(catalog
            .load_tracks(roots)?
            .into_iter()
            .map(Track::from)
            .collect())
    }

    pub(super) fn load_cached_artists(
        catalog: Option<&CatalogStore>,
        roots: &[PathBuf],
    ) -> anyhow::Result<Vec<Artist>> {
        let _span = perf::span(
            "library.load_cached_artists",
            format!("roots={}", roots.len()),
        );
        let Some(catalog) = catalog else {
            return Ok(Vec::new());
        };

        Ok(catalog
            .load_artists(roots)?
            .into_iter()
            .map(Artist::from)
            .collect())
    }

    pub(super) fn load_cached_albums(
        catalog: Option<&CatalogStore>,
        roots: &[PathBuf],
    ) -> anyhow::Result<Vec<Album>> {
        let _span = perf::span(
            "library.load_cached_albums",
            format!("roots={}", roots.len()),
        );
        let Some(catalog) = catalog else {
            return Ok(Vec::new());
        };

        Ok(catalog
            .load_albums(roots)?
            .into_iter()
            .map(Album::from)
            .collect())
    }

    /// Load tracks/artists/albums for startup. Tries the binary snapshot
    /// first (single sequential read, ~tens of ms for 8k tracks); falls back
    /// to running the three SQLite catalog queries in parallel.
    pub(super) fn load_browse_caches_for_startup(
        catalog: Option<&CatalogStore>,
        roots: &[PathBuf],
    ) -> (Vec<Track>, Vec<Artist>, Vec<Album>) {
        let _span = perf::span(
            "startup.load_browse_caches",
            format!("roots={}", roots.len()),
        );

        if let Some(catalog) = catalog
            && let Some(snapshot) = perf::time(
                "startup.snapshot_load",
                format!("roots={}", roots.len()),
                || tempo::snapshot::load(catalog.cache_dir(), roots),
            )
        {
            perf::event(
                "startup.snapshot.hit",
                format!(
                    "tracks={} artists={} albums={}",
                    snapshot.tracks.len(),
                    snapshot.artists.len(),
                    snapshot.albums.len()
                ),
            );
            let tracks = perf::time(
                "startup.snapshot_to_tracks",
                format!("count={}", snapshot.tracks.len()),
                || snapshot.tracks.into_iter().map(Track::from).collect(),
            );
            let artists = perf::time(
                "startup.snapshot_to_artists",
                format!("count={}", snapshot.artists.len()),
                || snapshot.artists.into_iter().map(Artist::from).collect(),
            );
            let albums = perf::time(
                "startup.snapshot_to_albums",
                format!("count={}", snapshot.albums.len()),
                || snapshot.albums.into_iter().map(Album::from).collect(),
            );
            return (tracks, artists, albums);
        }

        perf::event("startup.snapshot.miss", "");

        // Fallback path: run the three SQLite loads in parallel. Each
        // `load_*` opens its own short-lived `Connection`, so they do not
        // contend on a shared transaction; with WAL + mmap, three readers
        // overlap nicely on disk + CPU.
        let Some(catalog) = catalog else {
            return (Vec::new(), Vec::new(), Vec::new());
        };

        // Helper that runs each catalog query, logs any error loudly,
        // and falls back to an empty Vec. Logging here is the only
        // breadcrumb the user has when a SQL-side breakage (e.g. a
        // missing column from a half-applied migration) silently
        // empties the browse pages and triggers a full rescan -- before
        // this, every catalog error was swallowed by `unwrap_or_default`
        // with no surface signal at all.
        fn run_load<T>(
            label: &'static str,
            roots: &[PathBuf],
            f: impl FnOnce() -> anyhow::Result<Vec<T>>,
        ) -> Vec<T> {
            perf::time(label, format!("roots={}", roots.len()), || match f() {
                Ok(values) => values,
                Err(error) => {
                    perf::event(&format!("{label}.failed"), format!("error={error:#}"));
                    eprintln!("[tempo] {label} failed: {error:#}");
                    Vec::new()
                }
            })
        }

        let parallel_span = perf::span("startup.cached_parallel", format!("roots={}", roots.len()));
        std::thread::scope(|scope| {
            let tracks_handle = {
                let catalog = catalog.clone();
                let roots = roots.to_vec();
                scope.spawn(move || {
                    run_load("startup.cached_tracks", &roots, || {
                        Self::load_cached_tracks(Some(&catalog), &roots)
                    })
                })
            };
            let artists_handle = {
                let catalog = catalog.clone();
                let roots = roots.to_vec();
                scope.spawn(move || {
                    run_load("startup.cached_artists", &roots, || {
                        Self::load_cached_artists(Some(&catalog), &roots)
                    })
                })
            };
            let albums_handle = {
                let catalog = catalog.clone();
                let roots = roots.to_vec();
                scope.spawn(move || {
                    run_load("startup.cached_albums", &roots, || {
                        Self::load_cached_albums(Some(&catalog), &roots)
                    })
                })
            };

            let tracks = tracks_handle.join().unwrap_or_default();
            let artists = artists_handle.join().unwrap_or_default();
            let albums = albums_handle.join().unwrap_or_default();
            drop(parallel_span);
            (tracks, artists, albums)
        })
    }

    /// Rewrite the on-disk binary snapshot from SQLite on a background
    /// thread. We re-query the catalog (rather than reusing the in-memory
    /// browse vectors) so the snapshot stays bound to the canonical
    /// `Catalog*` shape, and so we don't have to keep a parallel copy alive
    /// in the UI struct. The work is fire-and-forget; failures are logged
    /// via the `perf` channel and the next startup just falls back to
    /// SQLite again.
    pub(super) fn spawn_snapshot_rebuild(&self, reason: &'static str) {
        let Some(catalog) = self.catalog.clone() else {
            return;
        };
        let roots = self.library_roots.clone();
        std::thread::Builder::new()
            .name("tempo-snapshot".into())
            .spawn(move || {
                let _span = perf::span("snapshot.rebuild", format!("reason={reason}"));
                let tracks = match catalog.load_tracks(&roots) {
                    Ok(tracks) => tracks,
                    Err(error) => {
                        perf::event(
                            "snapshot.rebuild.error",
                            format!("stage=tracks error={error:#}"),
                        );
                        return;
                    }
                };
                let artists = match catalog.load_artists(&roots) {
                    Ok(artists) => artists,
                    Err(error) => {
                        perf::event(
                            "snapshot.rebuild.error",
                            format!("stage=artists error={error:#}"),
                        );
                        return;
                    }
                };
                let albums = match catalog.load_albums(&roots) {
                    Ok(albums) => albums,
                    Err(error) => {
                        perf::event(
                            "snapshot.rebuild.error",
                            format!("stage=albums error={error:#}"),
                        );
                        return;
                    }
                };
                if let Err(error) =
                    tempo::snapshot::save(catalog.cache_dir(), &roots, &tracks, &artists, &albums)
                {
                    perf::event(
                        "snapshot.rebuild.error",
                        format!("stage=save error={error:#}"),
                    );
                }
            })
            .ok();
    }

    pub(super) fn reload_catalog_browse_data(&mut self) {
        let _span = perf::span("library.reload_catalog_browse_data", "");
        if let Ok(artists) = Self::load_cached_artists(self.catalog.as_ref(), &self.library_roots) {
            self.artists = artists;
            // Bump the generation so the browse filter cache invalidates
            // on the next read; the cache key includes this counter.
            self.artists_generation = self.artists_generation.wrapping_add(1);
            self.artist_filter_cache.borrow_mut().invalidate();
        }
        if let Ok(albums) = Self::load_cached_albums(self.catalog.as_ref(), &self.library_roots) {
            self.albums = albums;
            self.albums_generation = self.albums_generation.wrapping_add(1);
            self.album_filter_cache.borrow_mut().invalidate();
        }
        self.rebuild_genres();
    }

    pub(super) fn apply_library_event(&mut self, event: LibraryEvent, cx: &mut Context<Self>) {
        match event {
            LibraryEvent::ScanStarted => {
                perf::event(
                    "scan.started",
                    format!("roots={}", self.library_roots.len()),
                );
                self.context_menu_track = None;
                self.scan_progress = ScanProgress::default();
                self.scan_errors.clear();
                self.scan_changed_tracks = false;
                self.last_scan_browse_reload = None;
                self.is_scanning = true;
                self.library_status = format!("Scanning {}", self.library_root_label);
            }
            LibraryEvent::ScanProgress(progress) => {
                perf::event(
                    "scan.progress",
                    format!(
                        "discovered={} indexed={} errors={}",
                        progress.discovered, progress.indexed, progress.errors
                    ),
                );
                self.scan_progress = progress;
                self.library_status = Self::scan_status(progress, self.is_scanning);
            }
            LibraryEvent::TracksIndexed(tracks) => {
                let apply_start = Instant::now();
                let indexed_count = tracks.len();
                self.scan_changed_tracks = self.scan_changed_tracks || indexed_count > 0;
                // Paths whose waveform cache entries were superseded
                // by a fresh scan record. Invalidated in one
                // `player.update` at the end of the loop instead of
                // borrowing the player entity inside the hot-path
                // upsert loop.
                let mut waveforms_to_invalidate: Vec<PathBuf> = Vec::new();
                for track in tracks {
                    let track = Track::from(track);
                    // O(1) reverse lookup via `track_path_index` instead of
                    // the prior O(N) linear scan over `self.tracks`. This
                    // matters: cold scans of a 50k-track library used to
                    // do ~50k * (batch_size) PathBuf comparisons per
                    // batch.
                    if let Some(&existing_ix) = self.track_path_index.get(&track.path) {
                        // Maintain `library_size_bytes` incrementally:
                        // subtract the old size before overwriting.
                        let old_size = self.tracks[existing_ix].file_size;
                        self.library_size_bytes = self.library_size_bytes.saturating_sub(old_size);
                        self.library_size_bytes =
                            self.library_size_bytes.saturating_add(track.file_size);
                        waveforms_to_invalidate.push(track.path.clone());
                        self.tracks[existing_ix] = track;
                    } else {
                        let new_ix = self.tracks.len();
                        self.track_path_index.insert(track.path.clone(), new_ix);
                        self.library_size_bytes =
                            self.library_size_bytes.saturating_add(track.file_size);
                        self.tracks.push(track);
                    }
                }
                if !waveforms_to_invalidate.is_empty() {
                    self.player.update(cx, |player, _| {
                        for path in &waveforms_to_invalidate {
                            player.invalidate_waveform_for_path(path);
                        }
                    });
                }
                self.rebuild_genres();

                let rebuild_start = Instant::now();
                self.invalidate_track_indices();
                perf::log_duration_if_slow(
                    "scan.tracks_indexed.rebuild_indices",
                    rebuild_start.elapsed(),
                    Duration::from_millis(4),
                    format!("tracks={} tabs={}", self.tracks.len(), self.tabs.len()),
                );
                let clamp_start = Instant::now();
                self.clamp_track_indices(cx);
                perf::log_duration_if_slow(
                    "scan.tracks_indexed.clamp_indices",
                    clamp_start.elapsed(),
                    Duration::from_millis(4),
                    format!("tracks={}", self.tracks.len()),
                );
                if self.scan_progress.indexed < self.tracks.len() {
                    self.scan_progress.indexed = self.tracks.len();
                }
                if indexed_count > 0 {
                    self.reload_catalog_browse_data_during_scan();
                }
                self.library_status = Self::scan_status(self.scan_progress, self.is_scanning);
                perf::log_duration(
                    "scan.tracks_indexed.apply",
                    apply_start.elapsed(),
                    format!("batch={indexed_count} total={}", self.tracks.len()),
                );
            }
            LibraryEvent::TrackRemoved(path) => {
                let remove_start = Instant::now();
                self.apply_removed_track_paths(vec![path.clone()], cx);
                perf::log_duration(
                    "scan.track_removed.apply",
                    remove_start.elapsed(),
                    format!("path={}", path.display()),
                );
            }
            LibraryEvent::TracksRemoved(paths) => {
                let remove_start = Instant::now();
                let removed_count = self.apply_removed_track_paths(paths, cx);
                perf::log_duration(
                    "scan.tracks_removed.apply",
                    remove_start.elapsed(),
                    format!("removed={removed_count}"),
                );
            }
            LibraryEvent::ScanError(error) => {
                perf::event(
                    "scan.error",
                    format!("path={} message={}", error.path.display(), error.message),
                );
                self.scan_progress.errors += 1;
                self.library_status = format!("Scan warning: {}", error.message);
                self.scan_errors.push(error);
            }
            LibraryEvent::ScanFinished => {
                let finish_start = Instant::now();
                let changed = self.scan_changed_tracks;
                if changed
                    && self.catalog.is_some()
                    && let Ok(tracks) =
                        Self::load_cached_tracks(self.catalog.as_ref(), &self.library_roots)
                {
                    self.tracks = tracks;
                    self.track_path_index = build_track_path_index(&self.tracks);
                    self.library_size_bytes = self.tracks.iter().map(|track| track.file_size).sum();
                    self.rebuild_genres();
                    self.player
                        .update(cx, |player, _| player.clear_waveform_cache());
                    self.invalidate_track_indices();
                }
                if changed {
                    self.reload_catalog_browse_data();
                }
                self.clamp_track_indices(cx);
                self.is_scanning = false;
                self.last_scan_browse_reload = None;
                self.library_status = Self::scan_status(self.scan_progress, false);
                perf::log_duration(
                    "scan.finished.apply",
                    finish_start.elapsed(),
                    format!(
                        "changed={} tracks={} artists={} albums={} errors={}",
                        changed,
                        self.tracks.len(),
                        self.artists.len(),
                        self.albums.len(),
                        self.scan_progress.errors
                    ),
                );
                if changed {
                    self.spawn_snapshot_rebuild("scan_finished");
                }
            }
        }
    }

    fn apply_removed_track_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) -> usize {
        if paths.is_empty() {
            return 0;
        }

        let removed_paths = paths.into_iter().collect::<std::collections::HashSet<_>>();
        let mut removed_indices = self
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(ix, track)| removed_paths.contains(&track.path).then_some(ix))
            .collect::<Vec<_>>();

        if removed_indices.is_empty() {
            return 0;
        }

        self.scan_changed_tracks = true;
        removed_indices.sort_unstable_by(|left, right| right.cmp(left));

        // Drop the player's waveform cache entries for the removed
        // tracks. The cache is keyed by path so this is independent
        // of the index shifting that follows in the track-list mutation.
        self.player.update(cx, |player, _| {
            for path in &removed_paths {
                player.invalidate_waveform_for_path(path);
            }
        });

        for ix in &removed_indices {
            let removed_size = self.tracks[*ix].file_size;
            self.library_size_bytes = self.library_size_bytes.saturating_sub(removed_size);
            self.tracks.remove(*ix);
            self.remove_track_from_queue(*ix);
        }
        // Removals shift every subsequent track's index, so the reverse
        // map needs a full rebuild. Removals are far less common than
        // upserts, and this rebuild is still O(N) instead of the prior
        // O(N*M) per-batch upsert scan it replaces.
        self.track_path_index = build_track_path_index(&self.tracks);
        self.rebuild_genres();

        self.invalidate_track_indices();
        self.reload_catalog_browse_data();
        self.clamp_track_indices(cx);
        self.library_status = Self::scan_status(self.scan_progress, self.is_scanning);
        if !self.is_scanning {
            self.spawn_snapshot_rebuild("tracks_removed");
        }
        removed_indices.len()
    }

    fn reload_catalog_browse_data_during_scan(&mut self) {
        let now = Instant::now();
        if self.last_scan_browse_reload.is_some_and(|last_reload| {
            now.duration_since(last_reload) < SCAN_BROWSE_RELOAD_INTERVAL
        }) {
            return;
        }

        self.reload_catalog_browse_data();
        self.last_scan_browse_reload = Some(now);
    }

    pub(super) fn scan_status(progress: ScanProgress, is_scanning: bool) -> String {
        let mut status = Self::scan_status_summary(progress, is_scanning);

        if progress.errors > 0 {
            status.push_str(&format!(", {} errors", progress.errors));
        }

        status
    }

    pub(super) fn scan_status_summary(progress: ScanProgress, is_scanning: bool) -> String {
        let prefix = if is_scanning {
            "Scanning"
        } else {
            "Monitoring"
        };

        // The full "{prefix}: looking for audio files..." / "{prefix}:
        // N discovered, N indexed" form is still produced for the
        // Settings page (`render_library_settings` calls
        // `visible_scan_status()` which embeds this verbatim). For
        // the top bar (`render_scan_status` →
        // `visible_scan_status_without_errors`) we deliberately omit
        // the counts because they update at high frequency during
        // scans and the bare prefix is enough at-a-glance signal.
        let _ = progress;
        prefix.to_string()
    }

    pub(super) fn visible_scan_status(&self) -> String {
        self.visible_scan_status_with(self.library_status.clone())
    }

    pub(super) fn visible_scan_status_without_errors(&self) -> String {
        if self.scan_progress.errors == 0 {
            return self.visible_scan_status();
        }

        let error_suffix = format!(", {} errors", self.scan_progress.errors);
        let library_status = self
            .library_status
            .strip_suffix(&error_suffix)
            .unwrap_or(&self.library_status)
            .to_string();

        self.visible_scan_status_with(library_status)
    }

    pub(super) fn visible_scan_status_with(&self, library_status: String) -> String {
        let total = self.active_source_track_count();
        if self.active_search_query().trim().is_empty() {
            return format!("{} items  ·  {}", total, library_status);
        }

        format!(
            "{} of {} items  ·  {}",
            self.filtered_track_count(),
            total,
            library_status
        )
    }
}
