use std::{
    env, fs,
    ops::Range,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use gpui::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, ClipboardItem, Context, Corner,
    CursorStyle, FocusHandle, Image, ImageFormat, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, ObjectFit, ParentElement,
    PathPromptOptions, Pixels, Point, Render, ScrollStrategy, ScrollWheelEvent, SharedString,
    Styled, UniformListScrollHandle, Window, anchored, div, img, point, prelude::*, px, rgb,
    uniform_list,
};
use rodio::{Decoder, Source as _};
use serde::{Deserialize, Serialize};
use tempo::{
    catalog::{
        CatalogAlbum, CatalogArtist, CatalogStore, CatalogTrack, individual_artist_names,
        primary_artist_name,
    },
    library::{
        Artwork as LibraryArtwork, IndexingError, LibraryEvent, LibraryIndexer, LibraryWatcher,
        ScanProgress,
    },
    playback::PlaybackController,
};

mod artwork;
mod browse_grids;
mod library_state;
mod library_view;
mod menu;
mod player;
mod search;
mod settings;
mod sidebar;
mod table;
mod text_input;
mod theme;
mod tooltip;

use crate::{
    CloseTab, FocusSearch, MoveSelectionDown, MoveSelectionUp, NavigateBack, NavigateForward,
    NewTab, OpenSettings, PlayRandomTrack, PlaySelected, TogglePause,
};
use text_input::TextInputState;
use theme::{Theme, ThemeColors, bundled_themes, default_theme_id, resolve_theme_id};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Library,
    Artists,
    Albums,
    ScanErrors,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowseViewMode {
    Grid,
    Table,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Index,
    Title,
    Artist,
    Album,
    TrackNumber,
    Format,
    Bitrate,
    FileSize,
    Year,
    DateAdded,
    Plays,
    Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum TableColumn {
    Index,
    Artwork,
    Title,
    Artist,
    Album,
    TrackNumber,
    Format,
    Bitrate,
    FileSize,
    Year,
    DateAdded,
    Plays,
    Duration,
    Loved,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaybackMode {
    Straight,
    Loop,
    Shuffle,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMenuSource {
    Player,
    Settings,
}

#[derive(Clone, Copy)]
struct ColumnWidths {
    index: f32,
    artwork: f32,
    title: f32,
    artist: f32,
    album: f32,
    track_number: f32,
    format: f32,
    bitrate: f32,
    file_size: f32,
    year: f32,
    date_added: f32,
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
            artist: ARTIST_COL_W,
            album: ALBUM_COL_W,
            track_number: TRACK_NO_COL_W,
            format: FMT_COL_W,
            bitrate: BITRATE_COL_W,
            file_size: FILE_SIZE_COL_W,
            year: YEAR_COL_W,
            date_added: DATE_ADDED_COL_W,
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
    artist_id: Option<i64>,
    album_id: Option<i64>,
    path: PathBuf,
    title: String,
    artist: String,
    album: String,
    track_number: Option<u32>,
    year: String,
    date_added: SystemTime,
    duration: String,
    duration_value: Duration,
    codec: String,
    bitrate: Option<u32>,
    file_size: u64,
    plays: u32,
    loved: bool,
    artwork: Option<TrackArtwork>,
    album_initials: String,
    album_color: u32,
}

#[derive(Clone)]
struct Artist {
    artist_id: i64,
    name: String,
    bio: Option<String>,
    photo_path: Option<PathBuf>,
    album_count: usize,
    track_count: usize,
    initials: String,
    color: u32,
}

#[derive(Clone)]
struct Album {
    album_id: i64,
    artist_id: i64,
    title: String,
    artist: String,
    year: Option<String>,
    artwork_path: Option<PathBuf>,
    track_count: usize,
    initials: String,
    color: u32,
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

#[derive(Clone)]
struct ColumnDrag {
    column: TableColumn,
    label: SharedString,
    position: Point<Pixels>,
}

#[derive(Clone)]
struct Tooltip {
    id: SharedString,
    label: SharedString,
    position: Point<Pixels>,
}

impl ColumnDrag {
    fn new(column: TableColumn, label: &'static str) -> Self {
        Self {
            column,
            label: label.into(),
            position: Point::default(),
        }
    }

    fn position(mut self, position: Point<Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for ColumnDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(14.0))
            .pt(self.position.y - px(14.0))
            .child(
                div()
                    .h(px(28.0))
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x4b4f5a))
                    .bg(rgb(0x202229))
                    .shadow_lg()
                    .flex()
                    .items_center()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xf0f0f4))
                    .child(self.label.clone()),
            )
    }
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
    Artist(i64),
    Album(i64),
}

#[derive(Clone, PartialEq, Eq)]
struct NavigationEntry {
    page: Page,
    tab: Option<NavigationTab>,
}

#[derive(Clone, PartialEq, Eq)]
struct NavigationTab {
    tab_id: u64,
    source: TabSource,
    search_query: String,
}

struct BrowseTab {
    id: u64,
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
    start_offset: Point<Pixels>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowseScrollbarTarget {
    ArtistsGrid,
    ArtistsTable,
    AlbumsGrid,
    AlbumsTable,
}

#[derive(Clone, Copy)]
struct BrowseScrollbarDrag {
    target: BrowseScrollbarTarget,
    thumb_offset: f32,
    start_offset: Point<Pixels>,
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
    fn library(id: u64) -> Self {
        Self {
            id,
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

    fn playlist(id: u64, playlist_ix: usize) -> Self {
        Self {
            id,
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

    fn artist(id: u64, artist_id: i64) -> Self {
        Self {
            id,
            source: TabSource::Artist(artist_id),
            search_query: String::new(),
            sort_column: SortColumn::Album,
            sort_direction: SortDirection::Ascending,
            selected_track: 0,
            table_scroll_handle: UniformListScrollHandle::new(),
            track_indices: Vec::new(),
            scrollbar_markers: Vec::new(),
        }
    }

    fn album(id: u64, album_id: i64) -> Self {
        Self {
            id,
            source: TabSource::Album(album_id),
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

#[derive(Serialize, Deserialize)]
struct AppState {
    #[serde(default)]
    library_roots: Vec<PathBuf>,
    #[serde(default)]
    playlists: Vec<Playlist>,
    #[serde(default = "default_theme_id")]
    theme_id: String,
    #[serde(default)]
    output_device: Option<String>,
    #[serde(default = "default_volume")]
    volume: f32,
    #[serde(default = "default_visible_table_columns")]
    visible_table_columns: Vec<TableColumn>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library_roots: Vec::new(),
            playlists: Vec::new(),
            theme_id: default_theme_id(),
            output_device: None,
            volume: default_volume(),
            visible_table_columns: default_visible_table_columns(),
        }
    }
}

fn default_volume() -> f32 {
    0.75
}

fn default_visible_table_columns() -> Vec<TableColumn> {
    vec![
        TableColumn::Index,
        TableColumn::Artwork,
        TableColumn::Title,
        TableColumn::Artist,
        TableColumn::Album,
        TableColumn::TrackNumber,
        TableColumn::Bitrate,
        TableColumn::FileSize,
        TableColumn::Year,
        TableColumn::DateAdded,
        TableColumn::Duration,
    ]
}

const ALL_TABLE_COLUMNS: &[TableColumn] = &[
    TableColumn::Index,
    TableColumn::Artwork,
    TableColumn::Title,
    TableColumn::Artist,
    TableColumn::Album,
    TableColumn::TrackNumber,
    TableColumn::Format,
    TableColumn::Bitrate,
    TableColumn::FileSize,
    TableColumn::Year,
    TableColumn::DateAdded,
    TableColumn::Plays,
    TableColumn::Duration,
    TableColumn::Loved,
];

const INDEX_COL_W: f32 = 34.0;
const ART_COL_W: f32 = 32.0;
const TITLE_COL_W: f32 = 188.0;
const ARTIST_COL_W: f32 = 160.0;
const ALBUM_COL_W: f32 = 230.0;
const TRACK_NO_COL_W: f32 = 58.0;
const FMT_COL_W: f32 = 70.0;
const BITRATE_COL_W: f32 = 86.0;
const FILE_SIZE_COL_W: f32 = 86.0;
const YEAR_COL_W: f32 = 72.0;
const DATE_ADDED_COL_W: f32 = 96.0;
const PLAYS_COL_W: f32 = 82.0;
const TIME_COL_W: f32 = 64.0;
const LOVE_COL_W: f32 = 24.0;
const TABLE_ROW_H: f32 = 32.0;
const LEFT_SIDEBAR_W: f32 = 220.0;
const RIGHT_SIDEBAR_W: f32 = 300.0;
const WAVEFORM_SEGMENTS: usize = 360;
const WAVEFORM_CACHE_VERSION: u32 = 1;
const WAVEFORM_SAMPLED_MIN_DURATION: Duration = Duration::from_secs(30);
const WAVEFORM_MIN_SAMPLE_FRAMES: usize = 256;
const WAVEFORM_MAX_SAMPLE_FRAMES: usize = 2048;
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
const SEARCH_DEBOUNCE_DELAY: Duration = Duration::from_millis(90);
const FAST_SCROLL_OVERSCAN_ROWS: usize = 4;
const BROWSE_GRID_CARD_W: f32 = 154.0;
const BROWSE_GRID_GAP: f32 = 16.0;
const BROWSE_GRID_PAD_X: f32 = 32.0;

pub(crate) struct TempoApp {
    focus_handle: FocusHandle,
    search_focus_handle: FocusHandle,
    search_input: TextInputState,
    browse_search_query: String,
    search_debounce_generation: u64,
    page: Page,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
    column_widths: ColumnWidths,
    column_resize: Option<ColumnResize>,
    visible_columns: Vec<TableColumn>,
    column_menu_open: bool,
    column_menu_x: f32,
    column_menu_y: f32,
    tabs: Vec<BrowseTab>,
    active_tab: usize,
    next_tab_id: u64,
    back_history: Vec<NavigationEntry>,
    forward_history: Vec<NavigationEntry>,
    hovered_tooltip_id: Option<SharedString>,
    tooltip: Option<Tooltip>,
    tooltip_generation: u64,
    playing_track: usize,
    is_playing: bool,
    playback_mode: PlaybackMode,
    context_menu_track: Option<usize>,
    context_menu_position: Point<Pixels>,
    tracks: Vec<Track>,
    artists: Vec<Artist>,
    albums: Vec<Album>,
    artist_view_mode: BrowseViewMode,
    album_view_mode: BrowseViewMode,
    queue: Vec<usize>,
    waveform_cache: Vec<Option<Vec<f32>>>,
    waveform_loading: Vec<bool>,
    library_roots: Vec<PathBuf>,
    playlists: Vec<Playlist>,
    theme_id: String,
    themes: Vec<Theme>,
    library_root_label: String,
    library_status: String,
    playback_status: String,
    output_device: Option<String>,
    output_menu_source: Option<OutputMenuSource>,
    output_menu_position: Point<Pixels>,
    volume: f32,
    pre_mute_volume: f32,
    scan_progress: ScanProgress,
    scan_errors: Vec<IndexingError>,
    is_scanning: bool,
    table_scrollbar_drag: Option<TableScrollbarDrag>,
    browse_scrollbar_drag: Option<BrowseScrollbarDrag>,
    artist_grid_scroll_handle: UniformListScrollHandle,
    artist_table_scroll_handle: UniformListScrollHandle,
    album_grid_scroll_handle: UniformListScrollHandle,
    album_table_scroll_handle: UniformListScrollHandle,
    scan_errors_scroll_handle: UniformListScrollHandle,
    table_is_scrolling: bool,
    table_scroll_generation: u64,
    catalog: Option<CatalogStore>,
    _library_watcher: Option<LibraryWatcher>,
    playback: Option<PlaybackController>,
}

impl TempoApp {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let state = Self::load_app_state();
        let themes = bundled_themes();
        let theme_id = resolve_theme_id(state.theme_id, &themes);
        let roots = Self::default_library_roots(&state.library_roots);
        let library_root_label = Self::library_root_label(&roots);
        let (catalog, catalog_status) = match CatalogStore::open_default() {
            Ok(catalog) => (Some(catalog), None),
            Err(error) => (None, Some(format!("Catalog cache unavailable: {error:#}"))),
        };
        let cached_tracks = Self::load_cached_tracks(catalog.as_ref(), &roots).unwrap_or_default();
        let cached_artists =
            Self::load_cached_artists(catalog.as_ref(), &roots).unwrap_or_default();
        let cached_albums = Self::load_cached_albums(catalog.as_ref(), &roots).unwrap_or_default();
        let (event_tx, event_rx) = mpsc::channel();
        let (mut library_status, library_watcher) =
            Self::start_watcher_for_roots(&roots, event_tx, catalog.clone());
        if let Some(catalog_status) = catalog_status {
            library_status = catalog_status;
        }
        let playlists = state.playlists;
        let volume = state.volume.clamp(0.0, 1.0);
        let visible_columns = Self::sanitize_visible_columns(state.visible_table_columns);
        let (playback, playback_status) =
            match PlaybackController::new(state.output_device.as_deref(), volume) {
                Ok(playback) => (Some(playback), "Audio output ready".to_string()),
                Err(error) => (None, format!("Playback unavailable: {error:#}")),
            };
        let output_device = playback
            .as_ref()
            .map(|playback| playback.output_name().to_string())
            .or(state.output_device);

        let initial_page = if roots.is_empty() {
            Page::Settings
        } else {
            Page::Library
        };

        let mut app = Self {
            focus_handle,
            search_focus_handle,
            search_input: TextInputState::default(),
            browse_search_query: String::new(),
            search_debounce_generation: 0,
            page: initial_page,
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            column_widths: ColumnWidths::default(),
            column_resize: None,
            visible_columns,
            column_menu_open: false,
            column_menu_x: 0.0,
            column_menu_y: 0.0,
            tabs: vec![BrowseTab::library(1)],
            active_tab: 0,
            next_tab_id: 2,
            back_history: Vec::new(),
            forward_history: Vec::new(),
            hovered_tooltip_id: None,
            tooltip: None,
            tooltip_generation: 0,
            playing_track: 0,
            is_playing: false,
            playback_mode: PlaybackMode::Straight,
            context_menu_track: None,
            context_menu_position: Point::default(),
            tracks: cached_tracks,
            artists: cached_artists,
            albums: cached_albums,
            artist_view_mode: BrowseViewMode::Grid,
            album_view_mode: BrowseViewMode::Grid,
            queue: Vec::new(),
            waveform_cache: Vec::new(),
            waveform_loading: Vec::new(),
            library_roots: roots,
            playlists,
            theme_id,
            themes,
            library_root_label,
            library_status,
            playback_status,
            output_device,
            output_menu_source: None,
            output_menu_position: Point::default(),
            volume,
            pre_mute_volume: if volume > 0.0 {
                volume
            } else {
                default_volume()
            },
            scan_progress: ScanProgress::default(),
            scan_errors: Vec::new(),
            is_scanning: false,
            table_scrollbar_drag: None,
            browse_scrollbar_drag: None,
            artist_grid_scroll_handle: UniformListScrollHandle::new(),
            artist_table_scroll_handle: UniformListScrollHandle::new(),
            album_grid_scroll_handle: UniformListScrollHandle::new(),
            album_table_scroll_handle: UniformListScrollHandle::new(),
            scan_errors_scroll_handle: UniformListScrollHandle::new(),
            table_is_scrolling: false,
            table_scroll_generation: 0,
            catalog,
            _library_watcher: library_watcher,
            playback,
        };

        app.invalidate_track_indices();
        app.start_library_event_loop(event_rx, cx);
        app.start_playback_tick(cx);
        app
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

    fn resolved_page(&self, page: Page) -> Page {
        match page {
            Page::Library | Page::Artists | Page::Albums | Page::ScanErrors
                if self.library_roots.is_empty() =>
            {
                Page::Settings
            }
            page => page,
        }
    }

    fn set_page_without_history(&mut self, page: Page) {
        self.page = self.resolved_page(page);
        if self.page != Page::Library {
            self.search_input.clear();
            self.browse_search_query.clear();
            self.search_debounce_generation = self.search_debounce_generation.wrapping_add(1);
        }
        self.context_menu_track = None;
    }

    fn current_navigation_entry(&self) -> NavigationEntry {
        NavigationEntry {
            page: self.page,
            tab: (self.page == Page::Library).then(|| NavigationTab {
                tab_id: self.active_tab().id,
                source: self.active_tab().source,
                search_query: self.active_search_query().to_string(),
            }),
        }
    }

    fn allocate_tab_id(&mut self) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id = self.next_tab_id.saturating_add(1);
        id
    }

    fn reserve_tab_id(&mut self, id: u64) {
        self.next_tab_id = self.next_tab_id.max(id.saturating_add(1));
    }

    fn record_navigation_from(&mut self, previous: NavigationEntry) {
        if previous != self.current_navigation_entry() {
            self.back_history.push(previous);
            self.forward_history.clear();
        }
    }

    fn open_page(&mut self, page: Page) {
        let previous = self.current_navigation_entry();
        self.set_page_without_history(page);
        if self.page == Page::Library {
            self.sync_search_input_to_active_tab();
        }
        self.record_navigation_from(previous);
    }

    fn ensure_navigation_tab(&mut self, nav_tab: &NavigationTab) -> usize {
        if let Some(tab_ix) = self.tabs.iter().position(|tab| tab.id == nav_tab.tab_id) {
            self.restore_navigation_tab_state(tab_ix, nav_tab);
            return tab_ix;
        }

        if let Some(tab_ix) = self.tabs.iter().position(|tab| {
            tab.source == nav_tab.source && tab.search_query == nav_tab.search_query
        }) {
            return tab_ix;
        }

        self.reserve_tab_id(nav_tab.tab_id);
        let mut tab = match nav_tab.source {
            TabSource::Library => BrowseTab::library(nav_tab.tab_id),
            TabSource::Playlist(playlist_ix) => BrowseTab::playlist(nav_tab.tab_id, playlist_ix),
            TabSource::Artist(artist_id) => BrowseTab::artist(nav_tab.tab_id, artist_id),
            TabSource::Album(album_id) => BrowseTab::album(nav_tab.tab_id, album_id),
        };
        tab.search_query = nav_tab.search_query.clone();
        self.tabs.push(tab);
        let tab_ix = self.tabs.len() - 1;
        self.rebuild_track_indices_for_tab(tab_ix);
        tab_ix
    }

    fn restore_navigation_tab_state(&mut self, tab_ix: usize, nav_tab: &NavigationTab) {
        let Some(tab) = self.tabs.get_mut(tab_ix) else {
            return;
        };
        if tab.source == nav_tab.source && tab.search_query != nav_tab.search_query {
            tab.search_query = nav_tab.search_query.clone();
            self.rebuild_track_indices_for_tab(tab_ix);
        }
    }

    fn restore_navigation_entry(&mut self, entry: NavigationEntry) {
        if entry.page == Page::Library {
            if let Some(tab) = entry.tab {
                self.active_tab = self.ensure_navigation_tab(&tab);
            }
            self.set_page_without_history(Page::Library);
            if self.page == Page::Library {
                self.sync_search_input_to_active_tab();
            }
        } else {
            self.set_page_without_history(entry.page);
        }
    }

    fn navigate_back(&mut self) {
        let Some(entry) = self.back_history.pop() else {
            return;
        };

        let current = self.current_navigation_entry();
        self.forward_history.push(current);
        self.restore_navigation_entry(entry);
    }

    fn navigate_forward(&mut self) {
        let Some(entry) = self.forward_history.pop() else {
            return;
        };

        let current = self.current_navigation_entry();
        self.back_history.push(current);
        self.restore_navigation_entry(entry);
    }

    fn theme(&self) -> &Theme {
        self.themes
            .iter()
            .find(|theme| theme.id == self.theme_id)
            .or_else(|| self.themes.first())
            .expect("at least one theme is always available")
    }

    fn colors(&self) -> &ThemeColors {
        &self.theme().colors
    }

    fn set_theme(&mut self, theme_id: &str) {
        if self.themes.iter().any(|theme| theme.id == theme_id) {
            self.theme_id = theme_id.to_string();
            self.save_app_state();
        }
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

    fn sync_search_input_to_active_tab(&mut self) {
        self.search_input
            .set_text(self.active_search_query().to_string());
        self.search_debounce_generation = self.search_debounce_generation.wrapping_add(1);
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
            TabSource::Artist(artist_id) => self
                .artist_by_id(artist_id)
                .map(|artist| artist.name.clone())
                .unwrap_or_else(|| "Missing Artist".to_string()),
            TabSource::Album(album_id) => self
                .album_by_id(album_id)
                .map(|album| album.title.clone())
                .unwrap_or_else(|| "Missing Album".to_string()),
        }
    }

    fn new_library_tab(&mut self) {
        let previous = self.current_navigation_entry();
        let tab_id = self.allocate_tab_id();
        self.tabs.push(BrowseTab::library(tab_id));
        self.active_tab = self.tabs.len() - 1;
        self.rebuild_track_indices_for_tab(self.active_tab);
        self.set_page_without_history(Page::Library);
        self.sync_search_input_to_active_tab();
        self.context_menu_track = None;
        self.record_navigation_from(previous);
    }

    fn new_search_tab(&mut self, query: String) {
        let previous = self.current_navigation_entry();
        let tab_id = self.allocate_tab_id();
        let mut tab = BrowseTab::library(tab_id);
        tab.search_query = query.clone();
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        self.rebuild_track_indices_for_tab(self.active_tab);
        self.set_page_without_history(Page::Library);
        self.search_input.set_text(query);
        self.context_menu_track = None;
        self.record_navigation_from(previous);
    }

    fn open_all_music_tab(&mut self) {
        let previous = self.current_navigation_entry();
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| tab.source == TabSource::Library && tab.search_query.trim().is_empty())
        {
            self.active_tab = tab_ix;
        } else {
            let tab_id = self.allocate_tab_id();
            self.tabs.push(BrowseTab::library(tab_id));
            self.active_tab = self.tabs.len() - 1;
            self.rebuild_track_indices_for_tab(self.active_tab);
        }

        self.set_page_without_history(Page::Library);
        self.sync_search_input_to_active_tab();
        self.record_navigation_from(previous);
    }

    fn open_playlist_tab(&mut self, playlist_ix: usize) {
        if playlist_ix >= self.playlists.len() {
            return;
        }

        let previous = self.current_navigation_entry();
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| tab.source == TabSource::Playlist(playlist_ix))
        {
            self.active_tab = tab_ix;
        } else {
            let tab_id = self.allocate_tab_id();
            self.tabs.push(BrowseTab::playlist(tab_id, playlist_ix));
            self.active_tab = self.tabs.len() - 1;
            self.rebuild_track_indices_for_tab(self.active_tab);
        }

        self.set_page_without_history(Page::Library);
        self.sync_search_input_to_active_tab();
        self.record_navigation_from(previous);
    }

    fn open_artist_tab(&mut self, artist_id: i64) {
        let previous = self.current_navigation_entry();
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| tab.source == TabSource::Artist(artist_id))
        {
            self.active_tab = tab_ix;
        } else {
            let tab_id = self.allocate_tab_id();
            self.tabs.push(BrowseTab::artist(tab_id, artist_id));
            self.active_tab = self.tabs.len() - 1;
            self.rebuild_track_indices_for_tab(self.active_tab);
        }

        self.set_page_without_history(Page::Library);
        self.sync_search_input_to_active_tab();
        self.record_navigation_from(previous);
    }

    fn open_album_tab(&mut self, album_id: i64) {
        let previous = self.current_navigation_entry();
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| tab.source == TabSource::Album(album_id))
        {
            self.active_tab = tab_ix;
        } else {
            let tab_id = self.allocate_tab_id();
            self.tabs.push(BrowseTab::album(tab_id, album_id));
            self.active_tab = self.tabs.len() - 1;
            self.rebuild_track_indices_for_tab(self.active_tab);
        }

        self.set_page_without_history(Page::Library);
        self.sync_search_input_to_active_tab();
        self.record_navigation_from(previous);
    }

    fn select_tab(&mut self, tab_ix: usize) {
        if tab_ix >= self.tabs.len() {
            return;
        }

        let previous = self.current_navigation_entry();
        self.active_tab = tab_ix;
        self.set_page_without_history(Page::Library);
        self.sync_search_input_to_active_tab();
        self.record_navigation_from(previous);
    }

    fn artist_by_id(&self, artist_id: i64) -> Option<&Artist> {
        self.artists
            .iter()
            .find(|artist| artist.artist_id == artist_id)
    }

    fn album_by_id(&self, album_id: i64) -> Option<&Album> {
        self.albums.iter().find(|album| album.album_id == album_id)
    }

    fn albums_for_artist(&self, artist_id: i64) -> Vec<&Album> {
        let artist_name = self
            .artist_by_id(artist_id)
            .map(|artist| artist.name.as_str());
        self.albums
            .iter()
            .filter(|album| {
                album.artist_id == artist_id || artist_name.is_some_and(|name| album.artist == name)
            })
            .collect()
    }

    fn open_artist_tab_for_track(&mut self, track_ix: usize) {
        let Some(track) = self.tracks.get(track_ix) else {
            return;
        };
        let artist_name = primary_artist_name(&track.artist);
        let artist_id = track
            .artist_id
            .or_else(|| {
                self.artists
                    .iter()
                    .find(|artist| artist.name == artist_name)
                    .map(|artist| artist.artist_id)
            })
            .unwrap_or_else(|| Self::synthetic_tab_entity_id(&artist_name));
        self.open_artist_tab(artist_id);
    }

    fn open_album_tab_for_track(&mut self, track_ix: usize) {
        let Some(track) = self.tracks.get(track_ix) else {
            return;
        };
        let primary_artist = primary_artist_name(&track.artist);
        let album_id = track
            .album_id
            .or_else(|| {
                self.albums
                    .iter()
                    .find(|album| album.title == track.album && album.artist == primary_artist)
                    .map(|album| album.album_id)
            })
            .unwrap_or_else(|| {
                Self::synthetic_tab_entity_id(&format!("{}:{}", primary_artist, track.album))
            });
        self.open_album_tab(album_id);
    }

    fn select_track_in_all_music(&mut self, track_ix: usize) {
        if track_ix >= self.tracks.len() {
            return;
        }

        self.open_all_music_tab();
        self.set_active_selected_track(track_ix);
        if let Some(row_ix) = self
            .current_track_indices()
            .iter()
            .position(|ix| *ix == track_ix)
        {
            self.active_tab()
                .table_scroll_handle
                .scroll_to_item(row_ix, ScrollStrategy::Center);
        }
        self.context_menu_track = None;
    }

    fn synthetic_tab_entity_id(value: &str) -> i64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for byte in value.to_lowercase().bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }

        -((hash & 0x3fff_ffff_ffff_ffff) as i64).max(1)
    }

    fn close_tab(&mut self, tab_ix: usize) {
        if !self.can_close_tab(tab_ix) {
            return;
        }

        self.tabs.remove(tab_ix);
        if self.active_tab > tab_ix {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.sync_search_input_to_active_tab();
        self.context_menu_track = None;
    }

    fn can_close_tab(&self, tab_ix: usize) -> bool {
        let Some(tab) = self.tabs.get(tab_ix) else {
            return false;
        };

        self.tabs.len() > 1
            && !(tab.source == TabSource::Library && tab.search_query.trim().is_empty())
    }
}

impl From<tempo::library::Track> for Track {
    fn from(track: tempo::library::Track) -> Self {
        let album_initials = TempoApp::album_initials_for(&track.album, &track.title);
        let album_color = TempoApp::album_color_for(&track.album, &track.artist);

        Self {
            artist_id: None,
            album_id: None,
            path: track.path,
            title: track.title,
            artist: track.artist,
            album: track.album,
            track_number: track.track_number,
            year: track.year.unwrap_or_else(|| "Unknown year".to_string()),
            date_added: track.date_added,
            duration: format_duration(track.duration),
            duration_value: track.duration,
            codec: track.codec,
            bitrate: track.bitrate,
            file_size: track.file_size,
            plays: 0,
            loved: false,
            artwork: track.artwork.and_then(TrackArtwork::from_library),
            album_initials,
            album_color,
        }
    }
}

impl From<CatalogTrack> for Track {
    fn from(track: CatalogTrack) -> Self {
        let album_initials = TempoApp::album_initials_for(&track.album, &track.title);
        let album_color = TempoApp::album_color_for(&track.album, &track.artist);

        Self {
            artist_id: Some(track.artist_id),
            album_id: Some(track.album_id),
            path: track.path,
            title: track.title,
            artist: track.artist,
            album: track.album,
            track_number: track.track_number,
            year: track.year.unwrap_or_else(|| "Unknown year".to_string()),
            date_added: track.date_added,
            duration: format_duration(track.duration),
            duration_value: track.duration,
            codec: track.codec,
            bitrate: track.bitrate,
            file_size: track.file_size,
            plays: track.play_count,
            loved: false,
            artwork: track.artwork_path.map(TrackArtwork::File),
            album_initials,
            album_color,
        }
    }
}

impl From<CatalogArtist> for Artist {
    fn from(artist: CatalogArtist) -> Self {
        Self {
            artist_id: artist.artist_id,
            initials: TempoApp::initials_for(&artist.name),
            color: TempoApp::color_for(&artist.name, "artist"),
            name: artist.name,
            bio: artist.bio,
            photo_path: artist.photo_path,
            album_count: artist.album_count,
            track_count: artist.track_count,
        }
    }
}

impl From<CatalogAlbum> for Album {
    fn from(album: CatalogAlbum) -> Self {
        Self {
            album_id: album.album_id,
            artist_id: album.artist_id,
            initials: TempoApp::initials_for(&album.title),
            color: TempoApp::album_color_for(&album.title, &album.artist),
            title: album.title,
            artist: album.artist,
            year: album.year,
            artwork_path: album.artwork_path,
            track_count: album.track_count,
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
        let colors = self.colors();

        div()
            .id("tempo-app")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::play_selected))
            .on_action(cx.listener(Self::toggle_pause))
            .on_action(cx.listener(Self::move_selection_up))
            .on_action(cx.listener(Self::move_selection_down))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::focus_search))
            .on_action(cx.listener(Self::open_settings_action))
            .on_action(cx.listener(Self::play_random_track_action))
            .on_action(cx.listener(Self::navigate_back_action))
            .on_action(cx.listener(Self::navigate_forward_action))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(Self::navigate_back_mouse),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(Self::navigate_forward_mouse),
            )
            .on_key_down(cx.listener(Self::handle_table_key_down))
            .size_full()
            .bg(rgb(colors.app))
            .text_color(rgb(colors.text))
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
                    .child(self.render_content(window, cx)),
            )
            .child(self.render_player_bar(window, cx))
            .when_some(
                self.context_menu_track
                    .filter(|track_ix| *track_ix < self.tracks.len()),
                |this, track_ix| this.child(self.render_context_menu(track_ix, cx)),
            )
            .when(self.column_menu_open, |this| {
                this.child(self.render_column_menu(cx))
            })
            .when(
                self.output_menu_source == Some(OutputMenuSource::Settings),
                |this| this.child(self.settings_output_device_menu(cx)),
            )
            .when_some(self.tooltip.clone(), |this, tooltip| {
                this.child(self.render_tooltip(&tooltip))
            })
    }
}

impl TempoApp {
    fn play_selected(&mut self, _: &PlaySelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_focus_handle.is_focused(window) {
            return;
        }

        if self.tracks.is_empty() {
            return;
        }

        self.play_track(self.active_selected_track());
        cx.notify();
    }

    fn toggle_pause(&mut self, _: &TogglePause, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_focus_handle.is_focused(window) {
            return;
        }

        if self.tracks.is_empty() {
            return;
        }

        self.toggle_playback();
        cx.notify();
    }

    fn move_selection_up(
        &mut self,
        _: &MoveSelectionUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_focus_handle.is_focused(window) {
            self.search_input.move_left(false, false);
            cx.notify();
            return;
        }

        self.move_selection(-1);
        cx.notify();
    }

    fn move_selection_down(
        &mut self,
        _: &MoveSelectionDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_focus_handle.is_focused(window) {
            self.search_input.move_right(false, false);
            cx.notify();
            return;
        }

        self.move_selection(1);
        cx.notify();
    }

    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.new_library_tab();
        window.focus(&self.search_focus_handle);
        cx.notify();
    }

    fn close_active_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active_tab);
        cx.notify();
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.page, Page::Library | Page::Artists | Page::Albums) {
            self.open_page(Page::Library);
            self.sync_search_input_to_active_tab();
        }
        window.focus(&self.search_focus_handle);
        cx.notify();
    }

    fn open_settings_action(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.open_page(Page::Settings);
        cx.notify();
    }

    fn play_random_track_action(
        &mut self,
        _: &PlayRandomTrack,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.play_random_track();
        cx.notify();
    }

    fn navigate_back_action(&mut self, _: &NavigateBack, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_back();
        cx.notify();
    }

    fn navigate_forward_action(
        &mut self,
        _: &NavigateForward,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_forward();
        cx.notify();
    }

    fn navigate_back_mouse(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate_back();
        cx.stop_propagation();
        cx.notify();
    }

    fn navigate_forward_mouse(
        &mut self,
        _: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate_forward();
        cx.stop_propagation();
        cx.notify();
    }
}
