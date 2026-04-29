use super::*;
use std::time::{Duration as StdDuration, Instant};

const LIBRARY_EVENT_TICK: Duration = Duration::from_millis(100);
const LIBRARY_EVENT_BUDGET: StdDuration = StdDuration::from_millis(12);
const LIBRARY_EVENT_MAX_EVENTS: usize = 4;

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

    pub(super) fn save_app_state(&self) {
        let _span = perf::slow_span("app_state.save", Duration::from_millis(4), "");
        let Some(path) = Self::app_state_path() else {
            return;
        };

        let state = AppState {
            library_roots: self.library_roots.clone(),
            playlists: self.playlists.clone(),
            theme_id: self.theme_id.clone(),
            output_device: self.output_device.clone(),
            volume: self.volume,
            visible_table_columns: self.visible_columns.clone(),
            page: self.page,
            tabs: self.saved_tabs(),
            active_tab_id: self.tabs.get(self.active_tab).map(|tab| tab.id),
            artist_view_mode: self.artist_view_mode,
            album_view_mode: self.album_view_mode,
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
        };

        let Some(parent) = path.parent() else {
            return;
        };

        if fs::create_dir_all(parent).is_ok()
            && let Ok(contents) = serde_json::to_string_pretty(&state)
        {
            let _ = fs::write(path, contents);
        }
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
                    source: tab.source,
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

        self.stop_current_playback();
        self.library_root_label = Self::library_root_label(&self.library_roots);
        self.tracks = perf::time(
            "library.restart_watcher.load_cached_tracks",
            format!("roots={}", self.library_roots.len()),
            || {
                Self::load_cached_tracks(self.catalog.as_ref(), &self.library_roots)
                    .unwrap_or_default()
            },
        );
        perf::time("library.restart_watcher.reload_browse", "", || {
            self.reload_catalog_browse_data()
        });
        self.queue.clear();
        self.waveform_cache.clear();
        self.waveform_loading.clear();
        perf::time(
            "library.restart_watcher.rebuild_indices",
            format!("tracks={}", self.tracks.len()),
            || self.invalidate_track_indices(),
        );
        for tab in &mut self.tabs {
            tab.selected_track = 0;
        }
        self.playing_track = 0;
        self.is_playing = false;
        self.context_menu_track = None;
        self.scan_progress = ScanProgress::default();
        self.is_scanning = false;

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

                    if this
                        .update(cx, |app, cx| {
                            for event in pending_events {
                                app.apply_library_event(event);
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

        let parallel_span = perf::span("startup.cached_parallel", format!("roots={}", roots.len()));
        std::thread::scope(|scope| {
            let tracks_handle = {
                let catalog = catalog.clone();
                let roots = roots.to_vec();
                scope.spawn(move || {
                    perf::time(
                        "startup.cached_tracks",
                        format!("roots={}", roots.len()),
                        || Self::load_cached_tracks(Some(&catalog), &roots).unwrap_or_default(),
                    )
                })
            };
            let artists_handle = {
                let catalog = catalog.clone();
                let roots = roots.to_vec();
                scope.spawn(move || {
                    perf::time(
                        "startup.cached_artists",
                        format!("roots={}", roots.len()),
                        || Self::load_cached_artists(Some(&catalog), &roots).unwrap_or_default(),
                    )
                })
            };
            let albums_handle = {
                let catalog = catalog.clone();
                let roots = roots.to_vec();
                scope.spawn(move || {
                    perf::time(
                        "startup.cached_albums",
                        format!("roots={}", roots.len()),
                        || Self::load_cached_albums(Some(&catalog), &roots).unwrap_or_default(),
                    )
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
        }
        if let Ok(albums) = Self::load_cached_albums(self.catalog.as_ref(), &self.library_roots) {
            self.albums = albums;
        }
    }

    pub(super) fn apply_library_event(&mut self, event: LibraryEvent) {
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
                for track in tracks {
                    let track = Track::from(track);
                    if let Some(existing_ix) = self
                        .tracks
                        .iter()
                        .position(|existing| existing.path == track.path)
                    {
                        self.tracks[existing_ix] = track;
                        if existing_ix < self.waveform_cache.len() {
                            self.waveform_cache[existing_ix] = None;
                        }
                        if existing_ix < self.waveform_loading.len() {
                            self.waveform_loading[existing_ix] = false;
                        }
                    } else {
                        self.tracks.push(track);
                        self.waveform_cache.push(None);
                        self.waveform_loading.push(false);
                    }
                }

                let rebuild_start = Instant::now();
                self.invalidate_track_indices();
                perf::log_duration_if_slow(
                    "scan.tracks_indexed.rebuild_indices",
                    rebuild_start.elapsed(),
                    Duration::from_millis(4),
                    format!("tracks={} tabs={}", self.tracks.len(), self.tabs.len()),
                );
                let clamp_start = Instant::now();
                self.clamp_track_indices();
                perf::log_duration_if_slow(
                    "scan.tracks_indexed.clamp_indices",
                    clamp_start.elapsed(),
                    Duration::from_millis(4),
                    format!("tracks={}", self.tracks.len()),
                );
                if self.scan_progress.indexed < self.tracks.len() {
                    self.scan_progress.indexed = self.tracks.len();
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
                if let Some(ix) = self.tracks.iter().position(|track| track.path == path) {
                    self.scan_changed_tracks = true;
                    if let Some(catalog) = &self.catalog {
                        let _ = catalog.mark_file_removed(&path);
                    }
                    self.tracks.remove(ix);
                    if ix < self.waveform_cache.len() {
                        self.waveform_cache.remove(ix);
                    }
                    if ix < self.waveform_loading.len() {
                        self.waveform_loading.remove(ix);
                    }
                    self.remove_track_from_queue(ix);
                    self.invalidate_track_indices();
                    self.reload_catalog_browse_data();
                    self.clamp_track_indices();
                    self.library_status = Self::scan_status(self.scan_progress, self.is_scanning);
                }
                perf::log_duration(
                    "scan.track_removed.apply",
                    remove_start.elapsed(),
                    format!("path={}", path.display()),
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
                    self.waveform_cache.clear();
                    self.waveform_loading.clear();
                    self.invalidate_track_indices();
                }
                if changed {
                    self.reload_catalog_browse_data();
                }
                self.clamp_track_indices();
                self.is_scanning = false;
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

        if progress.discovered == 0 && progress.indexed == 0 && progress.errors == 0 {
            return format!("{prefix}: looking for audio files...");
        }

        let status = format!(
            "{prefix}: {} discovered, {} indexed",
            progress.discovered, progress.indexed
        );
        status
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
