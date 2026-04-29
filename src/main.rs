use std::{
    env, fs,
    ops::Range,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::{
    AnyElement, App, Application, Bounds, ClickEvent, Context, CursorStyle, FocusHandle, Image,
    ImageFormat, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, PathPromptOptions, Pixels, Render,
    ScrollStrategy, ScrollWheelEvent, SharedString, Styled, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, actions, div, img, point, prelude::*, px, rgb, size, uniform_list,
};
use rodio::{Decoder, Source as _};
use serde::{Deserialize, Serialize};
use tempo::{
    library::{
        Artwork as LibraryArtwork, IndexingError, LibraryEvent, LibraryIndexer, LibraryWatcher,
        ScanProgress,
    },
    playback::PlaybackController,
};

actions!(
    tempo,
    [
        PlaySelected,
        TogglePause,
        MoveSelectionUp,
        MoveSelectionDown,
        NewTab,
        FocusSearch
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Index,
    Title,
    Album,
    Format,
    Plays,
    Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableColumn {
    Index,
    Artwork,
    Title,
    Album,
    Format,
    Plays,
    Duration,
    Loved,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy)]
struct ColumnWidths {
    index: f32,
    artwork: f32,
    title: f32,
    album: f32,
    format: f32,
    plays: f32,
    duration: f32,
    loved: f32,
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            index: INDEX_COL_W,
            artwork: ART_COL_W,
            title: TITLE_COL_W,
            album: ALBUM_COL_W,
            format: FMT_COL_W,
            plays: PLAYS_COL_W,
            duration: TIME_COL_W,
            loved: LOVE_COL_W,
        }
    }
}

#[derive(Clone, Copy)]
struct ColumnResize {
    column: TableColumn,
    start_x: f32,
    start_width: f32,
}

#[derive(Clone)]
struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    album: String,
    year: String,
    duration: String,
    duration_value: Duration,
    codec: String,
    bitrate: Option<u32>,
    file_size: u64,
    plays: String,
    loved: bool,
    artwork: Option<TrackArtwork>,
    album_initials: String,
    album_color: u32,
}

#[derive(Clone)]
struct WaveformSource {
    path: PathBuf,
    title: String,
    artist: String,
    album: String,
    duration: String,
    duration_value: Duration,
}

impl WaveformSource {
    fn from_track(track: &Track) -> Self {
        Self {
            path: track.path.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration: track.duration.clone(),
            duration_value: track.duration_value,
        }
    }
}

#[derive(Clone)]
enum TrackArtwork {
    Embedded(Arc<Image>),
    File(PathBuf),
}

#[derive(Clone)]
struct TrackDrag {
    track_ix: usize,
    title: SharedString,
    artist: SharedString,
    position: gpui::Point<Pixels>,
}

impl TrackDrag {
    fn new(track_ix: usize, track: &Track) -> Self {
        Self {
            track_ix,
            title: track.title.clone().into(),
            artist: track.artist.clone().into(),
            position: gpui::Point::default(),
        }
    }

    fn position(mut self, position: gpui::Point<Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for TrackDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(18.0))
            .pt(self.position.y - px(18.0))
            .child(
                div()
                    .w(px(220.0))
                    .h(px(42.0))
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x4b4f5a))
                    .bg(rgb(0x202229))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xf0f0f4))
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(0x9a9ea8))
                            .child(self.artist.clone()),
                    ),
            )
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Playlist {
    name: String,
    track_paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TabSource {
    Library,
    Playlist(usize),
}

struct BrowseTab {
    source: TabSource,
    search_query: String,
    sort_column: SortColumn,
    sort_direction: SortDirection,
    selected_track: usize,
    table_scroll_handle: UniformListScrollHandle,
    track_indices: Vec<usize>,
    scrollbar_markers: Vec<ScrollbarMarker>,
}

#[derive(Clone)]
struct ScrollbarMarker {
    ratio: f32,
    label: String,
}

#[derive(Clone, Copy)]
struct TableScrollbarDrag {
    thumb_offset: f32,
}

#[derive(Clone, Copy)]
struct TableScrollbarMetrics {
    track_top: f32,
    track_height: f32,
    thumb_top: f32,
    thumb_height: f32,
    max_scroll: f32,
    scroll_top: f32,
}

impl BrowseTab {
    fn library() -> Self {
        Self {
            source: TabSource::Library,
            search_query: String::new(),
            sort_column: SortColumn::Index,
            sort_direction: SortDirection::Ascending,
            selected_track: 0,
            table_scroll_handle: UniformListScrollHandle::new(),
            track_indices: Vec::new(),
            scrollbar_markers: Vec::new(),
        }
    }

    fn playlist(playlist_ix: usize) -> Self {
        Self {
            source: TabSource::Playlist(playlist_ix),
            search_query: String::new(),
            sort_column: SortColumn::Index,
            sort_direction: SortDirection::Ascending,
            selected_track: 0,
            table_scroll_handle: UniformListScrollHandle::new(),
            track_indices: Vec::new(),
            scrollbar_markers: Vec::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct AppState {
    library_roots: Vec<PathBuf>,
    playlists: Vec<Playlist>,
}

const INDEX_COL_W: f32 = 34.0;
const ART_COL_W: f32 = 32.0;
const TITLE_COL_W: f32 = 188.0;
const ALBUM_COL_W: f32 = 230.0;
const FMT_COL_W: f32 = 70.0;
const PLAYS_COL_W: f32 = 82.0;
const TIME_COL_W: f32 = 64.0;
const LOVE_COL_W: f32 = 24.0;
const TABLE_ROW_H: f32 = 32.0;
const LEFT_SIDEBAR_W: f32 = 220.0;
const RIGHT_SIDEBAR_W: f32 = 300.0;
const WAVEFORM_SEGMENTS: usize = 360;
const PLAYER_BAR_PAD: f32 = 16.0;
const PLAYER_ART_W: f32 = 54.0;
const PLAYER_INFO_W: f32 = 220.0;
const PLAYER_CONTROLS_W: f32 = 170.0;
const PLAYER_GAP: f32 = 16.0;
const TABLE_SCROLLBAR_W: f32 = 54.0;
const TABLE_SCROLLBAR_TRACK_W: f32 = 6.0;
const TABLE_SCROLLBAR_MARGIN: f32 = 4.0;
const TABLE_SCROLLBAR_MIN_THUMB_H: f32 = 32.0;
const TABLE_SCROLLBAR_MAX_MARKERS: usize = 28;
const TABLE_SCROLL_IDLE_DELAY: Duration = Duration::from_millis(120);
const FAST_SCROLL_OVERSCAN_ROWS: usize = 4;

struct TempoApp {
    focus_handle: FocusHandle,
    search_focus_handle: FocusHandle,
    page: Page,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
    column_widths: ColumnWidths,
    column_resize: Option<ColumnResize>,
    tabs: Vec<BrowseTab>,
    active_tab: usize,
    playing_track: usize,
    is_playing: bool,
    context_menu_track: Option<usize>,
    context_menu_row: usize,
    tracks: Vec<Track>,
    queue: Vec<usize>,
    waveform_cache: Vec<Option<Vec<f32>>>,
    waveform_loading: Vec<bool>,
    library_roots: Vec<PathBuf>,
    playlists: Vec<Playlist>,
    library_root_label: String,
    library_status: String,
    playback_status: String,
    scan_progress: ScanProgress,
    scan_errors: Vec<IndexingError>,
    show_scan_errors: bool,
    is_scanning: bool,
    table_scrollbar_drag: Option<TableScrollbarDrag>,
    table_is_scrolling: bool,
    table_scroll_generation: u64,
    _library_watcher: Option<LibraryWatcher>,
    playback: Option<PlaybackController>,
}

impl TempoApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let state = Self::load_app_state();
        let roots = Self::default_library_roots(&state.library_roots);
        let library_root_label = Self::library_root_label(&roots);
        let (event_tx, event_rx) = mpsc::channel();
        let (library_status, library_watcher) = Self::start_watcher_for_roots(&roots, event_tx);
        let playlists = state.playlists;
        let (playback, playback_status) = match PlaybackController::new() {
            Ok(playback) => (Some(playback), "Audio output ready".to_string()),
            Err(error) => (None, format!("Playback unavailable: {error:#}")),
        };

        let initial_page = if roots.is_empty() {
            Page::Settings
        } else {
            Page::Library
        };

        let app = Self {
            focus_handle,
            search_focus_handle,
            page: initial_page,
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            column_widths: ColumnWidths::default(),
            column_resize: None,
            tabs: vec![BrowseTab::library()],
            active_tab: 0,
            playing_track: 0,
            is_playing: false,
            context_menu_track: None,
            context_menu_row: 0,
            tracks: Vec::new(),
            queue: Vec::new(),
            waveform_cache: Vec::new(),
            waveform_loading: Vec::new(),
            library_roots: roots,
            playlists,
            library_root_label,
            library_status,
            playback_status,
            scan_progress: ScanProgress::default(),
            scan_errors: Vec::new(),
            show_scan_errors: false,
            is_scanning: false,
            table_scrollbar_drag: None,
            table_is_scrolling: false,
            table_scroll_generation: 0,
            _library_watcher: library_watcher,
            playback,
        };

        app.start_library_event_loop(event_rx, cx);
        app.start_playback_tick(cx);
        app
    }

    fn default_library_roots(saved_roots: &[PathBuf]) -> Vec<PathBuf> {
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

    fn load_app_state() -> AppState {
        let Some(path) = Self::app_state_path() else {
            return AppState::default();
        };

        let Ok(contents) = fs::read_to_string(path) else {
            return AppState::default();
        };

        serde_json::from_str(&contents).unwrap_or_default()
    }

    fn app_state_path() -> Option<PathBuf> {
        if let Some(config_home) = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
            return Some(config_home.join("tempo").join("state.json"));
        }

        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config").join("tempo").join("state.json"))
    }

    fn save_app_state(&self) {
        let Some(path) = Self::app_state_path() else {
            return;
        };

        let state = AppState {
            library_roots: self.library_roots.clone(),
            playlists: self.playlists.clone(),
        };

        let Some(parent) = path.parent() else {
            return;
        };

        if fs::create_dir_all(parent).is_ok() {
            if let Ok(contents) = serde_json::to_string_pretty(&state) {
                let _ = fs::write(path, contents);
            }
        }
    }

    fn library_root_label(roots: &[PathBuf]) -> String {
        match roots {
            [] => "No library root".to_string(),
            [root] => root.display().to_string(),
            roots => format!("{} folders", roots.len()),
        }
    }

    fn start_watcher_for_roots(
        roots: &[PathBuf],
        event_tx: mpsc::Sender<LibraryEvent>,
    ) -> (String, Option<LibraryWatcher>) {
        if roots.is_empty() {
            return (
                "No folders configured. Add a music folder in Settings.".to_string(),
                None,
            );
        }

        let library_root_label = Self::library_root_label(roots);
        match LibraryIndexer::new(roots.to_vec()).start_watching(event_tx) {
            Ok(watcher) => (format!("Scanning {library_root_label}"), Some(watcher)),
            Err(error) => (format!("Library watcher failed: {error}"), None),
        }
    }

    fn restart_library_watcher(&mut self, cx: &mut Context<Self>) {
        if let Some(watcher) = self._library_watcher.take() {
            watcher.stop();
        }

        self.stop_current_playback();
        self.library_root_label = Self::library_root_label(&self.library_roots);
        self.tracks.clear();
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
        let (status, watcher) = Self::start_watcher_for_roots(&self.library_roots, event_tx);
        self.library_status = status;
        self._library_watcher = watcher;
        self.start_library_event_loop(event_rx, cx);
    }

    fn add_library_roots(&mut self, roots: Vec<PathBuf>, cx: &mut Context<Self>) {
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
            self.page = Page::Library;
            self.save_app_state();
            self.restart_library_watcher(cx);
        }
    }

    fn remove_library_root(&mut self, root_ix: usize, cx: &mut Context<Self>) {
        if root_ix < self.library_roots.len() {
            self.library_roots.remove(root_ix);
            if self.library_roots.is_empty() {
                self.page = Page::Settings;
            }
            self.save_app_state();
            self.restart_library_watcher(cx);
        }
    }

    fn create_playlist(&mut self) {
        let name = self.next_playlist_name();
        self.playlists.push(Playlist {
            name,
            track_paths: Vec::new(),
        });
        self.invalidate_track_indices();
        self.save_app_state();
    }

    fn add_track_to_playlist(&mut self, track_ix: usize, playlist_ix: usize) {
        let Some(track_path) = self.tracks.get(track_ix).map(|track| track.path.clone()) else {
            return;
        };

        let Some(playlist) = self.playlists.get_mut(playlist_ix) else {
            return;
        };

        playlist.track_paths.push(track_path);
        self.invalidate_track_indices();
        self.save_app_state();
        self.context_menu_track = None;
    }

    fn next_playlist_name(&self) -> String {
        let base = "New Playlist";
        if !self.playlists.iter().any(|playlist| playlist.name == base) {
            return base.to_string();
        }

        for ix in 2.. {
            let name = format!("{base} {ix}");
            if !self.playlists.iter().any(|playlist| playlist.name == name) {
                return name;
            }
        }

        base.to_string()
    }

    fn open_page(&mut self, page: Page) {
        self.page = if page == Page::Library && self.library_roots.is_empty() {
            Page::Settings
        } else {
            page
        };
        self.context_menu_track = None;
    }

    fn active_tab(&self) -> &BrowseTab {
        &self.tabs[self.active_tab]
    }

    fn active_tab_mut(&mut self) -> &mut BrowseTab {
        &mut self.tabs[self.active_tab]
    }

    fn active_search_query(&self) -> &str {
        &self.active_tab().search_query
    }

    fn active_selected_track(&self) -> usize {
        self.active_tab().selected_track
    }

    fn set_active_selected_track(&mut self, track_ix: usize) {
        self.active_tab_mut().selected_track = track_ix;
    }

    fn tab_title(&self, tab: &BrowseTab) -> String {
        let query = tab.search_query.trim();
        if !query.is_empty() {
            return query.to_string();
        }

        match tab.source {
            TabSource::Library => "All Music".to_string(),
            TabSource::Playlist(playlist_ix) => self
                .playlists
                .get(playlist_ix)
                .map(|playlist| playlist.name.clone())
                .unwrap_or_else(|| "Missing Playlist".to_string()),
        }
    }

    fn new_library_tab(&mut self) {
        self.tabs.push(BrowseTab::library());
        self.active_tab = self.tabs.len() - 1;
        self.rebuild_track_indices_for_tab(self.active_tab);
        self.page = Page::Library;
        self.context_menu_track = None;
    }

    fn open_all_music_tab(&mut self) {
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| tab.source == TabSource::Library && tab.search_query.trim().is_empty())
        {
            self.active_tab = tab_ix;
        } else {
            self.tabs.push(BrowseTab::library());
            self.active_tab = self.tabs.len() - 1;
            self.rebuild_track_indices_for_tab(self.active_tab);
        }

        self.open_page(Page::Library);
    }

    fn open_playlist_tab(&mut self, playlist_ix: usize) {
        if playlist_ix >= self.playlists.len() {
            return;
        }

        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| tab.source == TabSource::Playlist(playlist_ix))
        {
            self.active_tab = tab_ix;
        } else {
            self.tabs.push(BrowseTab::playlist(playlist_ix));
            self.active_tab = self.tabs.len() - 1;
            self.rebuild_track_indices_for_tab(self.active_tab);
        }

        self.open_page(Page::Library);
    }

    fn close_tab(&mut self, tab_ix: usize) {
        if self.tabs.len() <= 1 || tab_ix >= self.tabs.len() {
            return;
        }

        self.tabs.remove(tab_ix);
        if self.active_tab > tab_ix {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.context_menu_track = None;
    }

    fn start_library_event_loop(
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

    fn start_playback_tick(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;

                if this
                    .update(cx, |app, cx| {
                        if app.is_playing {
                            if app
                                .playback
                                .as_ref()
                                .is_some_and(|playback| playback.is_empty())
                            {
                                app.is_playing = false;
                                app.playback_status = "Playback finished".to_string();
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }
}

impl From<tempo::library::Track> for Track {
    fn from(track: tempo::library::Track) -> Self {
        let album_initials = TempoApp::album_initials_for(&track.album, &track.title);
        let album_color = TempoApp::album_color_for(&track.album, &track.artist);

        Self {
            path: track.path,
            title: track.title,
            artist: track.artist,
            album: track.album,
            year: track.year.unwrap_or_else(|| "Unknown year".to_string()),
            duration: format_duration(track.duration),
            duration_value: track.duration,
            codec: track.codec,
            bitrate: track.bitrate,
            file_size: track.file_size,
            plays: "0".to_string(),
            loved: false,
            artwork: track.artwork.and_then(TrackArtwork::from_library),
            album_initials,
            album_color,
        }
    }
}

impl TrackArtwork {
    fn from_library(artwork: LibraryArtwork) -> Option<Self> {
        match artwork {
            LibraryArtwork::Embedded { mime_type, data } => {
                image_format_from_artwork(mime_type.as_deref(), &data)
                    .map(|format| Self::Embedded(Arc::new(Image::from_bytes(format, data))))
            }
            LibraryArtwork::File(path) => Some(Self::File(path)),
        }
    }
}

fn image_format_from_artwork(mime_type: Option<&str>, data: &[u8]) -> Option<ImageFormat> {
    match mime_type.unwrap_or_default().to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/bmp" => Some(ImageFormat::Bmp),
        "image/tiff" | "image/tif" => Some(ImageFormat::Tiff),
        _ if data.starts_with(b"\x89PNG\r\n\x1a\n") => Some(ImageFormat::Png),
        _ if data.starts_with(&[0xff, 0xd8, 0xff]) => Some(ImageFormat::Jpeg),
        _ if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") => Some(ImageFormat::Gif),
        _ if data.starts_with(b"BM") => Some(ImageFormat::Bmp),
        _ => None,
    }
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}

impl Render for TempoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tempo-app")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::play_selected))
            .on_action(cx.listener(Self::toggle_pause))
            .on_action(cx.listener(Self::move_selection_up))
            .on_action(cx.listener(Self::move_selection_down))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::focus_search))
            .on_key_down(cx.listener(Self::handle_table_key_down))
            .size_full()
            .bg(rgb(0x111216))
            .text_color(rgb(0xd8d8dd))
            .font_family("Inter")
            .text_sm()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_left_sidebar(cx))
                    .child(self.render_content(cx)),
            )
            .child(self.render_player_bar(window, cx))
    }
}

impl TempoApp {
    fn apply_library_event(&mut self, event: LibraryEvent) {
        match event {
            LibraryEvent::ScanStarted => {
                self.stop_current_playback();
                self.tracks.clear();
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
                self.scan_errors.clear();
                self.show_scan_errors = false;
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
                    self.tracks.remove(ix);
                    if ix < self.waveform_cache.len() {
                        self.waveform_cache.remove(ix);
                    }
                    if ix < self.waveform_loading.len() {
                        self.waveform_loading.remove(ix);
                    }
                    self.remove_track_from_queue(ix);
                    self.invalidate_track_indices();
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
                self.clamp_track_indices();
                self.is_scanning = false;
                self.library_status = Self::scan_status(self.scan_progress, false);
            }
        }
    }

    fn scan_status(progress: ScanProgress, is_scanning: bool) -> String {
        let mut status = Self::scan_status_summary(progress, is_scanning);

        if progress.errors > 0 {
            status.push_str(&format!(", {} errors", progress.errors));
        }

        status
    }

    fn scan_status_summary(progress: ScanProgress, is_scanning: bool) -> String {
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

    fn visible_scan_status(&self) -> String {
        self.visible_scan_status_with(self.library_status.clone())
    }

    fn visible_scan_status_without_errors(&self) -> String {
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

    fn visible_scan_status_with(&self, library_status: String) -> String {
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

    fn filtered_track_count(&self) -> usize {
        self.current_track_indices().len()
    }

    fn active_source_track_count(&self) -> usize {
        self.source_track_count(self.active_tab().source)
    }

    fn invalidate_track_indices(&mut self) {
        for tab_ix in 0..self.tabs.len() {
            self.rebuild_track_indices_for_tab(tab_ix);
        }
    }

    fn current_track_indices(&self) -> &[usize] {
        self.track_indices_for_tab(self.active_tab)
    }

    fn track_indices_for_tab(&self, tab_ix: usize) -> &[usize] {
        self.tabs
            .get(tab_ix)
            .map(|tab| tab.track_indices.as_slice())
            .unwrap_or_default()
    }

    fn rebuild_track_indices_for_tab(&mut self, tab_ix: usize) {
        let Some(tab) = self.tabs.get(tab_ix) else {
            return;
        };

        let source = tab.source;
        let search_query = tab.search_query.clone();
        let sort_column = tab.sort_column;
        let sort_direction = tab.sort_direction;

        let indices =
            self.compute_track_indices(source, &search_query, sort_column, sort_direction);
        let scrollbar_markers = self.compute_scrollbar_markers(&indices, sort_column);
        if let Some(tab) = self.tabs.get_mut(tab_ix) {
            tab.track_indices = indices;
            tab.scrollbar_markers = scrollbar_markers;
        }
    }

    fn compute_track_indices(
        &self,
        source: TabSource,
        search_query: &str,
        sort_column: SortColumn,
        sort_direction: SortDirection,
    ) -> Vec<usize> {
        let terms = Self::search_terms(search_query);
        let mut indices = self
            .source_track_indices(source)
            .into_iter()
            .filter(|track_ix| {
                self.tracks
                    .get(*track_ix)
                    .is_some_and(|track| Self::track_matches_search_terms(track, &terms))
            })
            .collect::<Vec<_>>();

        if sort_column == SortColumn::Index {
            if sort_direction == SortDirection::Descending {
                indices.reverse();
            }
            return indices;
        }

        indices.sort_by(|a, b| {
            let left = &self.tracks[*a];
            let right = &self.tracks[*b];
            let ordering = match sort_column {
                SortColumn::Index => a.cmp(b),
                SortColumn::Title => left.title.cmp(&right.title),
                SortColumn::Album => left
                    .album
                    .cmp(&right.album)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Format => left
                    .codec
                    .cmp(&right.codec)
                    .then(left.title.cmp(&right.title)),
                SortColumn::Plays => left
                    .plays
                    .parse::<u32>()
                    .unwrap_or_default()
                    .cmp(&right.plays.parse::<u32>().unwrap_or_default()),
                SortColumn::Duration => left.duration_value.cmp(&right.duration_value),
            };

            match sort_direction {
                SortDirection::Ascending => ordering,
                SortDirection::Descending => ordering.reverse(),
            }
        });

        indices
    }

    fn compute_scrollbar_markers(
        &self,
        indices: &[usize],
        sort_column: SortColumn,
    ) -> Vec<ScrollbarMarker> {
        if indices.len() <= 1 {
            return Vec::new();
        }

        match sort_column {
            SortColumn::Title | SortColumn::Album | SortColumn::Format => {
                self.compute_grouped_scrollbar_markers(indices, sort_column)
            }
            SortColumn::Index | SortColumn::Plays | SortColumn::Duration => {
                self.compute_sampled_scrollbar_markers(indices, sort_column)
            }
        }
    }

    fn compute_grouped_scrollbar_markers(
        &self,
        indices: &[usize],
        sort_column: SortColumn,
    ) -> Vec<ScrollbarMarker> {
        let denominator = indices.len().saturating_sub(1) as f32;
        let mut markers = Vec::new();
        let mut previous_label = String::new();

        for (row_ix, track_ix) in indices.iter().copied().enumerate() {
            let Some(track) = self.tracks.get(track_ix) else {
                continue;
            };
            let label = match sort_column {
                SortColumn::Title => Self::marker_initial(&track.title),
                SortColumn::Album => Self::marker_initial(&track.album),
                SortColumn::Format => track.codec.to_ascii_uppercase(),
                SortColumn::Index | SortColumn::Plays | SortColumn::Duration => unreachable!(),
            };

            if label == previous_label {
                continue;
            }

            previous_label = label.clone();
            markers.push(ScrollbarMarker {
                ratio: row_ix as f32 / denominator,
                label,
            });

            if markers.len() >= TABLE_SCROLLBAR_MAX_MARKERS {
                break;
            }
        }

        markers
    }

    fn compute_sampled_scrollbar_markers(
        &self,
        indices: &[usize],
        sort_column: SortColumn,
    ) -> Vec<ScrollbarMarker> {
        let samples = [0.0_f32, 0.25, 0.5, 0.75, 1.0];
        let last_row = indices.len().saturating_sub(1);
        let mut markers = Vec::new();

        for ratio in samples {
            let row_ix = (ratio * last_row as f32).round() as usize;
            let Some(track_ix) = indices.get(row_ix).copied() else {
                continue;
            };
            let label = self.scrollbar_marker_label(track_ix, sort_column);
            if markers
                .iter()
                .any(|marker: &ScrollbarMarker| marker.label == label)
            {
                continue;
            }

            markers.push(ScrollbarMarker { ratio, label });
        }

        markers
    }

    fn marker_initial(value: &str) -> String {
        value
            .trim_start()
            .chars()
            .find(|ch| ch.is_alphanumeric())
            .map(|ch| ch.to_uppercase().collect::<String>())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| "#".to_string())
    }

    fn scrollbar_marker_label(&self, track_ix: usize, sort_column: SortColumn) -> String {
        let Some(track) = self.tracks.get(track_ix) else {
            return String::new();
        };

        match sort_column {
            SortColumn::Index => format!("{}", track_ix + 1),
            SortColumn::Title => Self::marker_initial(&track.title),
            SortColumn::Album => Self::marker_initial(&track.album),
            SortColumn::Format => track.codec.to_ascii_uppercase(),
            SortColumn::Plays => track.plays.clone(),
            SortColumn::Duration => track.duration.clone(),
        }
    }

    fn source_track_indices(&self, source: TabSource) -> Vec<usize> {
        match source {
            TabSource::Library => (0..self.tracks.len()).collect(),
            TabSource::Playlist(playlist_ix) => self
                .playlists
                .get(playlist_ix)
                .map(|playlist| {
                    playlist
                        .track_paths
                        .iter()
                        .filter_map(|path| self.tracks.iter().position(|track| track.path == *path))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn source_track_count(&self, source: TabSource) -> usize {
        match source {
            TabSource::Library => self.tracks.len(),
            TabSource::Playlist(_) => self.source_track_indices(source).len(),
        }
    }

    fn set_search_query(&mut self, query: String) {
        self.active_tab_mut().search_query = query;
        self.context_menu_track = None;
        self.invalidate_track_indices();
        let selected_track = self.active_selected_track();
        let replacement_track = {
            let indices = self.current_track_indices();
            if let Some(first_track_ix) = indices.first() {
                (!indices.contains(&selected_track)).then_some(*first_track_ix)
            } else {
                Some(0)
            }
        };
        if let Some(track_ix) = replacement_track {
            self.set_active_selected_track(track_ix);
        }
        self.active_tab()
            .table_scroll_handle
            .scroll_to_item(0, ScrollStrategy::Top);
    }

    fn clear_search_query(&mut self) {
        if !self.active_search_query().is_empty() {
            self.set_search_query(String::new());
        }
    }

    fn handle_search_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return;
        }

        match event.keystroke.key.as_str() {
            "backspace" => {
                let mut query = self.active_search_query().to_string();
                query.pop();
                self.set_search_query(query);
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                self.clear_search_query();
                cx.stop_propagation();
                cx.notify();
            }
            _ => {
                let Some(key_char) = event.keystroke.key_char.as_ref() else {
                    return;
                };
                if key_char.chars().all(|ch| !ch.is_control()) {
                    let mut query = self.active_search_query().to_string();
                    query.push_str(key_char);
                    self.set_search_query(query);
                    cx.stop_propagation();
                    cx.notify();
                }
            }
        }
    }

    fn search_terms(query: &str) -> Vec<String> {
        query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect()
    }

    fn track_matches_search_terms(track: &Track, terms: &[String]) -> bool {
        if terms.is_empty() {
            return true;
        }

        let searchable = format!(
            "{} {} {} {} {} {}",
            track.title,
            track.artist,
            track.album,
            track.year,
            track.codec,
            track.path.display()
        )
        .to_lowercase();

        terms.iter().all(|term| searchable.contains(term))
    }

    fn column_width(&self, column: TableColumn) -> f32 {
        match column {
            TableColumn::Index => self.column_widths.index,
            TableColumn::Artwork => self.column_widths.artwork,
            TableColumn::Title => self.column_widths.title,
            TableColumn::Album => self.column_widths.album,
            TableColumn::Format => self.column_widths.format,
            TableColumn::Plays => self.column_widths.plays,
            TableColumn::Duration => self.column_widths.duration,
            TableColumn::Loved => self.column_widths.loved,
        }
    }

    fn set_column_width(&mut self, column: TableColumn, width: f32) {
        let width = width.max(Self::min_column_width(column));
        match column {
            TableColumn::Index => self.column_widths.index = width,
            TableColumn::Artwork => self.column_widths.artwork = width,
            TableColumn::Title => self.column_widths.title = width,
            TableColumn::Album => self.column_widths.album = width,
            TableColumn::Format => self.column_widths.format = width,
            TableColumn::Plays => self.column_widths.plays = width,
            TableColumn::Duration => self.column_widths.duration = width,
            TableColumn::Loved => self.column_widths.loved = width,
        }
    }

    fn min_column_width(column: TableColumn) -> f32 {
        match column {
            TableColumn::Index | TableColumn::Artwork | TableColumn::Loved => 24.0,
            TableColumn::Format => 44.0,
            TableColumn::Plays | TableColumn::Duration => 52.0,
            TableColumn::Title | TableColumn::Album => 96.0,
        }
    }

    fn begin_column_resize(&mut self, column: TableColumn, event: &MouseDownEvent) {
        self.column_resize = Some(ColumnResize {
            column,
            start_x: f32::from(event.position.x),
            start_width: self.column_width(column),
        });
        self.context_menu_track = None;
    }

    fn resize_column_from_mouse(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(resize) = self.column_resize else {
            return false;
        };

        let delta = f32::from(event.position.x) - resize.start_x;
        self.set_column_width(resize.column, resize.start_width + delta);
        true
    }

    fn finish_column_resize(&mut self) -> bool {
        self.column_resize.take().is_some()
    }

    fn remove_track_from_queue(&mut self, removed_ix: usize) {
        self.queue = self
            .queue
            .iter()
            .filter_map(|track_ix| {
                if *track_ix == removed_ix {
                    None
                } else if *track_ix > removed_ix {
                    Some(*track_ix - 1)
                } else {
                    Some(*track_ix)
                }
            })
            .collect();
    }

    fn queue_track(&mut self, track_ix: usize) {
        if track_ix >= self.tracks.len() {
            return;
        }

        self.queue.push(track_ix);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    fn queue_album_from_track(&mut self, track_ix: usize, shuffled: bool) {
        let Some(album) = self.tracks.get(track_ix).map(|track| track.album.clone()) else {
            return;
        };

        let mut album_tracks = self
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(ix, track)| (track.album == album).then_some(ix))
            .collect::<Vec<_>>();

        if shuffled {
            let seed = Self::shuffle_seed();
            album_tracks.sort_by_key(|track_ix| {
                Self::shuffle_key(&self.tracks[*track_ix], *track_ix, seed)
            });
        }

        self.queue.extend(album_tracks);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    fn shuffle_seed() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_default()
    }

    fn shuffle_key(track: &Track, track_ix: usize, seed: u64) -> u64 {
        let mut hash = seed ^ ((track_ix as u64).wrapping_mul(0x9e3779b97f4a7c15));

        for part in [&track.title, &track.artist, &track.album] {
            for byte in part.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }

        hash ^ (hash >> 33)
    }

    fn clamp_track_indices(&mut self) {
        if self.tracks.is_empty() {
            for tab in &mut self.tabs {
                tab.selected_track = 0;
            }
            self.playing_track = 0;
            self.context_menu_track = None;
            self.is_playing = false;
            return;
        }

        let last = self.tracks.len() - 1;
        self.playing_track = self.playing_track.min(last);

        for tab_ix in 0..self.tabs.len() {
            let selected_track = self.tabs[tab_ix].selected_track.min(last);
            let replacement_track = {
                let indices = self.track_indices_for_tab(tab_ix);
                (!indices.contains(&selected_track)).then(|| indices.first().copied().unwrap_or(0))
            };
            let tab = &mut self.tabs[tab_ix];
            tab.selected_track = replacement_track.unwrap_or(selected_track);
        }

        if self
            .context_menu_track
            .is_some_and(|track_ix| track_ix > last)
        {
            self.context_menu_track = None;
        }

        self.queue.retain(|track_ix| *track_ix <= last);
    }

    fn play_track(&mut self, track_ix: usize) {
        let Some(track) = self.tracks.get(track_ix) else {
            return;
        };
        let track_path = track.path.clone();

        self.playing_track = track_ix;
        self.set_active_selected_track(track_ix);
        self.context_menu_track = None;

        let Some(playback) = &self.playback else {
            self.is_playing = false;
            return;
        };

        match playback.play_path(&track_path) {
            Ok(()) => {
                self.is_playing = true;
                self.playback_status = "Playing through default output".to_string();
            }
            Err(error) => {
                self.is_playing = false;
                self.playback_status = format!("Playback failed: {error:#}");
            }
        }
    }

    fn toggle_playback(&mut self) {
        if self.tracks.is_empty() {
            return;
        }

        if self.is_playing {
            if let Some(playback) = &self.playback {
                playback.pause();
            }
            self.is_playing = false;
            self.playback_status = "Playback paused".to_string();
            self.context_menu_track = None;
            return;
        }

        if self
            .playback
            .as_ref()
            .is_some_and(|playback| playback.is_empty())
        {
            self.play_track(self.playing_track);
            return;
        }

        if let Some(playback) = &self.playback {
            playback.resume();
            self.is_playing = true;
            self.playback_status = "Playing through default output".to_string();
        }

        self.context_menu_track = None;
    }

    fn stop_current_playback(&mut self) {
        if let Some(playback) = &self.playback {
            playback.stop();
        }
        self.is_playing = false;
    }

    fn play_adjacent_track(&mut self, delta: isize) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let position = indices
            .iter()
            .position(|ix| *ix == self.playing_track)
            .unwrap_or(0);
        let next = (position as isize + delta).clamp(0, indices.len().saturating_sub(1) as isize);
        self.play_track(indices[next as usize]);
    }

    fn playback_position(&self) -> Duration {
        self.playback
            .as_ref()
            .filter(|playback| !playback.is_empty())
            .map(PlaybackController::position)
            .unwrap_or_default()
    }

    fn seek_from_waveform_click(&mut self, click_x: f32, viewport_width: f32) {
        let Some(track) = self.tracks.get(self.playing_track) else {
            return;
        };

        let waveform_left = PLAYER_BAR_PAD + PLAYER_ART_W + PLAYER_GAP + PLAYER_INFO_W + PLAYER_GAP;
        let waveform_right = viewport_width - (PLAYER_GAP + PLAYER_CONTROLS_W + PLAYER_BAR_PAD);
        let waveform_width = (waveform_right - waveform_left).max(1.0);
        let ratio = ((click_x - waveform_left) / waveform_width).clamp(0.0, 1.0);
        let target = track.duration_value.mul_f32(ratio);

        self.seek_playback(target);
    }

    fn seek_playback(&mut self, position: Duration) {
        if self
            .playback
            .as_ref()
            .is_some_and(|playback| playback.is_empty())
        {
            self.play_track(self.playing_track);
        }

        match &self.playback {
            Some(playback) => match playback.seek(position) {
                Ok(()) => {
                    self.playback_status = format!("Seeked to {}", format_duration(position));
                }
                Err(error) => {
                    self.playback_status = format!("Seek failed: {error:#}");
                }
            },
            None => {
                self.playback_status = "Playback unavailable".to_string();
            }
        }
    }

    fn play_selected(&mut self, _: &PlaySelected, _: &mut Window, cx: &mut Context<Self>) {
        if self.tracks.is_empty() {
            return;
        }

        self.play_track(self.active_selected_track());
        cx.notify();
    }

    fn toggle_pause(&mut self, _: &TogglePause, _: &mut Window, cx: &mut Context<Self>) {
        if self.tracks.is_empty() {
            return;
        }

        self.toggle_playback();
        cx.notify();
    }

    fn move_selection_up(&mut self, _: &MoveSelectionUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1);
        cx.notify();
    }

    fn move_selection_down(
        &mut self,
        _: &MoveSelectionDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selection(1);
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.new_library_tab();
        window.focus(&self.search_focus_handle);
        cx.notify();
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_page(Page::Library);
        window.focus(&self.search_focus_handle);
        cx.notify();
    }

    fn handle_table_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page != Page::Library || !self.focus_handle.is_focused(window) {
            return;
        }

        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return;
        }

        let handled = match event.keystroke.key.as_str() {
            "pageup" => {
                self.move_selection_by_page(-1);
                true
            }
            "pagedown" => {
                self.move_selection_by_page(1);
                true
            }
            "home" => {
                self.select_table_edge(false);
                true
            }
            "end" => {
                self.select_table_edge(true);
                true
            }
            _ => Self::album_jump_key(event)
                .map(|jump_key| self.scroll_to_album_initial(jump_key))
                .unwrap_or(false),
        };

        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn move_selection(&mut self, delta: isize) {
        self.move_selection_by_rows(delta, ScrollStrategy::Center);
    }

    fn move_selection_by_page(&mut self, direction: isize) {
        let rows = self.table_page_row_count() as isize;
        self.move_selection_by_rows(direction * rows, ScrollStrategy::Center);
    }

    fn move_selection_by_rows(&mut self, delta: isize, strategy: ScrollStrategy) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let selected_track = self.active_selected_track();
        let Some(position) = indices.iter().position(|ix| *ix == selected_track) else {
            return;
        };
        let next = (position as isize + delta).clamp(0, indices.len().saturating_sub(1) as isize);
        self.select_table_row(next as usize, strategy);
    }

    fn select_table_edge(&mut self, end: bool) {
        let Some(last_row) = self.current_track_indices().len().checked_sub(1) else {
            return;
        };

        if end {
            self.select_table_row(last_row, ScrollStrategy::Bottom);
        } else {
            self.select_table_row(0, ScrollStrategy::Top);
        }
    }

    fn select_table_row(&mut self, row_ix: usize, strategy: ScrollStrategy) {
        let Some(track_ix) = self.current_track_indices().get(row_ix).copied() else {
            return;
        };

        self.set_active_selected_track(track_ix);
        self.active_tab()
            .table_scroll_handle
            .scroll_to_item(row_ix, strategy);
        self.context_menu_track = None;
    }

    fn table_page_row_count(&self) -> usize {
        self.table_scrollbar_metrics()
            .map(|metrics| (metrics.track_height / TABLE_ROW_H).floor() as usize)
            .unwrap_or(12)
            .saturating_sub(1)
            .max(1)
    }

    fn album_jump_key(event: &KeyDownEvent) -> Option<char> {
        let key_char = event.keystroke.key_char.as_deref()?;
        let mut chars = key_char.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }

        ch.is_ascii_alphanumeric().then(|| ch.to_ascii_uppercase())
    }

    fn scroll_to_album_initial(&mut self, jump_key: char) -> bool {
        let Some(row_ix) = self.current_track_indices().iter().position(|track_ix| {
            self.tracks
                .get(*track_ix)
                .is_some_and(|track| Self::album_initial_matches(&track.album, jump_key))
        }) else {
            return false;
        };

        self.select_table_row(row_ix, ScrollStrategy::Top);
        true
    }

    fn album_initial_matches(album: &str, jump_key: char) -> bool {
        album
            .chars()
            .find(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_uppercase() == jump_key)
            .unwrap_or(false)
    }

    fn table_scrollbar_metrics(&self) -> Option<TableScrollbarMetrics> {
        let handle = self.active_tab().table_scroll_handle.clone();
        let (base_handle, measured) = {
            let state = handle.0.borrow();
            (state.base_handle.clone(), state.last_item_size.is_some())
        };

        if !measured {
            return None;
        }

        let bounds = base_handle.bounds();
        let viewport_height = f32::from(bounds.size.height);
        if viewport_height <= 0.0 {
            return None;
        }

        let max_scroll = f32::from(base_handle.max_offset().height).max(0.0);
        let content_height = viewport_height + max_scroll;
        let track_height = (viewport_height - TABLE_SCROLLBAR_MARGIN * 2.0).max(1.0);
        let thumb_height = if content_height <= 0.0 {
            track_height
        } else {
            ((viewport_height / content_height) * track_height)
                .max(TABLE_SCROLLBAR_MIN_THUMB_H)
                .min(track_height)
        };
        let thumb_travel = (track_height - thumb_height).max(0.0);
        let scroll_top = (-f32::from(base_handle.offset().y)).clamp(0.0, max_scroll);
        let thumb_top = if max_scroll > 0.0 && thumb_travel > 0.0 {
            (scroll_top / max_scroll) * thumb_travel
        } else {
            0.0
        };

        Some(TableScrollbarMetrics {
            track_top: f32::from(bounds.origin.y) + TABLE_SCROLLBAR_MARGIN,
            track_height,
            thumb_top,
            thumb_height,
            max_scroll,
            scroll_top,
        })
    }

    fn begin_table_scrollbar_drag(&mut self, event: &MouseDownEvent) -> bool {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let local_y = f32::from(event.position.y) - metrics.track_top;
        let thumb_bottom = metrics.thumb_top + metrics.thumb_height;
        let thumb_offset = if (metrics.thumb_top..=thumb_bottom).contains(&local_y) {
            local_y - metrics.thumb_top
        } else {
            metrics.thumb_height / 2.0
        };

        self.table_scrollbar_drag = Some(TableScrollbarDrag { thumb_offset });
        self.scroll_table_to_scrollbar_y(event.position.y, thumb_offset);
        true
    }

    fn drag_table_scrollbar(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(drag) = self.table_scrollbar_drag else {
            return false;
        };

        if !event.dragging() {
            self.table_scrollbar_drag = None;
            return false;
        }

        self.scroll_table_to_scrollbar_y(event.position.y, drag.thumb_offset)
    }

    fn finish_table_scrollbar_drag(&mut self) -> bool {
        self.table_scrollbar_drag.take().is_some()
    }

    fn finish_table_drag_interactions(&mut self) -> bool {
        let scrolled = self.finish_table_scrollbar_drag();
        let resized = self.finish_column_resize();
        scrolled || resized
    }

    fn mark_table_scrolling(&mut self, cx: &mut Context<Self>) {
        self.table_scroll_generation = self.table_scroll_generation.wrapping_add(1);

        if self.table_is_scrolling {
            return;
        }

        self.table_is_scrolling = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            loop {
                let Ok(generation) = this.update(cx, |app, _cx| app.table_scroll_generation) else {
                    return;
                };

                cx.background_executor()
                    .timer(TABLE_SCROLL_IDLE_DELAY)
                    .await;

                let Ok(should_stop) = this.update(cx, |app, cx| {
                    if app.table_scroll_generation == generation && app.table_is_scrolling {
                        app.table_is_scrolling = false;
                        cx.notify();
                        true
                    } else {
                        false
                    }
                }) else {
                    return;
                };

                if should_stop {
                    return;
                }
            }
        })
        .detach();
    }

    fn scroll_table_to_scrollbar_y(&mut self, mouse_y: Pixels, thumb_offset: f32) -> bool {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let thumb_travel = (metrics.track_height - metrics.thumb_height).max(1.0);
        let thumb_top =
            (f32::from(mouse_y) - metrics.track_top - thumb_offset).clamp(0.0, thumb_travel);
        let ratio = thumb_top / thumb_travel;
        self.scroll_table_to_ratio(ratio)
    }

    fn scroll_table_to_ratio(&mut self, ratio: f32) -> bool {
        let handle = self.active_tab().table_scroll_handle.clone();
        let base_handle = handle.0.borrow().base_handle.clone();
        let max_scroll = f32::from(base_handle.max_offset().height).max(0.0);
        if max_scroll <= 0.0 {
            return false;
        }

        let target_y = px(-(ratio.clamp(0.0, 1.0) * max_scroll));
        let current = base_handle.offset();
        if (f32::from(current.y) - f32::from(target_y)).abs() < 0.5 {
            return false;
        }

        base_handle.set_offset(point(current.x, target_y));
        true
    }

    fn current_scrollbar_label(&self, metrics: TableScrollbarMetrics) -> Option<String> {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return None;
        }

        let ratio = if metrics.max_scroll > 0.0 {
            metrics.scroll_top / metrics.max_scroll
        } else {
            0.0
        };
        let row_ix =
            (ratio.clamp(0.0, 1.0) * indices.len().saturating_sub(1) as f32).round() as usize;
        let track_ix = *indices.get(row_ix)?;
        Some(self.scrollbar_marker_label(track_ix, self.active_tab().sort_column))
    }

    fn fast_scroll_top_row(&self) -> (usize, f32) {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return (0, 0.0);
        };

        let scroll_top = metrics.scroll_top.max(0.0);
        let row = (scroll_top / TABLE_ROW_H).floor() as usize;
        let offset = -(scroll_top % TABLE_ROW_H);
        (row, offset)
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Library => div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(self.render_library(cx))
                .child(self.render_queue(cx))
                .into_any_element(),
            Page::Settings => self.render_settings(cx).into_any_element(),
        }
    }

    fn render_left_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.left_sidebar_collapsed;

        if collapsed {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

        div()
            .w(px(LEFT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x15161a))
            .child(
                div()
                    .w(px(LEFT_SIDEBAR_W))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.render_sidebar_header(cx))
                    .child(self.render_library_nav(cx))
                    .child(self.render_playlists_nav(cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(0x24252b))
                            .text_xs()
                            .text_color(rgb(0x6f737c))
                            .flex()
                            .justify_between()
                            .child(format!("{} tracks", self.tracks.len()))
                            .child(Self::format_library_size(&self.tracks)),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(50.0))
            .flex()
            .items_center()
            .px_4()
            .border_b_1()
            .border_color(rgb(0x1e2026))
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xf0f0f4))
                    .child("Tempo"),
            )
            .child(
                Self::sidebar_button("‹", "toggle-left-sidebar").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.left_sidebar_collapsed = !this.left_sidebar_collapsed;
                        cx.notify();
                    },
                )),
            )
    }

    fn sidebar_button(label: &'static str, id: &'static str) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x30323a))
            .bg(rgb(0x1b1c22))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x9a9ea8))
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    fn render_library_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(Self::nav_group_title("LIBRARY"))
            .child(self.render_nav_item(
                "All Music",
                self.tracks.len().to_string(),
                self.page == Page::Library && self.active_tab().source == TabSource::Library,
                Page::Library,
                cx,
            ))
            .child(self.render_nav_item(
                "Settings",
                "",
                self.page == Page::Settings,
                Page::Settings,
                cx,
            ))
    }

    fn render_playlists_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(Self::nav_group_title("PLAYLISTS"))
                    .child(
                        Self::sidebar_button("+", "new-playlist").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .when(self.playlists.is_empty(), |this| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(0x777b84))
                        .child("No playlists yet"),
                )
            })
            .children(
                self.playlists
                    .iter()
                    .enumerate()
                    .map(|(ix, playlist)| self.render_playlist_nav_item(ix, playlist, cx)),
            )
    }

    fn nav_group_title(title: &'static str) -> impl IntoElement {
        div()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(0x666a73))
            .child(title)
    }

    fn render_playlist_nav_item(
        &self,
        ix: usize,
        playlist: &Playlist,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active =
            self.page == Page::Library && self.active_tab().source == TabSource::Playlist(ix);
        let bg = if active { 0x282a30 } else { 0x15161a };
        let fg = if active { 0xf0f0f4 } else { 0xb6b8bf };

        div()
            .id(SharedString::from(format!("playlist-{ix}")))
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .active(|this| this.opacity(0.82))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(playlist.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(playlist.track_paths.len().to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_playlist_tab(ix);
                cx.notify();
            }))
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                this.add_track_to_playlist(drag.track_ix, ix);
                cx.notify();
            }))
    }

    fn render_nav_item(
        &self,
        label: &'static str,
        count: impl Into<SharedString>,
        active: bool,
        target: Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bg = if active { 0x282a30 } else { 0x15161a };
        let fg = if active { 0xf0f0f4 } else { 0xb6b8bf };

        div()
            .id(SharedString::from(format!("nav-{label}")))
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .active(|this| this.opacity(0.82))
            .child(label)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(count.into()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if target == Page::Library {
                    this.open_all_music_tab();
                } else {
                    this.open_page(target);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_library(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(0x131419))
            .child(self.render_library_header(cx))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .child(self.render_table(cx))
            .when(
                self.show_scan_errors && !self.scan_errors.is_empty(),
                |this| this.child(self.render_scan_errors_popover(cx)),
            )
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .h(px(34.0))
            .flex_none()
            .px_3()
            .border_b_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x111216))
            .flex()
            .items_end()
            .gap_1()
            .children(
                self.tabs
                    .iter()
                    .enumerate()
                    .map(|(ix, tab)| self.render_tab(ix, tab, cx)),
            )
            .child(
                div()
                    .id("new-tab-button")
                    .mb(px(5.0))
                    .w(px(24.0))
                    .h(px(22.0))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x30323a))
                    .bg(rgb(0x18191f))
                    .text_color(rgb(0x9a9ea8))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_library_tab();
                        window.focus(&this.search_focus_handle);
                        cx.notify();
                    })),
            )
    }

    fn render_tab(
        &self,
        ix: usize,
        tab: &BrowseTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = ix == self.active_tab;
        let bg = if active { 0x1b1c22 } else { 0x15161a };
        let fg = if active { 0xf0f0f4 } else { 0x9a9ea8 };
        let border = if active { 0x3a3d45 } else { 0x24252b };

        div()
            .id(SharedString::from(format!("browse-tab-{ix}")))
            .max_w(px(210.0))
            .h(px(28.0))
            .px_3()
            .rounded_t_md()
            .border_1()
            .border_b_0()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .active(|this| this.opacity(0.82))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.tab_title(tab)),
            )
            .when(!tab.search_query.trim().is_empty(), |this| {
                this.child(div().text_xs().text_color(rgb(0xeeb17d)).child("search"))
            })
            .when(self.tabs.len() > 1, |this| {
                this.child(
                    div()
                        .id(SharedString::from(format!("close-tab-{ix}")))
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_sm()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x777b84))
                        .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf0f0f4)))
                        .child("x")
                        .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                            this.close_tab(ix);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
            .when_some(
                match tab.source {
                    TabSource::Playlist(playlist_ix) => Some(playlist_ix),
                    TabSource::Library => None,
                },
                |this, playlist_ix| {
                    this.on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                        this.add_track_to_playlist(drag.track_ix, playlist_ix);
                        cx.notify();
                    }))
                },
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.active_tab = ix;
                this.open_page(Page::Library);
                cx.notify();
            }))
    }

    fn render_library_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(54.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(rgb(0x24252b))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(self.left_sidebar_collapsed, |this| {
                        this.child(Self::sidebar_button("›", "open-left-sidebar").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.left_sidebar_collapsed = false;
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xf0f0f4))
                            .child(self.tab_title(self.active_tab())),
                    ),
            )
            .child(self.render_scan_status(cx))
            .child(div().flex_1())
            .child(
                div()
                    .id("library-search")
                    .w(px(180.0))
                    .h(px(26.0))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x30323a))
                    .bg(rgb(0x18191f))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(if self.active_search_query().is_empty() {
                        0x737781
                    } else {
                        0xd8d8dd
                    }))
                    .track_focus(&self.search_focus_handle)
                    .on_click(cx.listener(|this, _, window, _cx| {
                        window.focus(&this.search_focus_handle);
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        this.handle_search_key_down(event, cx);
                    }))
                    .child(if self.active_search_query().is_empty() {
                        "⌕  Search library".into()
                    } else {
                        format!("⌕  {}", self.active_search_query())
                    }),
            )
            .child(
                Self::sidebar_button("⚙", "open-settings").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.open_page(Page::Settings);
                        cx.notify();
                    },
                )),
            )
            .when(
                self.right_sidebar_collapsed && !self.queue.is_empty(),
                |this| {
                    this.child(Self::sidebar_button("‹", "open-right-sidebar").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.right_sidebar_collapsed = false;
                            cx.notify();
                        }),
                    ))
                },
            )
    }

    fn render_scan_status(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .text_xs()
            .text_color(rgb(if self.is_scanning { 0xeeb17d } else { 0x676b74 }))
            .flex()
            .items_center()
            .gap_1()
            .child(self.visible_scan_status_without_errors())
            .when(self.scan_progress.errors > 0, |this| {
                let label = format!(
                    "{} {}",
                    self.scan_progress.errors,
                    if self.scan_progress.errors == 1 {
                        "error"
                    } else {
                        "errors"
                    }
                );

                this.child(
                    div()
                        .id("scan-errors-toggle")
                        .rounded_sm()
                        .px_1()
                        .cursor_pointer()
                        .text_color(rgb(0xeeb17d))
                        .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf2c693)))
                        .child(label)
                        .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.show_scan_errors = !this.show_scan_errors;
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
    }

    fn render_scan_errors_popover(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let errors = self
            .scan_errors
            .iter()
            .rev()
            .take(10)
            .map(|error| {
                div()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x282a30))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(rgb(0xf0f0f4))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(error.path.display().to_string()),
                    )
                    .child(div().text_color(rgb(0xa1a5af)).child(error.message.clone()))
            })
            .collect::<Vec<_>>();
        let hidden_count = self.scan_errors.len().saturating_sub(errors.len());

        div()
            .absolute()
            .top(px(46.0))
            .right(px(16.0))
            .w(px(420.0))
            .max_h(px(360.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343741))
            .bg(rgb(0x1b1c22))
            .shadow_lg()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xf0f0f4))
                            .child("Scan errors"),
                    )
                    .child(div().flex_1())
                    .child(
                        Self::sidebar_button("x", "close-scan-errors").on_click(cx.listener(
                            |this, _event: &ClickEvent, _window, cx| {
                                this.show_scan_errors = false;
                                cx.stop_propagation();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x8a8e97))
                    .child("Most recent errors are shown first."),
            )
            .children(errors)
            .when(hidden_count > 0, |this| {
                this.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(rgb(0x8a8e97))
                        .child(format!("+{hidden_count} older errors")),
                )
            })
    }

    fn render_table(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let item_count = self.current_track_indices().len();
        let active_search_query = self.active_search_query().to_string();
        let has_no_search_results = item_count == 0
            && self.active_source_track_count() > 0
            && !active_search_query.trim().is_empty();
        let table_scroll_handle = self.active_tab().table_scroll_handle.clone();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| {
                    window.focus(&this.focus_handle);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let scrolled = this.drag_table_scrollbar(event);
                let resized = !scrolled && this.resize_column_from_mouse(event);
                if scrolled || resized {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_drag_interactions() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_drag_interactions() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(self.render_table_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when(self.table_scrollbar_drag.is_some(), |this| {
                        this.child(self.render_fast_scroll_rows())
                    })
                    .when(self.table_scrollbar_drag.is_none(), |this| {
                        this.child(
                            uniform_list(
                                "track-table-rows",
                                item_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    range
                                        .enumerate()
                                        .filter_map(|(visible_row_ix, row_ix)| {
                                            let track_ix = this
                                                .current_track_indices()
                                                .get(row_ix)
                                                .copied()?;
                                            Some(
                                                this.render_track_row(
                                                    visible_row_ix,
                                                    track_ix,
                                                    &this.tracks[track_ix],
                                                    this.table_is_scrolling,
                                                    cx,
                                                )
                                                .into_any_element(),
                                            )
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .on_scroll_wheel(cx.listener(
                                |this, _event: &ScrollWheelEvent, _window, cx| {
                                    this.mark_table_scrolling(cx);
                                },
                            ))
                            .track_scroll(table_scroll_handle),
                        )
                    })
                    .child(self.render_table_scrollbar(item_count, cx)),
            )
            .when(self.tracks.is_empty(), |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(104.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(0x24252b))
                        .bg(rgb(0x17181e))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(0xf0f0f4))
                                .child("No indexed audio yet"),
                        )
                        .child(
                            div()
                                .text_color(rgb(0x8a8e97))
                                .child(self.library_status.clone()),
                        ),
                )
            })
            .when(has_no_search_results, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(104.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(0x24252b))
                        .bg(rgb(0x17181e))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(0xf0f0f4))
                                .child("No matching tracks"),
                        )
                        .child(
                            div()
                                .text_color(rgb(0x8a8e97))
                                .child(format!("No tracks match \"{}\".", active_search_query)),
                        ),
                )
            })
            .when_some(
                self.context_menu_track
                    .filter(|track_ix| *track_ix < self.tracks.len()),
                |this, track_ix| this.child(self.render_context_menu(track_ix, cx)),
            )
    }

    fn render_fast_scroll_rows(&self) -> AnyElement {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return div().size_full().into_any_element();
        };

        let indices = self.current_track_indices();
        if indices.is_empty() {
            return div().size_full().into_any_element();
        }

        let viewport_height = metrics.track_height + TABLE_SCROLLBAR_MARGIN * 2.0;
        let visible_rows = (viewport_height / TABLE_ROW_H).ceil() as usize + 2;
        let (top_row, row_offset) = self.fast_scroll_top_row();
        let start_row = top_row.saturating_sub(FAST_SCROLL_OVERSCAN_ROWS);
        let rows_to_render = visible_rows + FAST_SCROLL_OVERSCAN_ROWS * 2;
        let first_row_top = row_offset - ((top_row - start_row) as f32 * TABLE_ROW_H);
        let rows = (0..rows_to_render)
            .filter_map(|visible_ix| {
                let row_ix = start_row + visible_ix;
                let track_ix = *indices.get(row_ix)?;
                let top = first_row_top + visible_ix as f32 * TABLE_ROW_H;
                Some(self.render_fast_track_row(top, track_ix, &self.tracks[track_ix]))
            })
            .collect::<Vec<_>>();

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .overflow_hidden()
            .bg(rgb(0x131419))
            .children(rows)
            .into_any_element()
    }

    fn render_fast_track_row(&self, top: f32, track_ix: usize, track: &Track) -> AnyElement {
        let active = track_ix == self.playing_track;
        let selected = track_ix == self.active_selected_track();
        let bg = if selected {
            0x30323a
        } else if active {
            0x25262c
        } else {
            0x131419
        };
        let title_color = if active { 0xeeb17d } else { 0xe2e2e7 };

        div()
            .absolute()
            .top(px(top))
            .left_0()
            .right_0()
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(0x202127))
            .bg(rgb(bg))
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Index)))
                    .text_xs()
                    .text_color(rgb(0x6d717a))
                    .child(if active {
                        if self.is_playing { "Ⅱ" } else { "▶" }.into()
                    } else {
                        format!("{:02}", track_ix + 1)
                    }),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Artwork)))
                    .flex()
                    .items_center()
                    .child(Self::album_tile_placeholder(track, 22.0)),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Title)))
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(title_color))
                    .child(track.title.clone()),
            )
            .child(Self::cell(
                track.album.clone(),
                self.column_width(TableColumn::Album),
            ))
            .child(Self::cell(
                track.codec.clone(),
                self.column_width(TableColumn::Format),
            ))
            .child(Self::cell(
                track.plays.clone(),
                self.column_width(TableColumn::Plays),
            ))
            .child(Self::cell(
                track.duration.clone(),
                self.column_width(TableColumn::Duration),
            ))
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Loved)))
                    .text_color(rgb(0xf0b282))
                    .child(if track.loved { "♥" } else { "" }),
            )
            .into_any_element()
    }

    fn render_table_scrollbar(&self, item_count: usize, cx: &mut Context<Self>) -> AnyElement {
        if item_count == 0 {
            return div().into_any_element();
        }

        let metrics = self.table_scrollbar_metrics();
        let thumb_top = metrics.map_or(0.0, |metrics| metrics.thumb_top);
        let thumb_height =
            metrics.map_or(TABLE_SCROLLBAR_MIN_THUMB_H, |metrics| metrics.thumb_height);
        let track_height = metrics.map_or(0.0, |metrics| metrics.track_height);
        let scrollable = metrics.is_some_and(|metrics| metrics.max_scroll > 0.0);
        let current_label = metrics.and_then(|metrics| self.current_scrollbar_label(metrics));
        let max_markers = if track_height > 0.0 {
            ((track_height / 16.0).floor() as usize).clamp(2, TABLE_SCROLLBAR_MAX_MARKERS)
        } else {
            0
        };
        let marker_stride = self
            .active_tab()
            .scrollbar_markers
            .len()
            .saturating_add(max_markers.saturating_sub(1))
            / max_markers.max(1);
        let marker_stride = marker_stride.max(1);
        let markers = self
            .active_tab()
            .scrollbar_markers
            .iter()
            .enumerate()
            .filter_map(|(ix, marker)| {
                if ix % marker_stride != 0 {
                    return None;
                }

                let top = TABLE_SCROLLBAR_MARGIN + marker.ratio.clamp(0.0, 1.0) * track_height;
                Some(
                    div()
                        .absolute()
                        .top(px((top - 7.0).max(TABLE_SCROLLBAR_MARGIN)))
                        .right(px(14.0))
                        .w(px(30.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_xs()
                        .text_color(rgb(0x6f7480))
                        .child(marker.label.clone())
                        .into_any_element(),
                )
            })
            .collect::<Vec<_>>();
        let current_label_top = (TABLE_SCROLLBAR_MARGIN + thumb_top + thumb_height / 2.0 - 9.0)
            .clamp(
                TABLE_SCROLLBAR_MARGIN,
                (track_height - 18.0).max(TABLE_SCROLLBAR_MARGIN),
            );

        div()
            .id("table-scrollbar")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(TABLE_SCROLLBAR_W))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if this.begin_table_scrollbar_drag(event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag_table_scrollbar(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .children(markers)
            .child(
                div()
                    .absolute()
                    .top(px(TABLE_SCROLLBAR_MARGIN))
                    .right(px(4.0))
                    .bottom(px(TABLE_SCROLLBAR_MARGIN))
                    .w(px(TABLE_SCROLLBAR_TRACK_W))
                    .rounded_full()
                    .bg(rgb(0x1d1f26))
                    .opacity(if scrollable { 0.95 } else { 0.45 })
                    .child(
                        div()
                            .absolute()
                            .top(px(thumb_top))
                            .left(px(1.0))
                            .right(px(1.0))
                            .h(px(thumb_height))
                            .rounded_full()
                            .bg(rgb(if self.table_scrollbar_drag.is_some() {
                                0xb2b8c4
                            } else {
                                0x767b86
                            })),
                    ),
            )
            .when(scrollable, |this| {
                this.when_some(current_label, |this, label| {
                    this.child(
                        div()
                            .absolute()
                            .top(px(current_label_top))
                            .right(px(15.0))
                            .max_w(px(36.0))
                            .h(px(18.0))
                            .px_1()
                            .rounded_sm()
                            .bg(rgb(0x252832))
                            .border_1()
                            .border_color(rgb(0x3c414d))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(rgb(0xd3d6df))
                            .overflow_hidden()
                            .child(label),
                    )
                })
            })
            .into_any_element()
    }

    fn render_table_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .h(px(27.0))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(0x24252b))
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(0x5f636c))
            .child(self.header_cell("#", TableColumn::Index, Some(SortColumn::Index), cx))
            .child(self.header_cell("", TableColumn::Artwork, None, cx))
            .child(self.header_cell("TITLE", TableColumn::Title, Some(SortColumn::Title), cx))
            .child(self.header_cell("ALBUM", TableColumn::Album, Some(SortColumn::Album), cx))
            .child(self.header_cell("FMT", TableColumn::Format, Some(SortColumn::Format), cx))
            .child(self.header_cell("PLAYS", TableColumn::Plays, Some(SortColumn::Plays), cx))
            .child(self.header_cell(
                "TIME",
                TableColumn::Duration,
                Some(SortColumn::Duration),
                cx,
            ))
            .child(self.header_cell("", TableColumn::Loved, None, cx))
    }

    fn header_cell(
        &self,
        label: &'static str,
        column: TableColumn,
        sort_column: Option<SortColumn>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let width = self.column_width(column);
        let tab = self.active_tab();
        let active = sort_column.is_some_and(|column| tab.sort_column == column);
        let icon = match tab.sort_direction {
            SortDirection::Ascending => "▲",
            SortDirection::Descending => "▼",
        };
        let id = match column {
            TableColumn::Index => "column-index",
            TableColumn::Artwork => "column-artwork",
            TableColumn::Title => "column-title",
            TableColumn::Album => "column-album",
            TableColumn::Format => "column-format",
            TableColumn::Plays => "column-plays",
            TableColumn::Duration => "column-duration",
            TableColumn::Loved => "column-loved",
        };

        div()
            .id(id)
            .relative()
            .h_full()
            .w(px(width))
            .flex()
            .items_center()
            .gap_1()
            .text_color(rgb(if active { 0xc9ccd4 } else { 0x5f636c }))
            .when(sort_column.is_some(), |this| {
                this.cursor_pointer()
                    .hover(|this| this.text_color(rgb(0xc9ccd4)))
            })
            .child(label)
            .when(active, |this| this.child(icon))
            .when_some(sort_column, |this, sort_column| {
                this.on_click(cx.listener(move |this, _, _, cx| {
                    let tab = this.active_tab_mut();
                    if tab.sort_column == sort_column {
                        tab.sort_direction = match tab.sort_direction {
                            SortDirection::Ascending => SortDirection::Descending,
                            SortDirection::Descending => SortDirection::Ascending,
                        };
                    } else {
                        tab.sort_column = sort_column;
                        tab.sort_direction = SortDirection::Ascending;
                    }

                    this.invalidate_track_indices();
                    cx.notify();
                }))
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(6.0))
                    .cursor(CursorStyle::ResizeColumn)
                    .hover(|this| this.bg(rgb(0x343741)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.begin_column_resize(column, event);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
    }

    fn format_library_size(tracks: &[Track]) -> String {
        let bytes = tracks.iter().map(|track| track.file_size).sum::<u64>();
        if bytes >= 1_000_000_000 {
            format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
        } else if bytes >= 1_000_000 {
            format!("{:.1} MB", bytes as f64 / 1_000_000.0)
        } else {
            format!("{} KB", bytes / 1_000)
        }
    }

    fn cached_waveform(&mut self, track_ix: usize, cx: &mut Context<Self>) -> (Vec<f32>, bool) {
        if self.waveform_cache.len() < self.tracks.len() {
            self.waveform_cache.resize_with(self.tracks.len(), || None);
        }
        if self.waveform_loading.len() < self.tracks.len() {
            self.waveform_loading.resize(self.tracks.len(), false);
        }

        if let Some(waveform) = self.waveform_cache[track_ix].as_ref() {
            return (waveform.clone(), self.waveform_loading[track_ix]);
        }

        let source = WaveformSource::from_track(&self.tracks[track_ix]);

        if !self.waveform_loading[track_ix] {
            self.waveform_loading[track_ix] = true;
            let expected_path = source.path.clone();
            cx.spawn(async move |this, cx| {
                let waveform = cx
                    .background_executor()
                    .spawn(async move { TempoApp::generate_audio_waveform(&source) })
                    .await;

                let _ = this.update(cx, |app, cx| {
                    if app
                        .tracks
                        .get(track_ix)
                        .is_some_and(|track| track.path == expected_path)
                    {
                        app.waveform_cache[track_ix] = Some(waveform);
                        if track_ix < app.waveform_loading.len() {
                            app.waveform_loading[track_ix] = false;
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }

        (
            Self::generate_loading_waveform(Self::waveform_loading_phase()),
            true,
        )
    }

    fn generate_audio_waveform(track: &WaveformSource) -> Vec<f32> {
        Self::decode_waveform(track).unwrap_or_else(|| Self::generate_fallback_waveform(track))
    }

    fn decode_waveform(track: &WaveformSource) -> Option<Vec<f32>> {
        let file = fs::File::open(&track.path).ok()?;
        let mut decoder = Decoder::try_from(file).ok()?;
        let duration = decoder.total_duration().unwrap_or(track.duration_value);
        let sample_rate = decoder.sample_rate().get() as f64;
        let channels = decoder.channels().get() as f64;
        let total_samples = (duration.as_secs_f64() * sample_rate * channels)
            .ceil()
            .max(1.0) as usize;

        if total_samples == 0 {
            return None;
        }

        let mut peaks = vec![0.0_f32; WAVEFORM_SEGMENTS];
        let mut saw_sample = false;
        let samples_per_bin = (total_samples / WAVEFORM_SEGMENTS).max(1);
        let mut bin = 0;
        let mut next_bin_sample = samples_per_bin;

        for (sample_ix, sample) in decoder.by_ref().enumerate() {
            while sample_ix >= next_bin_sample && bin < WAVEFORM_SEGMENTS - 1 {
                bin += 1;
                next_bin_sample = next_bin_sample.saturating_add(samples_per_bin);
            }

            peaks[bin] = peaks[bin].max(sample.abs());
            saw_sample = true;
        }

        if !saw_sample {
            return None;
        }

        let max_peak = peaks.iter().copied().fold(0.0_f32, f32::max).max(0.001);
        Some(
            peaks
                .into_iter()
                .map(|peak| 8.0 + (peak / max_peak).sqrt() * 50.0)
                .collect(),
        )
    }

    fn generate_fallback_waveform(track: &WaveformSource) -> Vec<f32> {
        let mut seed = 0xcbf29ce484222325_u64;

        for part in [&track.title, &track.artist, &track.album, &track.duration] {
            for byte in part.bytes() {
                seed ^= byte as u64;
                seed = seed.wrapping_mul(0x100000001b3);
            }
        }

        let pulse_count = 3.0 + (track.title.len() % 5) as f32;
        let mut previous = 0.38;

        (0..WAVEFORM_SEGMENTS)
            .map(|ix| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);

                let noise = ((seed >> 33) as f32) / ((1_u64 << 31) as f32);
                let position = ix as f32 / WAVEFORM_SEGMENTS as f32;
                let pulse = (position * std::f32::consts::TAU * pulse_count).sin().abs();
                let target = (0.16 + noise * 0.5 + pulse * 0.34).min(1.0);

                previous = previous * 0.66 + target * 0.34;
                8.0 + previous * 50.0
            })
            .collect()
    }

    fn generate_loading_waveform(phase: f32) -> Vec<f32> {
        (0..WAVEFORM_SEGMENTS)
            .map(|ix| {
                let position = ix as f32 / 12.0;
                let sweep = ((position - phase).sin() + 1.0) * 0.5;
                let ripple = ((position * 0.35 + phase * 0.6).sin() + 1.0) * 0.5;
                (10.0 + (sweep * 0.7 + ripple * 0.3) * 42.0).round()
            })
            .collect()
    }

    fn waveform_loading_phase() -> f32 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as f32 / 90.0)
            .unwrap_or_default()
    }

    fn render_track_row(
        &self,
        row_ix: usize,
        track_ix: usize,
        track: &Track,
        lightweight: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = track_ix == self.playing_track;
        let selected = track_ix == self.active_selected_track();
        let bg = if selected {
            0x30323a
        } else if active {
            0x25262c
        } else {
            0x131419
        };
        let title_color = if active { 0xeeb17d } else { 0xe2e2e7 };

        div()
            .id(SharedString::from(format!("track-row-{track_ix}")))
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(0x202127))
            .bg(rgb(bg))
            .when(!lightweight, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(rgb(0x202229)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        window.focus(&this.focus_handle);
                        this.set_active_selected_track(track_ix);
                        this.context_menu_track = None;

                        if event.standard_click() && event.modifiers().control {
                            this.queue_track(track_ix);
                            cx.notify();
                            return;
                        }

                        if event.standard_click() && event.click_count() >= 2 {
                            this.play_track(track_ix);
                        }

                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle);
                            this.set_active_selected_track(track_ix);
                            this.context_menu_track = Some(track_ix);
                            this.context_menu_row = row_ix;
                            cx.notify();
                        }),
                    )
                    .on_drag(
                        TrackDrag::new(track_ix, track),
                        |drag: &TrackDrag, position, _, cx| {
                            let preview = drag.clone().position(position);
                            cx.new(|_| preview)
                        },
                    )
            })
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Index)))
                    .text_xs()
                    .text_color(rgb(0x6d717a))
                    .child(if active {
                        if self.is_playing { "Ⅱ" } else { "▶" }.into()
                    } else {
                        format!("{:02}", track_ix + 1)
                    }),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Artwork)))
                    .flex()
                    .items_center()
                    .child(if lightweight {
                        Self::album_tile_placeholder(track, 22.0)
                    } else {
                        Self::album_tile(track, 22.0)
                    }),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Title)))
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(title_color))
                    .child(track.title.clone()),
            )
            .child(Self::cell(
                track.album.clone(),
                self.column_width(TableColumn::Album),
            ))
            .child(Self::cell(
                track.codec.clone(),
                self.column_width(TableColumn::Format),
            ))
            .child(Self::cell(
                track.plays.clone(),
                self.column_width(TableColumn::Plays),
            ))
            .child(Self::cell(
                track.duration.clone(),
                self.column_width(TableColumn::Duration),
            ))
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Loved)))
                    .text_color(rgb(0xf0b282))
                    .child(if track.loved { "♥" } else { "" }),
            )
    }

    fn cell(content: impl Into<SharedString>, width: f32) -> impl IntoElement {
        div()
            .w(px(width))
            .overflow_hidden()
            .text_ellipsis()
            .text_color(rgb(0x8a8e97))
            .child(content.into())
    }

    fn render_context_menu(
        &self,
        track_ix: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let track = &self.tracks[track_ix];
        let top = 27.0 + ((self.context_menu_row as f32 + 1.0) * TABLE_ROW_H).min(560.0);

        div()
            .absolute()
            .top(px(top))
            .left(px(76.0))
            .w(px(190.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x343741))
            .bg(rgb(0x1b1c22))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x2b2d35))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xf0f0f4))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(track.title.clone()),
            )
            .child(
                Self::context_menu_item("Play from start").on_click(cx.listener(
                    move |this, _, _, cx| {
                        if track_ix < this.tracks.len() {
                            this.play_track(track_ix);
                            cx.notify();
                        }
                    },
                )),
            )
            .child(
                Self::context_menu_item("Add to queue").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.queue_track(track_ix);
                        cx.notify();
                    },
                )),
            )
            .child(Self::context_menu_item("Queue Album").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.queue_album_from_track(track_ix, false);
                    cx.notify();
                },
            )))
            .child(
                Self::context_menu_item("Queue Album Shuffled").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.queue_album_from_track(track_ix, true);
                        cx.notify();
                    },
                )),
            )
            .when(!self.playlists.is_empty(), |this| {
                this.child(
                    div()
                        .mt_1()
                        .px_3()
                        .pt_2()
                        .pb_1()
                        .border_t_1()
                        .border_color(rgb(0x2b2d35))
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(0x777b84))
                        .child("ADD TO PLAYLIST"),
                )
                .children(
                    self.playlists
                        .iter()
                        .enumerate()
                        .map(|(playlist_ix, playlist)| {
                            Self::context_menu_item_dynamic(format!("Add to {}", playlist.name))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.add_track_to_playlist(track_ix, playlist_ix);
                                    cx.notify();
                                }))
                        }),
                )
            })
            .child(Self::context_menu_item("Go to album"))
            .child(Self::context_menu_item("Show file"))
    }

    fn context_menu_item(label: &'static str) -> gpui::Stateful<gpui::Div> {
        div()
            .id(SharedString::from(format!("context-menu-{label}")))
            .h(px(28.0))
            .px_3()
            .flex()
            .items_center()
            .cursor_pointer()
            .text_color(rgb(0xc9ccd4))
            .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf0f0f4)))
            .child(label)
    }

    fn context_menu_item_dynamic(label: String) -> gpui::Stateful<gpui::Div> {
        let id = SharedString::from(format!("context-menu-{label}"));

        div()
            .id(id)
            .h(px(28.0))
            .px_3()
            .flex()
            .items_center()
            .cursor_pointer()
            .text_color(rgb(0xc9ccd4))
            .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf0f0f4)))
            .child(label)
    }

    fn album_tile(track: &Track, size: f32) -> AnyElement {
        let initials = track.album_initials.clone();
        let color = track.album_color;
        let fallback_initials = initials.clone();

        div()
            .w(px(size))
            .h(px(size))
            .rounded_sm()
            .border_1()
            .border_color(rgb(0x3a3d45))
            .overflow_hidden()
            .child(match &track.artwork {
                Some(TrackArtwork::Embedded(image)) => img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Self::album_tile_fallback(fallback_initials.clone(), color)
                    })
                    .into_any_element(),
                Some(TrackArtwork::File(path)) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Self::album_tile_fallback(fallback_initials.clone(), color)
                    })
                    .into_any_element(),
                None => Self::album_tile_fallback(initials, color),
            })
            .into_any_element()
    }

    fn album_tile_placeholder(track: &Track, size: f32) -> AnyElement {
        div()
            .w(px(size))
            .h(px(size))
            .rounded_sm()
            .border_1()
            .border_color(rgb(0x3a3d45))
            .overflow_hidden()
            .child(Self::album_tile_fallback(
                track.album_initials.clone(),
                track.album_color,
            ))
            .into_any_element()
    }

    fn album_tile_fallback(initials: String, color: u32) -> AnyElement {
        div()
            .size_full()
            .bg(rgb(color))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(0xf4f0ea))
            .child(initials)
            .into_any_element()
    }

    fn album_initials_for(album: &str, title: &str) -> String {
        let source = if album == "Unknown Album" {
            title
        } else {
            album
        };

        let mut initials = source
            .split_whitespace()
            .filter_map(|word| word.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();

        if initials.is_empty() {
            initials.push('?');
        }

        initials
    }

    fn album_color_for(album: &str, artist: &str) -> u32 {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in album.bytes().chain(artist.bytes()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }

        let palette = [
            0x7b5735, 0x496777, 0x5b6b73, 0x7d6c48, 0x8c5f55, 0x55536f, 0x42685f, 0x744f6d,
        ];
        palette[(hash as usize) % palette.len()]
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.right_sidebar_collapsed;

        if collapsed || self.queue.is_empty() {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

        div()
            .w(px(RIGHT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_l_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x17161b))
            .child(
                div()
                    .w(px(RIGHT_SIDEBAR_W))
                    .h(px(54.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(0x24252b))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Self::sidebar_button("›", "toggle-right-sidebar").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.right_sidebar_collapsed = !this.right_sidebar_collapsed;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(0xf0f0f4))
                                    .child("Up Next"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x676b74))
                            .child(format!("{} tracks", self.queue.len())),
                    ),
            )
            .child(
                div().w(px(RIGHT_SIDEBAR_W)).children(
                    self.queue
                        .iter()
                        .filter(|track_ix| **track_ix < self.tracks.len())
                        .enumerate()
                        .map(|(ix, track_ix)| self.render_queue_row(ix, &self.tracks[*track_ix])),
                ),
            )
            .into_any_element()
    }

    fn render_queue_row(&self, ix: usize, track: &Track) -> impl IntoElement {
        let active = ix == 0;
        let bg = if active { 0x242329 } else { 0x17161b };

        div()
            .h(px(41.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(bg))
            .child(
                div()
                    .w(px(22.0))
                    .text_xs()
                    .text_color(rgb(0x70747d))
                    .child(format!("{}", ix + 1)),
            )
            .child(Self::album_tile(track, 28.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(if active { 0xeeb17d } else { 0xe2e2e7 }))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x878b94))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(track.artist.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(track.duration.clone()),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .bg(rgb(0x131419))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(54.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(0x24252b))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(self.left_sidebar_collapsed, |this| {
                                this.child(Self::sidebar_button("›", "open-left-sidebar").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.left_sidebar_collapsed = false;
                                        cx.notify();
                                    }),
                                ))
                            })
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(0xf0f0f4))
                                    .child(if self.library_roots.is_empty() {
                                        "Set Up Tempo"
                                    } else {
                                        "Settings"
                                    }),
                            ),
                    )
                    .when(!self.library_roots.is_empty(), |this| {
                        this.child(
                            div()
                                .id("settings-back")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x30323a))
                                .bg(rgb(0x1b1c22))
                                .active(|this| this.opacity(0.82))
                                .child("Back to Library")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_page(Page::Library);
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(self.library_roots.is_empty(), |this| {
                        this.child(self.render_onboarding_card())
                    })
                    .child(self.render_library_settings(cx))
                    .child(self.render_playlist_settings(cx)),
            )
    }

    fn render_onboarding_card(&self) -> impl IntoElement {
        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343741))
            .bg(rgb(0x1b1c22))
            .p_5()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xf0f0f4))
                    .child("Choose where Tempo should scan"),
            )
            .child(
                div()
                    .text_color(rgb(0xa6aab4))
                    .child("Add one or more music folders to start indexing your local library."),
            )
    }

    fn render_library_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x17181e))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x1b1c22))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Library"))
                    .child(
                        Self::settings_button("Add folder", "add-library-folder").on_click(
                            cx.listener(|_this, _, _window, cx| {
                                let paths = cx.prompt_for_paths(PathPromptOptions {
                                    files: false,
                                    directories: true,
                                    multiple: true,
                                    prompt: Some("Choose music folders".into()),
                                });

                                cx.spawn(async move |this, cx| {
                                    if let Ok(Ok(Some(paths))) = paths.await {
                                        let _ = this.update(cx, |app, cx| {
                                            app.add_library_roots(paths, cx);
                                            cx.notify();
                                        });
                                    }
                                })
                                .detach();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(0x24252b))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if self.is_scanning { 0xeeb17d } else { 0x858993 }))
                            .child(self.visible_scan_status()),
                    )
                    .when(self.library_roots.is_empty(), |this| {
                        this.child(div().text_color(rgb(0xc9ccd4)).child(
                            "No folders configured. Use Add folder to choose one or more roots.",
                        ))
                    })
                    .children(
                        self.library_roots
                            .iter()
                            .enumerate()
                            .map(|(ix, root)| self.render_library_root_row(ix, root, cx)),
                    )
                    .when(PathBuf::from("/mnt/data/music").is_dir(), |this| {
                        this.child(
                            Self::settings_button("Add /mnt/data/music", "add-mounted-music")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.add_library_roots(
                                        vec![PathBuf::from("/mnt/data/music")],
                                        cx,
                                    );
                                    cx.notify();
                                })),
                        )
                    }),
            )
    }

    fn render_library_root_row(
        &self,
        ix: usize,
        root: &PathBuf,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let root_label = root.display().to_string();
        div()
            .min_h(px(34.0))
            .px_3()
            .rounded_md()
            .bg(rgb(0x131419))
            .border_1()
            .border_color(rgb(0x24252b))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(rgb(0xd8d8dd))
                    .child(root_label),
            )
            .child(
                Self::settings_button("Remove", &format!("remove-library-root-{ix}")).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.remove_library_root(ix, cx);
                        cx.notify();
                    }),
                ),
            )
    }

    fn render_playlist_settings(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x17181e))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(0x1b1c22))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().font_weight(gpui::FontWeight::BOLD).child("Playlists"))
                    .child(
                        Self::settings_button("New playlist", "new-playlist-settings").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(0x24252b))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(self.playlists.is_empty(), |this| {
                        this.child(
                            div()
                                .text_color(rgb(0xc9ccd4))
                                .child("No playlists yet. Create one to start organizing tracks."),
                        )
                    })
                    .children(
                        self.playlists
                            .iter()
                            .enumerate()
                            .map(|(ix, playlist)| Self::render_playlist_settings_row(ix, playlist)),
                    ),
            )
    }

    fn render_playlist_settings_row(ix: usize, playlist: &Playlist) -> impl IntoElement + use<> {
        div()
            .min_h(px(34.0))
            .px_3()
            .rounded_md()
            .bg(rgb(0x131419))
            .border_1()
            .border_color(rgb(0x24252b))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(rgb(0xd8d8dd))
                    .child(playlist.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x858993))
                    .child(format!("{} tracks", playlist.track_paths.len())),
            )
            .id(SharedString::from(format!("settings-playlist-{ix}")))
    }

    fn settings_button(
        label: &'static str,
        id: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();

        div()
            .id(id)
            .cursor_pointer()
            .px_3()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x30323a))
            .bg(rgb(0x1b1c22))
            .text_color(rgb(0xc9ccd4))
            .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf0f0f4)))
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    fn render_player_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.tracks.is_empty() {
            return div()
                .h(px(86.0))
                .flex_none()
                .flex()
                .items_center()
                .gap_4()
                .px_4()
                .border_t_1()
                .border_color(rgb(0x282a30))
                .bg(rgb(0x18191e))
                .child(
                    div()
                        .w(px(54.0))
                        .h(px(54.0))
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(0x3a3d45))
                        .bg(rgb(0x25262c))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x777b84))
                        .child("♪"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(0xf0f0f4))
                                .child(if self.is_scanning {
                                    "Scanning library"
                                } else {
                                    "Library scanner idle"
                                }),
                        )
                        .child(
                            div()
                                .text_color(rgb(0x9a9ea8))
                                .child(self.visible_scan_status()),
                        ),
                )
                .into_any_element();
        }

        self.playing_track = self.playing_track.min(self.tracks.len() - 1);
        let (waveform, waveform_loading) = self.cached_waveform(self.playing_track, cx);
        if waveform_loading {
            window.request_animation_frame();
        }
        let playback_position = self.playback_position();
        let track = &self.tracks[self.playing_track];
        let playback_position = playback_position.min(track.duration_value);
        let playback_progress = if track.duration_value.is_zero() {
            0.0
        } else {
            (playback_position.as_secs_f32() / track.duration_value.as_secs_f32()).clamp(0.0, 1.0)
        };

        div()
            .h(px(86.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_t_1()
            .border_color(rgb(0x282a30))
            .bg(rgb(0x18191e))
            .child(Self::album_tile(track, 54.0))
            .child(
                div()
                    .w(px(220.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xf0f0f4))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(rgb(0x9a9ea8))
                            .child(format!("{} - {}", track.artist, track.album)),
                    )
                    .child(div().text_xs().text_color(rgb(0x70747d)).child(format!(
                        "{}  ·  {}  ·  {}  ·  {}",
                        track.codec,
                        Self::bitrate_label(track),
                        track.year,
                        self.playback_status.clone()
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .relative()
                    .child(Self::waveform_seekbar(
                        format_duration(playback_position),
                        track.duration.clone(),
                        playback_progress,
                        waveform,
                        waveform_loading,
                        cx,
                    )),
            )
            .child(
                div()
                    .w(px(170.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_color(rgb(0xa6aab4))
                    .child(Self::transport_overlay(self.is_playing, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child("☰")
                            .child("♩")
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(3.0))
                                    .rounded_full()
                                    .bg(rgb(0x777b84))
                                    .child(
                                        div()
                                            .w(px(104.0))
                                            .h(px(3.0))
                                            .rounded_full()
                                            .bg(rgb(0xd8d8dd)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn bitrate_label(track: &Track) -> String {
        track
            .bitrate
            .map(|bitrate| format!("{bitrate} kbps"))
            .unwrap_or_else(|| "unknown bitrate".to_string())
    }

    fn waveform_seekbar(
        elapsed: String,
        duration: String,
        progress: f32,
        waveform: Vec<f32>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let progress_segments = (waveform.len() as f32 * progress).round() as usize;

        div()
            .id("waveform-seekbar")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .cursor_pointer()
            .rounded_lg()
            .overflow_hidden()
            .bg(rgb(0x111218))
            .border_1()
            .border_color(rgb(0x30323a))
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                if event.standard_click() {
                    let click_x = f32::from(event.position().x);
                    let viewport_width = f32::from(window.viewport_size().width);
                    this.seek_from_waveform_click(click_x, viewport_width);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .absolute()
                    .top(px(42.0))
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(rgb(0x242833)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .children(waveform.into_iter().enumerate().map(move |(ix, height)| {
                        Self::waveform_bar(ix, height, progress_segments, loading)
                    })),
            )
            .when(loading, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_2()
                        .left_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(0x111218))
                        .text_xs()
                        .text_color(rgb(0x9bbdff))
                        .child("Loading waveform"),
                )
            })
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .left_3()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(0x111218))
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(elapsed),
            )
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .right_3()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(0x111218))
                    .text_xs()
                    .text_color(rgb(0x777b84))
                    .child(duration),
            )
    }

    fn waveform_bar(
        ix: usize,
        height: f32,
        progress_segments: usize,
        loading: bool,
    ) -> impl IntoElement {
        let played = ix < progress_segments;
        let playhead = ix == progress_segments;
        let peak = height > 44.0;
        let color = if loading && peak {
            0x6f9dff
        } else if loading {
            0x444a58
        } else if playhead {
            0xd7e5ff
        } else if played && peak {
            0x9bbdff
        } else if played {
            0x6f9dff
        } else if peak {
            0x555b69
        } else {
            0x383d49
        };

        div()
            .flex_1()
            .min_w(px(1.0))
            .h(px(if playhead { 58.0 } else { height }))
            .rounded_full()
            .bg(rgb(color))
            .opacity(if loading || played || playhead {
                1.0
            } else {
                0.78
            })
    }

    fn transport_overlay(is_playing: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_full()
            .bg(rgb(0x111216))
            .border_1()
            .border_color(rgb(0x30323a))
            .child(Self::transport_button("⌘", false))
            .child(
                Self::transport_button("◀", false).on_click(cx.listener(|this, _, _, cx| {
                    this.play_adjacent_track(-1);
                    cx.notify();
                })),
            )
            .child(
                Self::transport_button(if is_playing { "Ⅱ" } else { "▶" }, true).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.toggle_playback();
                        cx.notify();
                    }),
                ),
            )
            .child(
                Self::transport_button("▶", false).on_click(cx.listener(|this, _, _, cx| {
                    this.play_adjacent_track(1);
                    cx.notify();
                })),
            )
            .child(Self::transport_button("↻", false))
    }

    fn transport_button(label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
        let size = if primary { 28.0 } else { 22.0 };
        let hover_size = if primary { 32.0 } else { 26.0 };
        let bg = if primary { 0xe7e7ea } else { 0x18191e };
        let fg = if primary { 0x111216 } else { 0x9a9ea8 };

        div()
            .id(SharedString::from(format!("transport-{label}-{primary}")))
            .w(px(size))
            .h(px(size))
            .rounded_full()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .hover(move |this| {
                this.w(px(hover_size))
                    .h(px(hover_size))
                    .bg(rgb(0xf0f0f4))
                    .text_color(rgb(0x111216))
            })
            .child(label)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("tempo".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| TempoApp::new(window, cx)),
        )
        .expect("failed to open Tempo window");

        cx.bind_keys([
            KeyBinding::new("enter", PlaySelected, None),
            KeyBinding::new("space", TogglePause, None),
            KeyBinding::new("left", MoveSelectionUp, None),
            KeyBinding::new("right", MoveSelectionDown, None),
            KeyBinding::new("ctrl-t", NewTab, None),
            KeyBinding::new("ctrl-f", FocusSearch, None),
            KeyBinding::new("/", FocusSearch, None),
        ]);

        cx.activate(true);
    });
}
