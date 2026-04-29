use super::*;

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
        if let Some(watcher) = self._library_watcher.take() {
            watcher.stop();
        }

        self.stop_current_playback();
        self.library_root_label = Self::library_root_label(&self.library_roots);
        self.tracks = Self::load_cached_tracks(self.catalog.as_ref(), &self.library_roots)
            .unwrap_or_default();
        self.reload_catalog_browse_data();
        self.queue.clear();
        self.waveform_cache.clear();
        self.waveform_loading.clear();
        self.invalidate_track_indices();
        for tab in &mut self.tabs {
            tab.selected_track = 0;
        }
        self.playing_track = 0;
        self.is_playing = false;
        self.context_menu_track = None;
        self.scan_progress = ScanProgress::default();
        self.is_scanning = false;

        let (event_tx, event_rx) = mpsc::channel();
        let (status, watcher) =
            Self::start_watcher_for_roots(&self.library_roots, event_tx, self.catalog.clone());
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
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;

                loop {
                    match event_rx.try_recv() {
                        Ok(event) => {
                            if this
                                .update(cx, |app, cx| {
                                    app.apply_library_event(event);
                                    cx.notify();
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                }
            }
        })
        .detach();
    }

    pub(super) fn load_cached_tracks(
        catalog: Option<&CatalogStore>,
        roots: &[PathBuf],
    ) -> anyhow::Result<Vec<Track>> {
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
        let Some(catalog) = catalog else {
            return Ok(Vec::new());
        };

        Ok(catalog
            .load_albums(roots)?
            .into_iter()
            .map(Album::from)
            .collect())
    }

    pub(super) fn reload_catalog_browse_data(&mut self) {
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
                self.context_menu_track = None;
                self.scan_progress = ScanProgress::default();
                self.scan_errors.clear();
                self.is_scanning = true;
                self.library_status = format!("Scanning {}", self.library_root_label);
            }
            LibraryEvent::ScanProgress(progress) => {
                self.scan_progress = progress;
                self.library_status = Self::scan_status(progress, self.is_scanning);
            }
            LibraryEvent::TracksIndexed(tracks) => {
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

                self.invalidate_track_indices();
                self.clamp_track_indices();
                if self.scan_progress.indexed < self.tracks.len() {
                    self.scan_progress.indexed = self.tracks.len();
                }
                self.library_status = Self::scan_status(self.scan_progress, self.is_scanning);
            }
            LibraryEvent::TrackRemoved(path) => {
                if let Some(ix) = self.tracks.iter().position(|track| track.path == path) {
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
            }
            LibraryEvent::ScanError(error) => {
                self.scan_progress.errors += 1;
                self.library_status = format!("Scan warning: {}", error.message);
                self.scan_errors.push(error);
            }
            LibraryEvent::ScanFinished => {
                if self.catalog.is_some()
                    && let Ok(tracks) =
                        Self::load_cached_tracks(self.catalog.as_ref(), &self.library_roots)
                {
                    self.tracks = tracks;
                    self.waveform_cache.clear();
                    self.waveform_loading.clear();
                    self.invalidate_track_indices();
                }
                self.reload_catalog_browse_data();
                self.clamp_track_indices();
                self.is_scanning = false;
                self.library_status = Self::scan_status(self.scan_progress, false);
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
