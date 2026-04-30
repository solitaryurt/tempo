use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    env, fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use gpui::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, ClipboardItem, Context, Corner,
    CursorStyle, Entity, FocusHandle, Image, ImageFormat, IntoElement, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, ObjectFit, ParentElement, PathPromptOptions, Pixels, Point, Render,
    ScrollStrategy, ScrollWheelEvent, SharedString, Styled, Subscription, UniformListScrollHandle,
    Window, anchored, div, img, point, prelude::*, px, relative, rgb, uniform_list,
};
use rodio::{Decoder, Source as _};
use serde::{Deserialize, Serialize};
use tempo::{
    catalog::{
        CatalogAlbum, CatalogArtist, CatalogMetadataActivity, CatalogStore, CatalogTrack,
        individual_artist_names, primary_artist_name,
    },
    library::{
        Artwork as LibraryArtwork, IndexingError, LibraryEvent, LibraryIndexer, LibraryWatcher,
        ScanProgress,
    },
    metadata_worker::{MetadataEvent, MetadataWorker},
    perf,
    playback::PlaybackController,
};

mod artwork;
mod browse_grids;
mod history;
mod library_state;
mod library_view;
mod liked;
mod menu;
mod player;
mod search;
mod settings;
mod sidebar;
mod table;
mod text_input;
mod theme;
mod tooltip;

// Re-export the layout helpers so direct child modules
// (`sidebar`, `table`, `library_view`, etc.) and grandchild modules
// (`player::*`) can call `menu_at`/`menu_panel`/etc. via `super::*`
// without an explicit `use crate::app::menu::...` line. The helpers
// are free functions taking `ThemeColors` so any entity can render
// menus and album tiles uniformly. See the module docs in
// `src/app/menu.rs` and `src/app/artwork.rs`.
pub(in crate::app) use menu::{
    menu_at, menu_header, menu_header_with_subtitle, menu_item, menu_item_base, menu_panel,
    menu_section_label,
};

use crate::{
    CloseAllTabs, CloseTab, FocusSearch, MoveSelectionDown, MoveSelectionUp, NavigateBack,
    NavigateForward, NewTab, NextTab, OpenSettings, PlayRandomTrack, PlaySelected, PreviousTab,
    ReopenClosedTab, SelectTab1, SelectTab2, SelectTab3, SelectTab4, SelectTab5, SelectTab6,
    SelectTab7, SelectTab8, SelectTab9, SelectTab10, TogglePause,
};
use text_input::TextInputState;
use theme::{Theme, ThemeColors, bundled_themes, default_theme_id, resolve_theme_id};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Page {
    Library,
    Artists,
    Albums,
    Genres,
    Liked,
    PlaybackHistory,
    ScanErrors,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum BrowseViewMode {
    Grid,
    Table,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SortColumn {
    Index,
    Title,
    Artist,
    AlbumByArtist,
    Album,
    Genre,
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
    Genre,
    TrackNumber,
    Format,
    Bitrate,
    FileSize,
    Year,
    DateAdded,
    Plays,
    Duration,
    // Renamed from `Loved` -> `Liked` (terminology + behavior). Old saved
    // state from before the rename still deserializes via the serde
    // alias so users don't lose their column layout.
    #[serde(alias = "Loved")]
    Liked,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlaybackMode {
    Straight,
    Loop,
    Shuffle,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum OnlineMetadataMode {
    Off,
    Automatic,
}

impl OnlineMetadataMode {
    fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Automatic => "Automatic",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OutputMenuSource {
    Player,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NowPlayingLink {
    Title,
    Artist,
    Album,
}

#[derive(Clone, Copy)]
struct ColumnWidths {
    index: f32,
    artwork: f32,
    title: f32,
    artist: f32,
    album: f32,
    genre: f32,
    track_number: f32,
    format: f32,
    bitrate: f32,
    file_size: f32,
    year: f32,
    date_added: f32,
    plays: f32,
    duration: f32,
    liked: f32,
}

impl Default for ColumnWidths {
    fn default() -> Self {
        Self {
            index: INDEX_COL_W,
            artwork: ART_COL_W,
            title: TITLE_COL_W,
            artist: ARTIST_COL_W,
            album: ALBUM_COL_W,
            genre: GENRE_COL_W,
            track_number: TRACK_NO_COL_W,
            format: FMT_COL_W,
            bitrate: BITRATE_COL_W,
            file_size: FILE_SIZE_COL_W,
            year: YEAR_COL_W,
            date_added: DATE_ADDED_COL_W,
            plays: PLAYS_COL_W,
            duration: TIME_COL_W,
            liked: LIKED_COL_W,
        }
    }
}

#[derive(Clone, Copy)]
struct ArtistTableColumnWidths {
    artwork: f32,
    artist: f32,
    albums: f32,
    tracks: f32,
    duration: f32,
}

impl Default for ArtistTableColumnWidths {
    fn default() -> Self {
        Self {
            artwork: 42.0,
            artist: 360.0,
            albums: 92.0,
            tracks: 92.0,
            duration: 92.0,
        }
    }
}

#[derive(Clone, Copy)]
struct AlbumTableColumnWidths {
    artwork: f32,
    album: f32,
    artist: f32,
    year: f32,
    tracks: f32,
    duration: f32,
}

impl Default for AlbumTableColumnWidths {
    fn default() -> Self {
        Self {
            artwork: 42.0,
            album: 260.0,
            artist: 220.0,
            year: 90.0,
            tracks: 92.0,
            duration: 92.0,
        }
    }
}

#[derive(Clone, Copy)]
struct GenreTableColumnWidths {
    genre: f32,
    artists: f32,
    albums: f32,
    tracks: f32,
    duration: f32,
}

impl Default for GenreTableColumnWidths {
    fn default() -> Self {
        Self {
            genre: 260.0,
            artists: 260.0,
            albums: 92.0,
            tracks: 92.0,
            duration: 92.0,
        }
    }
}

#[derive(Clone, Copy)]
struct ScanErrorColumnWidths {
    index: f32,
    path: f32,
    error: f32,
}

impl Default for ScanErrorColumnWidths {
    fn default() -> Self {
        Self {
            index: 52.0,
            path: 420.0,
            error: 420.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ArtistTableColumn {
    Artwork,
    Artist,
    Albums,
    Tracks,
    Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum AlbumTableColumn {
    Artwork,
    Album,
    Artist,
    Year,
    Tracks,
    Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum GenreTableColumn {
    Genre,
    Artists,
    Albums,
    Tracks,
    Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanErrorColumn {
    Index,
    Path,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnResizeTarget {
    Track(TableColumn),
    Artist(ArtistTableColumn),
    Album(AlbumTableColumn),
    Genre(GenreTableColumn),
    ScanError(ScanErrorColumn),
    PlaybackHistoryPlayedAt,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnMenuKind {
    Tracks,
    Artists,
    Albums,
    Genres,
}

#[derive(Clone)]
struct BrowseColumnDrag {
    target: ColumnResizeTarget,
    label: SharedString,
    position: Point<Pixels>,
}

#[derive(Clone, Copy)]
struct ColumnResize {
    target: ColumnResizeTarget,
    start_x: f32,
    start_width: f32,
}

#[derive(Clone)]
struct Track {
    artist_id: Option<i64>,
    album_id: Option<i64>,
    path: PathBuf,
    // Hot, render-path text fields are `SharedString` (Arc-shared) so
    // every per-cell `.clone()` in the table/queue/player bar is a
    // refcount bump rather than a fresh allocation. Previously these
    // were `String` and each visible row cloned all of them on every
    // repaint.
    title: SharedString,
    artist: SharedString,
    album: SharedString,
    genre: SharedString,
    track_number: Option<u32>,
    year: SharedString,
    date_added: SystemTime,
    duration: SharedString,
    duration_value: Duration,
    codec: SharedString,
    bitrate: Option<u32>,
    file_size: u64,
    plays: u32,
    liked: bool,
    artwork: Option<TrackArtwork>,
    album_initials: String,
    album_color: u32,
    /// Pre-lowercased concatenation of `title`, `artist`, `album`,
    /// `genre`, `year`, `codec`, and the displayed path. Computed once
    /// at construction time so search keystrokes don't have to
    /// `format!() + to_lowercase()` per track per filter rebuild --
    /// previously the dominant cost on a 50k-track library.
    searchable_lower: String,
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
    /// Pre-lowercased searchable blob; see `Track::searchable_lower`.
    searchable_lower: String,
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
    /// Pre-lowercased searchable blob; see `Track::searchable_lower`.
    searchable_lower: String,
}

#[derive(Clone)]
struct GenreAlbumSummary {
    album_id: Option<i64>,
    title: String,
    artist: String,
    artwork_path: Option<PathBuf>,
    track_count: usize,
    play_count: u32,
    initials: String,
    color: u32,
}

#[derive(Clone)]
struct Genre {
    key: String,
    name: String,
    artist_count: usize,
    album_count: usize,
    track_count: usize,
    duration_value: Duration,
    artists: Vec<String>,
    albums: Vec<GenreAlbumSummary>,
    top_albums: Vec<GenreAlbumSummary>,
    color: u32,
    searchable_lower: String,
}

#[derive(Default)]
struct MetadataDemandQueue {
    artists: HashSet<i64>,
    albums: HashSet<i64>,
}

#[derive(Clone)]
struct WaveformSource {
    path: PathBuf,
    title: SharedString,
    artist: SharedString,
    album: SharedString,
    duration: SharedString,
    duration_value: Duration,
}

// `WaveformSource` is constructed inline by `PlayerEntity::cached_waveform_for_path`
// (the only caller) from a `PlayingTrackSnapshot`. The previous
// `from_track(&Track)` constructor is gone with the entity split
// since the entity no longer borrows a `Track` directly.

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

impl BrowseColumnDrag {
    fn new(target: ColumnResizeTarget, label: &'static str) -> Self {
        Self {
            target,
            label: label.into(),
            position: Point::default(),
        }
    }

    fn position(mut self, position: Point<Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for BrowseColumnDrag {
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
            title: track.title.clone(),
            artist: track.artist.clone(),
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

/// Right-click menu state for a sidebar playlist nav item.
#[derive(Clone, Copy)]
struct PlaylistContextMenu {
    playlist_ix: usize,
    position: Point<Pixels>,
}

/// Inline-rename state for a sidebar playlist nav item. Holds the
/// editing buffer; the focus handle lives on `TempoApp` so it survives
/// across re-renders.
struct PlaylistRename {
    playlist_ix: usize,
    input: TextInputState,
}

#[derive(Clone, Serialize, Deserialize)]
struct PlaybackHistoryEntry {
    played_at_unix_secs: u64,
    track_path: PathBuf,
    title: String,
    artist: String,
    album: String,
    duration: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
enum TabSource {
    Library,
    Playlist(usize),
    Artist(i64),
    Album(i64),
    Genre(String),
}

/// Lightweight snapshot of a closed tab's identity used by the
/// reopen-closed-tab stack (Ctrl+Shift+T). We deliberately drop
/// per-session ephemera (sort, scroll position, selection) and only
/// preserve enough state to recreate the tab via the existing
/// `open_*_tab` helpers. The `search_query` is preserved so a closed
/// search Library tab reopens with the same query string.
#[derive(Clone)]
struct ClosedTab {
    source: TabSource,
    search_query: String,
}

/// Maximum number of recently-closed tabs the reopen stack remembers.
const CLOSED_TABS_MAX: usize = 25;

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
    table_scroll_top: f32,
    restore_table_scroll_top: Option<f32>,
    table_horizontal_scroll: f32,
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

#[derive(Clone, Copy)]
struct TableHorizontalScrollbarDrag {
    thumb_offset: f32,
    start_scroll: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowseScrollbarTarget {
    ArtistsGrid,
    ArtistsTable,
    AlbumsGrid,
    AlbumsTable,
    GenresGrid,
    PlaybackHistory,
    Liked,
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

#[derive(Clone, Copy)]
struct TableHorizontalScrollbarMetrics {
    track_left: f32,
    track_width: f32,
    thumb_left: f32,
    thumb_width: f32,
    max_scroll: f32,
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
            table_scroll_top: 0.0,
            restore_table_scroll_top: None,
            table_horizontal_scroll: 0.0,
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
            table_scroll_top: 0.0,
            restore_table_scroll_top: None,
            table_horizontal_scroll: 0.0,
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
            table_scroll_top: 0.0,
            restore_table_scroll_top: None,
            table_horizontal_scroll: 0.0,
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
            table_scroll_top: 0.0,
            restore_table_scroll_top: None,
            table_horizontal_scroll: 0.0,
            table_scroll_handle: UniformListScrollHandle::new(),
            track_indices: Vec::new(),
            scrollbar_markers: Vec::new(),
        }
    }

    fn genre(id: u64, genre_key: String) -> Self {
        Self {
            id,
            source: TabSource::Genre(genre_key),
            search_query: String::new(),
            sort_column: SortColumn::Album,
            sort_direction: SortDirection::Ascending,
            selected_track: 0,
            table_scroll_top: 0.0,
            restore_table_scroll_top: None,
            table_horizontal_scroll: 0.0,
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
    #[serde(default = "default_visible_artist_table_columns")]
    visible_artist_table_columns: Vec<ArtistTableColumn>,
    #[serde(default = "default_visible_album_table_columns")]
    visible_album_table_columns: Vec<AlbumTableColumn>,
    #[serde(default = "default_visible_genre_table_columns")]
    visible_genre_table_columns: Vec<GenreTableColumn>,
    #[serde(default = "default_page")]
    page: Page,
    #[serde(default)]
    tabs: Vec<SavedBrowseTab>,
    #[serde(default)]
    active_tab_id: Option<u64>,
    #[serde(default = "default_browse_view_mode")]
    artist_view_mode: BrowseViewMode,
    #[serde(default = "default_browse_view_mode")]
    album_view_mode: BrowseViewMode,
    #[serde(default = "default_browse_view_mode")]
    genre_view_mode: BrowseViewMode,
    #[serde(default = "default_artist_table_sort_column")]
    artist_table_sort_column: ArtistTableColumn,
    #[serde(default = "default_sort_direction")]
    artist_table_sort_direction: SortDirection,
    #[serde(default = "default_album_table_sort_column")]
    album_table_sort_column: AlbumTableColumn,
    #[serde(default = "default_sort_direction")]
    album_table_sort_direction: SortDirection,
    #[serde(default = "default_genre_table_sort_column")]
    genre_table_sort_column: GenreTableColumn,
    #[serde(default = "default_sort_direction")]
    genre_table_sort_direction: SortDirection,
    #[serde(default)]
    genre_grid_scroll_top: f32,
    #[serde(default)]
    artist_grid_scroll_top: f32,
    #[serde(default)]
    artist_table_scroll_top: f32,
    #[serde(default)]
    album_grid_scroll_top: f32,
    #[serde(default)]
    album_table_scroll_top: f32,
    #[serde(default)]
    playback_history: Vec<PlaybackHistoryEntry>,
    /// Path of the track that was last shown in the now-playing area.
    /// Stored as a path (rather than an index) so the now-playing display
    /// stays correct after the library is rescanned and indices shift.
    #[serde(default)]
    playing_track_path: Option<PathBuf>,
    #[serde(default = "default_online_metadata_mode")]
    online_metadata_mode: OnlineMetadataMode,
    /// One-shot flag tracking whether the Liked-column position
    /// migration has run on this saved state. The migration moves a
    /// legacy trailing `Liked` (formerly inert `Loved`) column to the
    /// new default slot right after `#`. Once we've done it on a
    /// given state file, we leave the user's layout alone forever
    /// after -- so dragging Liked back to the end sticks across
    /// restarts.
    #[serde(default)]
    liked_column_migrated: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct SavedBrowseTab {
    id: u64,
    source: TabSource,
    #[serde(default)]
    search_query: String,
    #[serde(default = "default_sort_column")]
    sort_column: SortColumn,
    #[serde(default = "default_sort_direction")]
    sort_direction: SortDirection,
    #[serde(default)]
    selected_track: usize,
    #[serde(default)]
    table_scroll_top: f32,
    #[serde(default)]
    table_horizontal_scroll: f32,
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
            visible_artist_table_columns: default_visible_artist_table_columns(),
            visible_album_table_columns: default_visible_album_table_columns(),
            visible_genre_table_columns: default_visible_genre_table_columns(),
            page: default_page(),
            tabs: Vec::new(),
            active_tab_id: None,
            artist_view_mode: default_browse_view_mode(),
            album_view_mode: default_browse_view_mode(),
            genre_view_mode: default_browse_view_mode(),
            artist_table_sort_column: default_artist_table_sort_column(),
            artist_table_sort_direction: default_sort_direction(),
            album_table_sort_column: default_album_table_sort_column(),
            album_table_sort_direction: default_sort_direction(),
            genre_table_sort_column: default_genre_table_sort_column(),
            genre_table_sort_direction: default_sort_direction(),
            genre_grid_scroll_top: 0.0,
            artist_grid_scroll_top: 0.0,
            artist_table_scroll_top: 0.0,
            album_grid_scroll_top: 0.0,
            album_table_scroll_top: 0.0,
            playback_history: Vec::new(),
            playing_track_path: None,
            online_metadata_mode: default_online_metadata_mode(),
            // Fresh app state already builds the right default layout,
            // so the migration is a no-op. Mark it done so we don't
            // re-run it spuriously on the second launch.
            liked_column_migrated: true,
        }
    }
}

fn default_online_metadata_mode() -> OnlineMetadataMode {
    OnlineMetadataMode::Off
}

fn default_page() -> Page {
    Page::Library
}

fn default_browse_view_mode() -> BrowseViewMode {
    BrowseViewMode::Grid
}

fn default_sort_column() -> SortColumn {
    SortColumn::Index
}

fn default_sort_direction() -> SortDirection {
    SortDirection::Ascending
}

fn default_volume() -> f32 {
    0.75
}

fn default_visible_table_columns() -> Vec<TableColumn> {
    vec![
        TableColumn::Index,
        TableColumn::Liked,
        TableColumn::Artwork,
        TableColumn::Title,
        TableColumn::Artist,
        TableColumn::Album,
        TableColumn::Genre,
        TableColumn::TrackNumber,
        TableColumn::Bitrate,
        TableColumn::FileSize,
        TableColumn::Year,
        TableColumn::DateAdded,
        TableColumn::Duration,
    ]
}

fn default_visible_artist_table_columns() -> Vec<ArtistTableColumn> {
    vec![
        ArtistTableColumn::Artwork,
        ArtistTableColumn::Artist,
        ArtistTableColumn::Albums,
        ArtistTableColumn::Tracks,
        ArtistTableColumn::Duration,
    ]
}

fn default_visible_album_table_columns() -> Vec<AlbumTableColumn> {
    vec![
        AlbumTableColumn::Artwork,
        AlbumTableColumn::Album,
        AlbumTableColumn::Artist,
        AlbumTableColumn::Year,
        AlbumTableColumn::Tracks,
        AlbumTableColumn::Duration,
    ]
}

fn default_visible_genre_table_columns() -> Vec<GenreTableColumn> {
    vec![
        GenreTableColumn::Genre,
        GenreTableColumn::Artists,
        GenreTableColumn::Albums,
        GenreTableColumn::Tracks,
        GenreTableColumn::Duration,
    ]
}

fn default_artist_table_sort_column() -> ArtistTableColumn {
    ArtistTableColumn::Artist
}

fn default_album_table_sort_column() -> AlbumTableColumn {
    AlbumTableColumn::Artist
}

fn default_genre_table_sort_column() -> GenreTableColumn {
    GenreTableColumn::Genre
}

const ALL_ARTIST_TABLE_COLUMNS: &[ArtistTableColumn] = &[
    ArtistTableColumn::Artwork,
    ArtistTableColumn::Artist,
    ArtistTableColumn::Albums,
    ArtistTableColumn::Tracks,
    ArtistTableColumn::Duration,
];

const ALL_ALBUM_TABLE_COLUMNS: &[AlbumTableColumn] = &[
    AlbumTableColumn::Artwork,
    AlbumTableColumn::Album,
    AlbumTableColumn::Artist,
    AlbumTableColumn::Year,
    AlbumTableColumn::Tracks,
    AlbumTableColumn::Duration,
];

const ALL_GENRE_TABLE_COLUMNS: &[GenreTableColumn] = &[
    GenreTableColumn::Genre,
    GenreTableColumn::Artists,
    GenreTableColumn::Albums,
    GenreTableColumn::Tracks,
    GenreTableColumn::Duration,
];

/// Memoized output of `artist_indices_for_search_query` /
/// `album_indices_for_search_query`. The cache stores both the filtered
/// index list and a "generation" stamp so the consumer can also derive
/// scrollbar marker labels without recomputing the filter.
#[derive(Default)]
pub(crate) struct BrowseFilterCache {
    /// `(query, source_generation)` that produced `indices`. `None`
    /// means the cache has never been populated; any new query or
    /// mutation of the source `Vec` invalidates the entry.
    key: Option<(String, u64)>,
    indices: Vec<usize>,
}

impl BrowseFilterCache {
    fn invalidate(&mut self) {
        self.key = None;
        self.indices.clear();
    }
}

/// Build the `path -> index` reverse map from a fresh tracks slice.
/// Reused by the constructor and by every code path that bulk-replaces
/// `self.tracks` (full library reload, post-scan refresh, etc).
fn build_track_path_index(tracks: &[Track]) -> HashMap<PathBuf, usize> {
    tracks
        .iter()
        .enumerate()
        .map(|(ix, track)| (track.path.clone(), ix))
        .collect()
}

fn genre_names_for(raw: &str) -> Vec<String> {
    raw.split([';', '/', ',', '|', '+'])
        .map(str::trim)
        .filter(|genre| !genre.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn genre_key_for(value: &str) -> String {
    value.trim().to_lowercase()
}

fn genre_searchable_lower(
    name: &str,
    artists: &[String],
    albums: &[GenreAlbumSummary],
    artist_count: usize,
    album_count: usize,
    track_count: usize,
) -> String {
    let mut value = format!("{name} {artist_count} {album_count} {track_count}");
    for artist in artists {
        value.push(' ');
        value.push_str(artist);
    }
    for album in albums {
        value.push(' ');
        value.push_str(&album.title);
        value.push(' ');
        value.push_str(&album.artist);
    }
    value.to_lowercase()
}

fn blend_rgb(left: u32, right: u32, right_weight: f32) -> u32 {
    let right_weight = right_weight.clamp(0.0, 1.0);
    let left_weight = 1.0 - right_weight;
    let channel = |shift| {
        let left = ((left >> shift) & 0xff_u32) as f32;
        let right = ((right >> shift) & 0xff_u32) as f32;
        (left * left_weight + right * right_weight).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn old_default_visible_table_columns() -> Vec<TableColumn> {
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

/// Default visible columns for the layout that shipped between the
/// Genre addition and the Liked column. Used by `sanitize_visible_columns`
/// to detect users on that exact default and migrate them forward
/// without disturbing customised layouts.
fn previous_default_visible_table_columns() -> Vec<TableColumn> {
    vec![
        TableColumn::Index,
        TableColumn::Artwork,
        TableColumn::Title,
        TableColumn::Artist,
        TableColumn::Album,
        TableColumn::Genre,
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
    TableColumn::Genre,
    TableColumn::TrackNumber,
    TableColumn::Format,
    TableColumn::Bitrate,
    TableColumn::FileSize,
    TableColumn::Year,
    TableColumn::DateAdded,
    TableColumn::Plays,
    TableColumn::Duration,
    TableColumn::Liked,
];

const INDEX_COL_W: f32 = 34.0;
const ART_COL_W: f32 = 32.0;
const TITLE_COL_W: f32 = 188.0;
const ARTIST_COL_W: f32 = 160.0;
const ALBUM_COL_W: f32 = 230.0;
const GENRE_COL_W: f32 = 120.0;
const TRACK_NO_COL_W: f32 = 58.0;
const FMT_COL_W: f32 = 70.0;
const BITRATE_COL_W: f32 = 86.0;
const FILE_SIZE_COL_W: f32 = 86.0;
const YEAR_COL_W: f32 = 72.0;
const DATE_ADDED_COL_W: f32 = 96.0;
const PLAYS_COL_W: f32 = 82.0;
const TIME_COL_W: f32 = 64.0;
const LIKED_COL_W: f32 = 24.0;
const TABLE_ROW_H: f32 = 32.0;
const LEFT_SIDEBAR_W: f32 = 220.0;
const RIGHT_SIDEBAR_W: f32 = 300.0;
const WAVEFORM_SEGMENTS: usize = 360;
const WAVEFORM_CACHE_VERSION: u32 = 1;
const WAVEFORM_SAMPLED_MIN_DURATION: Duration = Duration::from_secs(30);
const WAVEFORM_MIN_SAMPLE_FRAMES: usize = 256;
const WAVEFORM_MAX_SAMPLE_FRAMES: usize = 2048;
/// How long the per-column morph animation runs when the active
/// waveform changes (loading shimmer → loaded peaks, or song A →
/// song B). 400ms ease-out reads as snappy without dragging.
const WAVEFORM_MORPH_DURATION: Duration = Duration::from_millis(400);
const PLAYER_VOLUME_BAR_W: f32 = 104.0;
const TABLE_SCROLLBAR_W: f32 = 54.0;
const TABLE_SCROLLBAR_TRACK_W: f32 = 6.0;
const TABLE_SCROLLBAR_MARGIN: f32 = 4.0;
const TABLE_SCROLLBAR_MIN_THUMB_H: f32 = 32.0;
const TABLE_SCROLLBAR_MAX_MARKERS: usize = 28;
const TABLE_SCROLL_IDLE_DELAY: Duration = Duration::from_millis(120);
const SEARCH_DEBOUNCE_DELAY: Duration = Duration::from_millis(90);
const SCAN_BROWSE_RELOAD_INTERVAL: Duration = Duration::from_millis(750);
const FAST_SCROLL_OVERSCAN_ROWS: usize = 4;
const BROWSE_GRID_CARD_W: f32 = 154.0;
const GENRE_GRID_CARD_W: f32 = 330.0;
const BROWSE_GRID_GAP: f32 = 16.0;
const BROWSE_GRID_PAD_X: f32 = 32.0;

/// Pixels the tab-bar arrow buttons (`‹` / `›`) move the strip per
/// click. Sized so a click reveals roughly one full tab worth of
/// content (max tab width is 176px), which feels right for keyboard-
/// less navigation. Wheel scrolling uses the gpui-builtin per-line
/// stepping instead.
const TAB_BAR_ARROW_STEP: f32 = 160.0;

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
    artist_table_column_widths: ArtistTableColumnWidths,
    album_table_column_widths: AlbumTableColumnWidths,
    genre_table_column_widths: GenreTableColumnWidths,
    scan_error_column_widths: ScanErrorColumnWidths,
    playback_history_played_at_width: f32,
    column_resize: Option<ColumnResize>,
    visible_columns: Vec<TableColumn>,
    visible_artist_columns: Vec<ArtistTableColumn>,
    visible_album_columns: Vec<AlbumTableColumn>,
    visible_genre_columns: Vec<GenreTableColumn>,
    column_menu_open: bool,
    column_menu_kind: ColumnMenuKind,
    column_menu_x: f32,
    column_menu_y: f32,
    tabs: Vec<BrowseTab>,
    active_tab: usize,
    next_tab_id: u64,
    back_history: Vec<NavigationEntry>,
    forward_history: Vec<NavigationEntry>,
    /// LIFO stack of recently-closed tabs (most-recent at the back).
    /// Capped at `CLOSED_TABS_MAX`; oldest entries are evicted from the
    /// front when the cap is exceeded. Used by the Ctrl+Shift+T
    /// reopen-closed-tab action. The first/anchor "All Music" tab
    /// cannot be closed and therefore never lands here.
    closed_tabs: Vec<ClosedTab>,
    /// Horizontal scroll handle for the tab bar. Lets the tab strip
    /// overflow when too many tabs are open: `track_scroll` registers
    /// the inner row, which gives us viewport bounds, max scroll, and
    /// `scroll_to_item` for auto-scrolling the active tab into view
    /// after Ctrl+Tab / sidebar / drag-open / reopen actions.
    tab_bar_scroll_handle: gpui::ScrollHandle,
    /// Index of the active tab the last time we requested an
    /// auto-scroll into view. Used so we only call `scroll_to_item`
    /// when the active tab actually changes (not on every render),
    /// preserving the user's manual horizontal-scroll position.
    last_scrolled_active_tab: usize,
    hovered_tooltip_id: Option<SharedString>,
    tooltip: Option<Tooltip>,
    tooltip_generation: u64,
    /// Cached resolution of `self.player.read(cx).playing_track_path()`
    /// against `self.track_path_index`. Refreshed when the player
    /// emits [`player::PlayerEvent::PlayingTrackChanged`] and when the
    /// track list mutates (scan apply, library reload). Cross-region
    /// readers (table active-row, history page) check both this *and*
    /// `self.player.read(cx).is_playing()` to decide whether to draw
    /// the playing-row highlight. Stays at the last valid value when
    /// nothing is playing so seek-and-resume still works.
    playing_track: usize,
    context_menu_track: Option<usize>,
    context_menu_position: Point<Pixels>,
    /// Index of the track whose Liked-column heart cell is currently
    /// hovered. Stored on the app so the cell renderer can swap the
    /// outline-heart icon for the accent-stroke variant on hover. Kept
    /// outside the per-track state because only one cell is hovered at
    /// a time and we want the previously hovered cell to repaint
    /// without iterating the table.
    hovered_liked_track: Option<usize>,
    /// Right-click context menu state for sidebar playlist nav items.
    /// `None` means no menu is open. The position is the mouse-down
    /// location at the moment of the right-click; the menu anchors there.
    playlist_context_menu: Option<PlaylistContextMenu>,
    /// Inline rename state for a sidebar playlist. While `Some`, the
    /// playlist nav item swaps its label for an editable input bound to
    /// `rename_input`. Enter commits, Escape cancels, click-away cancels.
    playlist_rename: Option<PlaylistRename>,
    /// Single-shot focus handle for the rename input. Re-created when a
    /// rename starts; dropped when it ends.
    playlist_rename_focus_handle: Option<FocusHandle>,
    /// Delete-playlist confirmation modal state. `Some(playlist_ix)`
    /// shows a centered dialog asking for confirmation; `None` means no
    /// dialog is open.
    playlist_delete_confirm: Option<usize>,
    tracks: Vec<Track>,
    /// Reverse-index from `Track::path` to its position in `tracks`.
    /// Used by the scanner to upsert known tracks in O(1) instead of
    /// O(N) -- a hot path during cold scans of large libraries.
    /// Always kept in sync with `tracks` via `Self::rebuild_track_path_index`
    /// or the in-place `track_path_index_*` helpers.
    track_path_index: HashMap<PathBuf, usize>,
    /// Cached sum of `Track::file_size` over all entries in `tracks`.
    /// Maintained incrementally so the sidebar footer can render the
    /// "12.3 GB" label without iterating the full track list every
    /// frame. Always equals `tracks.iter().map(|t| t.file_size).sum()`.
    library_size_bytes: u64,
    artists: Vec<Artist>,
    albums: Vec<Album>,
    genres: Vec<Genre>,
    /// Bumped whenever `self.artists` is reassigned. Used as a cache
    /// generation token by `artist_filter_cache` so a new artist load
    /// invalidates any memoized filter result without us needing to
    /// touch the cache directly from every mutation site.
    artists_generation: u64,
    /// Bumped whenever `self.albums` is reassigned.
    albums_generation: u64,
    /// Aggregate total play duration per `Artist::artist_id`. Rebuilt
    /// from `self.tracks` whenever the track list changes; consumed
    /// by the Artists table Duration column and its sort comparator.
    /// Mirrors the genre duration aggregation since `CatalogArtist`
    /// does not carry a duration field.
    artist_durations: HashMap<i64, Duration>,
    /// Aggregate total play duration per `Album::album_id`. See
    /// [`Self::artist_durations`] for the rationale.
    album_durations: HashMap<i64, Duration>,
    /// Bumped whenever derived genre aggregates are rebuilt.
    genres_generation: u64,
    /// Memoized filter results for the Browse pages. Key is the
    /// `browse_search_query` at the time of computation; if the query
    /// hasn't changed and `artists` hasn't been mutated, the cached
    /// indices are reused on every repaint instead of being recomputed
    /// up to three times per frame (artists grid + scrollbar markers +
    /// floating drag label).
    artist_filter_cache: RefCell<BrowseFilterCache>,
    album_filter_cache: RefCell<BrowseFilterCache>,
    genre_filter_cache: RefCell<BrowseFilterCache>,
    artist_view_mode: BrowseViewMode,
    album_view_mode: BrowseViewMode,
    genre_view_mode: BrowseViewMode,
    artist_table_sort_column: ArtistTableColumn,
    artist_table_sort_direction: SortDirection,
    album_table_sort_column: AlbumTableColumn,
    album_table_sort_direction: SortDirection,
    genre_table_sort_column: GenreTableColumn,
    genre_table_sort_direction: SortDirection,
    queue: Vec<usize>,
    library_roots: Vec<PathBuf>,
    playlists: Vec<Playlist>,
    playback_history: Vec<PlaybackHistoryEntry>,
    theme_id: String,
    themes: Vec<Theme>,
    library_root_label: String,
    library_status: String,
    online_metadata_mode: OnlineMetadataMode,
    scan_progress: ScanProgress,
    scan_errors: Vec<IndexingError>,
    scan_changed_tracks: bool,
    last_scan_browse_reload: Option<Instant>,
    is_scanning: bool,
    metadata_activity: CatalogMetadataActivity,
    table_scrollbar_drag: Option<TableScrollbarDrag>,
    table_horizontal_scrollbar_drag: Option<TableHorizontalScrollbarDrag>,
    browse_scrollbar_drag: Option<BrowseScrollbarDrag>,
    artist_grid_scroll_handle: UniformListScrollHandle,
    artist_table_scroll_handle: UniformListScrollHandle,
    album_grid_scroll_handle: UniformListScrollHandle,
    album_table_scroll_handle: UniformListScrollHandle,
    genre_grid_scroll_handle: UniformListScrollHandle,
    scan_errors_scroll_handle: UniformListScrollHandle,
    playback_history_scroll_handle: UniformListScrollHandle,
    liked_scroll_handle: UniformListScrollHandle,
    table_is_scrolling: bool,
    table_scroll_generation: u64,
    catalog: Option<CatalogStore>,
    _library_watcher: Option<LibraryWatcher>,
    metadata_event_tx: mpsc::Sender<MetadataEvent>,
    metadata_demand_queue: Arc<Mutex<MetadataDemandQueue>>,
    metadata_status_expanded: bool,
    _metadata_worker: Option<MetadataWorker>,
    /// the architectural design notes — in short, this entity owns the
    /// audio backend, current track identity, volume, output device
    /// picker, waveform cache, and now-playing hover state, so the
    /// per-second playback tick only invalidates the player bar
    /// instead of the whole app tree.
    ///
    /// `TempoApp` retains the `tracks`/`tabs`/`queue`/`playback_history`
    /// state and orchestrates "play this track" by resolving an index
    /// to a path and calling `player.update(cx, |p, cx|
    /// p.start_playback(...))`.
    /// The audio playback subsystem. See [`player::PlayerEntity`] for
    /// the architectural design notes — in short, this entity owns the
    /// audio backend, current track identity, volume, output device
    /// picker, waveform cache, and now-playing hover state, so the
    /// per-second playback tick only invalidates the player bar
    /// instead of the whole app tree.
    ///
    /// `TempoApp` retains the `tracks`/`tabs`/`queue`/`playback_history`
    /// state and orchestrates "play this track" by resolving an index
    /// to a path and calling `player.update(cx, |p, cx|
    /// p.start_playback(...))`.
    player: Entity<player::PlayerEntity>,
    /// Denormalized mirror of `self.player.read(cx).volume()` — kept
    /// in sync via the [`player::PlayerEvent::StateMutated`] handler.
    /// Exists so [`Self::build_app_state_snapshot`] can read from
    /// `&self` only, sparing the dozens of `save_app_state()` callsites
    /// across settings / playlist / tab code from having to take
    /// `cx`. Authoritative state is on `self.player`; this field is
    /// for serialization only.
    volume_snapshot: f32,
    /// Denormalized mirror of `self.player.read(cx).output_device()`,
    /// for the same reason as `volume_snapshot`.
    output_device_snapshot: Option<String>,
    /// Held subscription that forwards [`player::PlayerEvent`]s into
    /// `TempoApp::handle_player_event`. Dropping this subscription
    /// would silently break cross-region updates (e.g. table active
    /// row would stop refreshing on track change), so it lives on the
    /// app for the app's lifetime.
    _player_subscription: Subscription,
    _save_on_quit: Option<Subscription>,
}

impl TempoApp {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _startup_span = perf::span("startup.total", "");
        let focus_handle = cx.focus_handle();
        let search_focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let state = perf::time("startup.load_app_state", "", Self::load_app_state);
        let themes = perf::time("startup.load_themes", "", bundled_themes);
        let theme_id = perf::time("startup.resolve_theme", "", || {
            resolve_theme_id(state.theme_id, &themes)
        });
        let roots = perf::time("startup.default_roots", "", || {
            Self::default_library_roots(&state.library_roots)
        });
        let library_root_label = Self::library_root_label(&roots);
        let (catalog, catalog_status) = match perf::time_result(
            "startup.catalog_open_default",
            "",
            CatalogStore::open_default,
        ) {
            Ok(catalog) => (Some(catalog), None),
            Err(error) => (None, Some(format!("Catalog cache unavailable: {error:#}"))),
        };
        let (cached_tracks, cached_artists, cached_albums) =
            Self::load_browse_caches_for_startup(catalog.as_ref(), &roots);
        perf::event(
            "startup.cached_tracks.count",
            format!("tracks={}", cached_tracks.len()),
        );
        perf::event(
            "startup.cached_artists.count",
            format!("artists={}", cached_artists.len()),
        );
        perf::event(
            "startup.cached_albums.count",
            format!("albums={}", cached_albums.len()),
        );
        let cached_genres = perf::time(
            "startup.cached_genres.build",
            format!(
                "tracks={} albums={}",
                cached_tracks.len(),
                cached_albums.len()
            ),
            || Self::build_genres(&cached_tracks, &cached_albums),
        );
        let (event_tx, event_rx) = mpsc::channel();
        let (metadata_event_tx, metadata_event_rx) = mpsc::channel();
        let metadata_demand_queue = Arc::new(Mutex::new(MetadataDemandQueue::default()));
        let (mut library_status, library_watcher) = perf::time(
            "startup.start_watcher",
            format!("roots={}", roots.len()),
            || Self::start_watcher_for_roots(&roots, event_tx, catalog.clone()),
        );
        if let Some(catalog_status) = catalog_status {
            library_status = catalog_status;
        }
        let playlists = state.playlists;
        let volume = state.volume.clamp(0.0, 1.0);
        let mut visible_columns = Self::sanitize_visible_columns(state.visible_table_columns);
        let visible_artist_columns =
            Self::sanitize_artist_table_columns(state.visible_artist_table_columns);
        let visible_album_columns =
            Self::sanitize_album_table_columns(state.visible_album_table_columns);
        let visible_genre_columns =
            Self::sanitize_genre_table_columns(state.visible_genre_table_columns);
        // One-shot migration: relocate the formerly inert `Liked`
        // column from the trailing position into the new default slot
        // right after `#`. Gated on a saved-state flag so subsequent
        // user-driven drags-to-end persist across restarts; once the
        // migration has run on a state file we never touch the
        // ordering again.
        if !state.liked_column_migrated {
            Self::migrate_legacy_liked_position(&mut visible_columns);
        }
        // Resolve the active theme's colors once up front so we can
        // hand them to `PlayerEntity::new` (which renders entirely
        // from its own state, including theme colors).
        let initial_theme_colors = themes
            .iter()
            .find(|theme| theme.id == theme_id)
            .or_else(|| themes.first())
            .map(|theme| theme.colors)
            .expect("at least one theme is always bundled");
        // Construct the audio playback entity. Audio backend init is
        // deferred to `start_deferred_playback_init` (called below
        // after the entity is in the entity map) so the 25–50ms cpal
        // device-enumeration cost doesn't block the first frame.
        let player = cx.new(|player_cx| {
            player::PlayerEntity::new(
                volume,
                state.output_device.clone(),
                catalog.clone(),
                initial_theme_colors,
                player_cx,
            )
        });
        // Subscribe to typed events for cross-region coordination
        // (track change, play/pause flip, auto-advance, etc.); the
        // held subscription lives on `TempoApp` for its lifetime so
        // events keep flowing until the app exits.
        //
        // We deliberately do *not* `cx.observe(&player, ...)` — the
        // player owns its own `Render` impl now, so its per-frame
        // `cx.notify()` calls invalidate only the player-bar subtree.
        // That's the whole point of the Entity split: the 1 Hz
        // playback tick stops repainting the table/sidebar/grids.
        let player_subscription = cx.subscribe(&player, |this, _player, event, cx| {
            this.handle_player_event(event, cx);
        });

        let initial_page = if roots.is_empty() {
            Page::Settings
        } else {
            state.page
        };
        // Resolve the now-playing track index by path so a rescanned library
        // (which can shift indices) still highlights the correct row in the
        // bottom-left now-playing area. Falls back to the most recent
        // playback-history entry, then to the first track.
        let initial_playing_track = state
            .playing_track_path
            .as_deref()
            .and_then(|saved_path| {
                cached_tracks
                    .iter()
                    .position(|track| track.path == saved_path)
            })
            .or_else(|| {
                state.playback_history.last().and_then(|entry| {
                    cached_tracks
                        .iter()
                        .position(|track| track.path == entry.track_path)
                })
            })
            .unwrap_or(0);
        // Seed the player's render snapshot with whichever track will
        // be visible in the player bar at startup. If the library is
        // empty `cached_tracks` is empty and the player renders the
        // empty placeholder — `set_playing_track(None)` is the
        // default but we set it explicitly for clarity.
        let initial_player_snapshot = cached_tracks
            .get(initial_playing_track)
            .map(player::PlayingTrackSnapshot::from_track);
        player.update(cx, |player, _| {
            player.set_playing_track(initial_player_snapshot);
        });
        let mut tabs = Self::restore_tabs(&state.tabs);
        if tabs.is_empty() {
            tabs.push(BrowseTab::library(1));
        }
        let active_tab = state
            .active_tab_id
            .and_then(|tab_id| tabs.iter().position(|tab| tab.id == tab_id))
            .unwrap_or(0);
        let next_tab_id = tabs
            .iter()
            .map(|tab| tab.id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(2);
        let artist_grid_scroll_top = state.artist_grid_scroll_top;
        let artist_table_scroll_top = state.artist_table_scroll_top;
        let album_grid_scroll_top = state.album_grid_scroll_top;
        let album_table_scroll_top = state.album_table_scroll_top;
        let genre_grid_scroll_top = state.genre_grid_scroll_top;
        let save_on_quit = cx.on_app_quit(|app, cx| {
            // Refresh denormalized snapshots one last time so the
            // shutdown save reflects any in-flight player state.
            app.refresh_player_state_snapshot(cx);
            perf::event("shutdown.app_quit", "saving_state");
            // Synchronous save at shutdown so the latest state is always
            // flushed even if the debounce window for the background save
            // thread hadn't elapsed yet.
            perf::time("shutdown.save_app_state", "", || app.save_app_state_now());
            // `PRAGMA optimize` is cheap on a clean shutdown and lets
            // SQLite refresh its query-plan statistics for the next
            // launch. Failures are silently ignored.
            if let Some(catalog) = app.catalog.as_ref() {
                perf::time("shutdown.catalog_optimize", "", || catalog.run_optimize());
            }
            async {}
        });

        let mut app = Self {
            focus_handle,
            search_focus_handle,
            search_input: TextInputState::default(),
            browse_search_query: String::new(),
            search_debounce_generation: 0,
            page: Self::resolved_page_for_roots(initial_page, &roots),
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            column_widths: ColumnWidths::default(),
            artist_table_column_widths: ArtistTableColumnWidths::default(),
            album_table_column_widths: AlbumTableColumnWidths::default(),
            genre_table_column_widths: GenreTableColumnWidths::default(),
            scan_error_column_widths: ScanErrorColumnWidths::default(),
            playback_history_played_at_width: 178.0,
            column_resize: None,
            visible_columns,
            visible_artist_columns,
            visible_album_columns,
            visible_genre_columns,
            column_menu_open: false,
            column_menu_kind: ColumnMenuKind::Tracks,
            column_menu_x: 0.0,
            column_menu_y: 0.0,
            tabs,
            active_tab,
            next_tab_id,
            back_history: Vec::new(),
            forward_history: Vec::new(),
            closed_tabs: Vec::new(),
            tab_bar_scroll_handle: gpui::ScrollHandle::new(),
            last_scrolled_active_tab: active_tab,
            hovered_tooltip_id: None,
            tooltip: None,
            tooltip_generation: 0,
            playing_track: initial_playing_track,
            context_menu_track: None,
            context_menu_position: Point::default(),
            hovered_liked_track: None,
            playlist_context_menu: None,
            playlist_rename: None,
            playlist_rename_focus_handle: None,
            playlist_delete_confirm: None,
            track_path_index: build_track_path_index(&cached_tracks),
            library_size_bytes: cached_tracks.iter().map(|track| track.file_size).sum(),
            tracks: cached_tracks,
            artists: cached_artists,
            albums: cached_albums,
            genres: cached_genres,
            artists_generation: 0,
            albums_generation: 0,
            artist_durations: HashMap::new(),
            album_durations: HashMap::new(),
            genres_generation: 0,
            artist_filter_cache: RefCell::new(BrowseFilterCache::default()),
            album_filter_cache: RefCell::new(BrowseFilterCache::default()),
            genre_filter_cache: RefCell::new(BrowseFilterCache::default()),
            artist_view_mode: state.artist_view_mode,
            album_view_mode: state.album_view_mode,
            genre_view_mode: state.genre_view_mode,
            artist_table_sort_column: state.artist_table_sort_column,
            artist_table_sort_direction: state.artist_table_sort_direction,
            album_table_sort_column: state.album_table_sort_column,
            album_table_sort_direction: state.album_table_sort_direction,
            genre_table_sort_column: state.genre_table_sort_column,
            genre_table_sort_direction: state.genre_table_sort_direction,
            queue: Vec::new(),
            library_roots: roots,
            playlists,
            theme_id,
            themes,
            library_root_label,
            library_status,
            online_metadata_mode: state.online_metadata_mode,
            scan_progress: ScanProgress::default(),
            scan_errors: Vec::new(),
            scan_changed_tracks: false,
            last_scan_browse_reload: None,
            is_scanning: false,
            metadata_activity: CatalogMetadataActivity::default(),
            table_scrollbar_drag: None,
            table_horizontal_scrollbar_drag: None,
            browse_scrollbar_drag: None,
            artist_grid_scroll_handle: UniformListScrollHandle::new(),
            artist_table_scroll_handle: UniformListScrollHandle::new(),
            album_grid_scroll_handle: UniformListScrollHandle::new(),
            album_table_scroll_handle: UniformListScrollHandle::new(),
            genre_grid_scroll_handle: UniformListScrollHandle::new(),
            scan_errors_scroll_handle: UniformListScrollHandle::new(),
            playback_history_scroll_handle: UniformListScrollHandle::new(),
            liked_scroll_handle: UniformListScrollHandle::new(),
            table_is_scrolling: false,
            table_scroll_generation: 0,
            catalog,
            playback_history: state.playback_history,
            _library_watcher: library_watcher,
            metadata_event_tx,
            metadata_demand_queue,
            metadata_status_expanded: false,
            _metadata_worker: None,
            player,
            volume_snapshot: volume,
            output_device_snapshot: state.output_device.clone(),
            _player_subscription: player_subscription,
            _save_on_quit: Some(save_on_quit),
        };

        app.artist_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-artist_grid_scroll_top.max(0.0))));
        app.artist_table_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-artist_table_scroll_top.max(0.0))));
        app.album_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-album_grid_scroll_top.max(0.0))));
        app.album_table_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-album_table_scroll_top.max(0.0))));
        app.genre_grid_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-genre_grid_scroll_top.max(0.0))));

        perf::time(
            "startup.initial_artist_album_durations",
            format!("tracks={}", app.tracks.len()),
            || app.rebuild_artist_album_durations(),
        );
        perf::time(
            "startup.initial_index_rebuild",
            format!("tracks={} tabs={}", app.tracks.len(), app.tabs.len()),
            || app.invalidate_track_indices(),
        );
        perf::time("startup.clamp_track_indices", "", || {
            app.clamp_track_indices(cx)
        });
        perf::time("startup.start_library_event_loop", "", || {
            app.start_library_event_loop(event_rx, cx)
        });
        perf::time("startup.start_metadata_event_loop", "", || {
            app.start_metadata_event_loop(metadata_event_rx, cx)
        });
        perf::time("startup.start_metadata_activity_poll", "", || {
            app.start_metadata_activity_poll(cx)
        });
        perf::time("startup.start_playback_tick", "", || {
            app.start_playback_tick(cx)
        });
        perf::time("startup.start_deferred_playback_init", "", || {
            app.start_deferred_playback_init(cx)
        });
        // If the snapshot didn't exist or was stale, build it now in the
        // background so the next launch hits the fast path.
        if !tempo::snapshot::snapshot_path(
            app.catalog
                .as_ref()
                .map(|catalog| catalog.cache_dir())
                .unwrap_or(std::path::Path::new("")),
        )
        .exists()
        {
            app.spawn_snapshot_rebuild("initial_build");
        }
        app.restart_metadata_worker();
        app
    }

    fn set_online_metadata_mode(&mut self, mode: OnlineMetadataMode) {
        if self.online_metadata_mode == mode {
            return;
        }

        self.online_metadata_mode = mode;
        if mode == OnlineMetadataMode::Off {
            self.metadata_activity = CatalogMetadataActivity::default();
        }
        self.restart_metadata_worker();
        self.save_app_state();
    }

    fn queue_artist_metadata_demand(&self, artist_id: i64) {
        if self.online_metadata_mode != OnlineMetadataMode::Automatic {
            return;
        }
        let Some(catalog) = self.catalog.clone() else {
            return;
        };
        let Ok(mut demand_queue) = self.metadata_demand_queue.lock() else {
            return;
        };
        if !demand_queue.artists.insert(artist_id) {
            return;
        }
        drop(demand_queue);

        std::thread::Builder::new()
            .name("tempo-metadata-demand".into())
            .spawn(move || {
                if let Err(error) = catalog.enqueue_artist_metadata_demand(artist_id) {
                    perf::event(
                        "metadata.demand.artist_error",
                        format!("artist_id={artist_id} error={error:#}"),
                    );
                }
            })
            .ok();
    }

    fn queue_album_cover_demand(&self, album_id: i64) {
        if self.online_metadata_mode != OnlineMetadataMode::Automatic {
            return;
        }
        let Some(catalog) = self.catalog.clone() else {
            return;
        };
        let Ok(mut demand_queue) = self.metadata_demand_queue.lock() else {
            return;
        };
        if !demand_queue.albums.insert(album_id) {
            return;
        }
        drop(demand_queue);

        std::thread::Builder::new()
            .name("tempo-metadata-demand".into())
            .spawn(move || {
                if let Err(error) = catalog.enqueue_album_cover_demand(album_id) {
                    perf::event(
                        "metadata.demand.album_error",
                        format!("album_id={album_id} error={error:#}"),
                    );
                }
            })
            .ok();
    }

    fn rebuild_genres(&mut self) {
        self.genres = Self::build_genres(&self.tracks, &self.albums);
        self.genres_generation = self.genres_generation.wrapping_add(1);
        self.genre_filter_cache.borrow_mut().invalidate();
        self.rebuild_artist_album_durations();
    }

    /// Recompute `artist_durations` and `album_durations` from the
    /// current `self.tracks`. Called from the same hot-paths that
    /// trigger `rebuild_genres` so the artist/album Duration columns
    /// stay in sync with the rest of the derived browse data.
    fn rebuild_artist_album_durations(&mut self) {
        let mut artist_durations: HashMap<i64, Duration> = HashMap::new();
        let mut album_durations: HashMap<i64, Duration> = HashMap::new();
        for track in &self.tracks {
            if let Some(artist_id) = track.artist_id {
                *artist_durations
                    .entry(artist_id)
                    .or_insert_with(|| Duration::from_secs(0)) += track.duration_value;
            }
            if let Some(album_id) = track.album_id {
                *album_durations
                    .entry(album_id)
                    .or_insert_with(|| Duration::from_secs(0)) += track.duration_value;
            }
        }
        self.artist_durations = artist_durations;
        self.album_durations = album_durations;
    }

    pub(super) fn artist_total_duration(&self, artist_id: i64) -> Duration {
        self.artist_durations
            .get(&artist_id)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn album_total_duration(&self, album_id: i64) -> Duration {
        self.album_durations
            .get(&album_id)
            .copied()
            .unwrap_or_default()
    }

    fn build_genres(tracks: &[Track], albums: &[Album]) -> Vec<Genre> {
        struct AlbumAggregate {
            album_id: Option<i64>,
            title: String,
            artist: String,
            artwork_path: Option<PathBuf>,
            track_count: usize,
            play_count: u32,
            initials: String,
            color: u32,
        }

        struct GenreAggregate {
            key: String,
            name: String,
            artist_names: HashSet<String>,
            album_keys: HashSet<String>,
            albums: HashMap<String, AlbumAggregate>,
            track_count: usize,
            duration_value: Duration,
        }

        let album_by_id = albums
            .iter()
            .map(|album| (album.album_id, album))
            .collect::<HashMap<_, _>>();
        let album_by_name = albums
            .iter()
            .map(|album| {
                (
                    format!(
                        "{}:{}",
                        genre_key_for(&album.artist),
                        genre_key_for(&album.title)
                    ),
                    album,
                )
            })
            .collect::<HashMap<_, _>>();

        let mut genres = HashMap::<String, GenreAggregate>::new();
        for track in tracks {
            for genre_name in genre_names_for(&track.genre) {
                let genre_key = genre_key_for(&genre_name);
                if genre_key.is_empty() {
                    continue;
                }

                let aggregate = genres
                    .entry(genre_key.clone())
                    .or_insert_with(|| GenreAggregate {
                        key: genre_key.clone(),
                        name: genre_name.clone(),
                        artist_names: HashSet::new(),
                        album_keys: HashSet::new(),
                        albums: HashMap::new(),
                        track_count: 0,
                        duration_value: Duration::from_secs(0),
                    });

                aggregate.track_count += 1;
                aggregate.duration_value += track.duration_value;
                for artist in individual_artist_names(&track.artist) {
                    aggregate.artist_names.insert(artist);
                }

                let primary_artist = primary_artist_name(&track.artist);
                let album_name_key = format!(
                    "{}:{}",
                    genre_key_for(&primary_artist),
                    genre_key_for(&track.album)
                );
                let album = track
                    .album_id
                    .and_then(|album_id| album_by_id.get(&album_id).copied())
                    .or_else(|| album_by_name.get(&album_name_key).copied());
                let album_key = track
                    .album_id
                    .map(|album_id| format!("id:{album_id}"))
                    .unwrap_or_else(|| album_name_key.clone());
                aggregate.album_keys.insert(album_key.clone());

                let album_entry = aggregate.albums.entry(album_key).or_insert_with(|| {
                    let artwork_path =
                        album
                            .and_then(|album| album.artwork_path.clone())
                            .or_else(|| match &track.artwork {
                                Some(TrackArtwork::File(path)) => Some(path.clone()),
                                Some(TrackArtwork::Embedded(_)) | None => None,
                            });
                    let title = album
                        .map(|album| album.title.clone())
                        .unwrap_or_else(|| track.album.to_string());
                    let artist = album
                        .map(|album| album.artist.clone())
                        .unwrap_or_else(|| primary_artist.clone());
                    AlbumAggregate {
                        album_id: track.album_id,
                        initials: album
                            .map(|album| album.initials.clone())
                            .unwrap_or_else(|| artwork::album_initials_for(&title, &track.title)),
                        color: album
                            .map(|album| album.color)
                            .unwrap_or_else(|| artwork::album_color_for(&title, &artist)),
                        title,
                        artist,
                        artwork_path,
                        track_count: 0,
                        play_count: 0,
                    }
                });
                album_entry.track_count += 1;
                album_entry.play_count = album_entry.play_count.saturating_add(track.plays);
                if album_entry.artwork_path.is_none() {
                    album_entry.artwork_path = album.and_then(|album| album.artwork_path.clone());
                }
            }
        }

        let mut genres = genres
            .into_values()
            .map(|genre| {
                let mut artists = genre.artist_names.into_iter().collect::<Vec<_>>();
                artists.sort_by_key(|artist| artist.to_lowercase());

                let mut albums = genre
                    .albums
                    .into_values()
                    .map(|album| GenreAlbumSummary {
                        album_id: album.album_id,
                        title: album.title,
                        artist: album.artist,
                        artwork_path: album.artwork_path,
                        track_count: album.track_count,
                        play_count: album.play_count,
                        initials: album.initials,
                        color: album.color,
                    })
                    .collect::<Vec<_>>();
                albums.sort_by(|left, right| {
                    right
                        .play_count
                        .cmp(&left.play_count)
                        .then(right.track_count.cmp(&left.track_count))
                        .then(left.artist.to_lowercase().cmp(&right.artist.to_lowercase()))
                        .then(left.title.to_lowercase().cmp(&right.title.to_lowercase()))
                });
                let top_albums = albums.iter().take(3).cloned().collect::<Vec<_>>();
                let artist_count = artists.len();
                let album_count = genre.album_keys.len();
                let searchable_lower = genre_searchable_lower(
                    &genre.name,
                    &artists,
                    &albums,
                    artist_count,
                    album_count,
                    genre.track_count,
                );
                Genre {
                    key: genre.key,
                    color: artwork::color_for(&genre.name, "genre"),
                    name: genre.name,
                    artist_count,
                    album_count,
                    track_count: genre.track_count,
                    duration_value: genre.duration_value,
                    artists,
                    albums,
                    top_albums,
                    searchable_lower,
                }
            })
            .collect::<Vec<_>>();
        genres.sort_by_key(|genre| genre.name.to_lowercase());
        genres
    }

    fn restart_metadata_worker(&mut self) {
        if let Some(worker) = self._metadata_worker.take() {
            worker.stop();
        }

        if self.online_metadata_mode != OnlineMetadataMode::Automatic {
            return;
        }

        let Some(catalog) = self.catalog.clone() else {
            self.library_status =
                "Online metadata unavailable: catalog cache is unavailable".to_string();
            return;
        };

        match MetadataWorker::start(catalog, self.metadata_event_tx.clone()) {
            Ok(worker) => {
                self._metadata_worker = Some(worker);
            }
            Err(error) => {
                self.library_status = format!("Online metadata worker failed: {error:#}");
            }
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

    /// Number of tracks currently flagged as liked. Powers the count
    /// shown next to the "Liked" sidebar entry. O(N) over the in-memory
    /// track list -- fine because the sidebar re-renders only on state
    /// changes, not per frame.
    pub(super) fn liked_track_count(&self) -> usize {
        self.tracks.iter().filter(|track| track.liked).count()
    }

    /// Toggle the liked flag for the track at `track_ix`. Updates the
    /// in-memory `Track::liked` immediately for snappy UI feedback,
    /// then persists to the catalog so the state survives restarts.
    /// Catalog persistence failures are swallowed -- the in-memory
    /// state is still authoritative for this session, and the catalog
    /// will reconverge the next time `set_liked` succeeds.
    pub(super) fn toggle_liked(&mut self, track_ix: usize) {
        let Some(track) = self.tracks.get_mut(track_ix) else {
            return;
        };
        let new_value = !track.liked;
        track.liked = new_value;
        let path = track.path.clone();

        if let Some(catalog) = self.catalog.as_ref()
            && let Err(error) = catalog.set_liked(&path, new_value)
        {
            // Best-effort persistence; log via perf event so the
            // failure is observable without hard-failing the UI.
            perf::event(
                "tempo.toggle_liked.persist_failed",
                format!("path={} error={error}", path.display()),
            );
        }
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

    /// Open the right-click context menu for a sidebar playlist nav
    /// item at the given screen position. Closes any other open menus
    /// or rename input to keep the UI in a single state.
    fn open_playlist_context_menu(&mut self, playlist_ix: usize, position: Point<Pixels>) {
        if playlist_ix >= self.playlists.len() {
            return;
        }
        self.cancel_playlist_rename();
        self.context_menu_track = None;
        self.column_menu_open = false;
        self.playlist_context_menu = Some(PlaylistContextMenu {
            playlist_ix,
            position,
        });
    }

    fn close_playlist_context_menu(&mut self) {
        self.playlist_context_menu = None;
    }

    /// Begin inline rename of a sidebar playlist. Pre-populates the
    /// input with the current name and selects it so typing replaces
    /// the whole name. `window.focus(handle)` runs on the same frame
    /// so the new input is the active focus target by the time GPUI
    /// dispatches the next key event.
    fn start_playlist_rename(
        &mut self,
        playlist_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(playlist) = self.playlists.get(playlist_ix) else {
            return;
        };
        let mut input = TextInputState::default();
        input.set_text(playlist.name.clone());
        input.select_all();
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        self.playlist_context_menu = None;
        self.playlist_delete_confirm = None;
        self.playlist_rename = Some(PlaylistRename { playlist_ix, input });
        self.playlist_rename_focus_handle = Some(focus_handle);
    }

    fn cancel_playlist_rename(&mut self) {
        self.playlist_rename = None;
        self.playlist_rename_focus_handle = None;
    }

    /// Key handler for the inline playlist rename input. Mirrors the
    /// behaviour of the library search box: Enter commits, Escape
    /// cancels, arrows/home/end/backspace/delete edit the buffer,
    /// Cmd/Ctrl+A selects all, Cmd/Ctrl+C/X/V handle clipboard.
    pub(super) fn handle_playlist_rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        let command = modifiers.control || modifiers.platform;

        let Some(rename) = self.playlist_rename.as_mut() else {
            return;
        };

        match event.keystroke.key.as_str() {
            "enter" => {
                self.commit_playlist_rename();
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                self.cancel_playlist_rename();
                cx.stop_propagation();
                cx.notify();
            }
            "backspace" => {
                rename.input.backspace(command);
                cx.stop_propagation();
                cx.notify();
            }
            "delete" => {
                rename.input.delete(command);
                cx.stop_propagation();
                cx.notify();
            }
            "left" => {
                rename.input.move_left(command, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "right" => {
                rename.input.move_right(command, modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "home" => {
                rename.input.move_home(modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "end" => {
                rename.input.move_end(modifiers.shift);
                cx.stop_propagation();
                cx.notify();
            }
            "space" => {
                if command || modifiers.alt || modifiers.function {
                    return;
                }
                rename.input.insert(" ");
                cx.stop_propagation();
                cx.notify();
            }
            _ => {
                if command && !modifiers.alt && !modifiers.function {
                    match event.keystroke.key.as_str() {
                        "a" => {
                            rename.input.select_all();
                            cx.stop_propagation();
                            cx.notify();
                            return;
                        }
                        "c" => {
                            if let Some(text) = rename.input.selected_text() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                            }
                            cx.stop_propagation();
                            return;
                        }
                        "x" => {
                            if let Some(text) = rename.input.selected_text() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                rename.input.insert("");
                                cx.notify();
                            }
                            cx.stop_propagation();
                            return;
                        }
                        "v" => {
                            if let Some(text) =
                                cx.read_from_clipboard().and_then(|item| item.text())
                            {
                                rename.input.insert(&text.replace('\n', " "));
                                cx.notify();
                            }
                            cx.stop_propagation();
                            return;
                        }
                        _ => {}
                    }
                }

                let Some(key_char) = event.keystroke.key_char.as_ref() else {
                    return;
                };
                if command || modifiers.alt || modifiers.function {
                    return;
                }
                if key_char.chars().all(|ch| !ch.is_control()) {
                    rename.input.insert(key_char);
                    cx.stop_propagation();
                    cx.notify();
                }
            }
        }
    }

    /// Apply the in-progress rename. Empty / whitespace-only names are
    /// rejected (treated as a cancel) so we never end up with an
    /// invisible playlist label.
    fn commit_playlist_rename(&mut self) {
        let Some(rename) = self.playlist_rename.take() else {
            return;
        };
        self.playlist_rename_focus_handle = None;
        let new_name = rename.input.text().trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let Some(playlist) = self.playlists.get_mut(rename.playlist_ix) else {
            return;
        };
        if playlist.name == new_name {
            return;
        }
        playlist.name = new_name;
        self.save_app_state();
    }

    /// Show the delete-confirmation modal for a playlist.
    fn request_delete_playlist(&mut self, playlist_ix: usize) {
        if playlist_ix >= self.playlists.len() {
            return;
        }
        self.playlist_context_menu = None;
        self.cancel_playlist_rename();
        self.playlist_delete_confirm = Some(playlist_ix);
    }

    fn cancel_delete_playlist(&mut self) {
        self.playlist_delete_confirm = None;
    }

    /// Confirm and perform the playlist deletion. Closes any open tabs
    /// referencing this playlist and shifts indices in surviving tabs
    /// + saved state so all `TabSource::Playlist(ix)` references stay
    ///   valid.
    fn confirm_delete_playlist(&mut self) {
        let Some(playlist_ix) = self.playlist_delete_confirm.take() else {
            return;
        };
        if playlist_ix >= self.playlists.len() {
            return;
        }

        self.playlists.remove(playlist_ix);

        // Drop tabs that pointed at the deleted playlist; for tabs
        // pointing at a higher index, shift down by 1 to keep them
        // aligned with the new `playlists` Vec.
        let mut tab_ix = 0;
        while tab_ix < self.tabs.len() {
            match &self.tabs[tab_ix].source {
                TabSource::Playlist(ix) if *ix == playlist_ix => {
                    // Skip the closability check used for normal tab
                    // close: when the underlying playlist is gone,
                    // the tab has nothing to render so it must be
                    // removed even if it's the active tab.
                    self.tabs.remove(tab_ix);
                    if self.active_tab > tab_ix {
                        self.active_tab -= 1;
                    } else if self.active_tab >= self.tabs.len() {
                        self.active_tab = self.tabs.len().saturating_sub(1);
                    }
                    continue;
                }
                TabSource::Playlist(ix) if *ix > playlist_ix => {
                    self.tabs[tab_ix].source = TabSource::Playlist(*ix - 1);
                }
                _ => {}
            }
            tab_ix += 1;
        }

        // Same shift for navigation history entries -- otherwise
        // back/forward could resurrect a stale playlist index.
        for entry in self
            .back_history
            .iter_mut()
            .chain(self.forward_history.iter_mut())
        {
            if let Some(tab) = entry.tab.as_mut() {
                match &tab.source {
                    TabSource::Playlist(ix) if *ix == playlist_ix => {
                        entry.tab = None;
                    }
                    TabSource::Playlist(ix) if *ix > playlist_ix => {
                        tab.source = TabSource::Playlist(*ix - 1);
                    }
                    _ => {}
                }
            }
        }

        // Same shift for the reopen-closed-tab stack. Drop entries
        // that pointed at the deleted playlist and decrement higher
        // indices; otherwise Ctrl+Shift+T could resurrect a stale
        // playlist tab pointing at the wrong (or out-of-bounds) row.
        self.closed_tabs
            .retain_mut(|closed| match &mut closed.source {
                TabSource::Playlist(ix) if *ix == playlist_ix => false,
                TabSource::Playlist(ix) if *ix > playlist_ix => {
                    *ix -= 1;
                    true
                }
                _ => true,
            });

        self.sync_search_input_to_active_tab();
        self.context_menu_track = None;
        self.invalidate_track_indices();
        self.save_app_state();
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
        Self::resolved_page_for_roots(page, &self.library_roots)
    }

    fn resolved_page_for_roots(page: Page, library_roots: &[PathBuf]) -> Page {
        match page {
            Page::Library
            | Page::Artists
            | Page::Albums
            | Page::Genres
            | Page::Liked
            | Page::ScanErrors
                if library_roots.is_empty() =>
            {
                Page::Settings
            }
            page => page,
        }
    }

    fn restore_tabs(saved_tabs: &[SavedBrowseTab]) -> Vec<BrowseTab> {
        let mut tabs = Vec::new();
        for saved in saved_tabs {
            if tabs.iter().any(|tab: &BrowseTab| tab.id == saved.id) {
                continue;
            }

            let mut tab = match &saved.source {
                TabSource::Library => BrowseTab::library(saved.id),
                TabSource::Playlist(playlist_ix) => BrowseTab::playlist(saved.id, *playlist_ix),
                TabSource::Artist(artist_id) => BrowseTab::artist(saved.id, *artist_id),
                TabSource::Album(album_id) => BrowseTab::album(saved.id, *album_id),
                TabSource::Genre(genre_key) => BrowseTab::genre(saved.id, genre_key.clone()),
            };
            tab.search_query = saved.search_query.clone();
            tab.sort_column = saved.sort_column;
            tab.sort_direction = saved.sort_direction;
            tab.selected_track = saved.selected_track;
            tab.table_scroll_top = saved.table_scroll_top.max(0.0);
            tab.restore_table_scroll_top = Some(tab.table_scroll_top);
            tab.table_horizontal_scroll = saved.table_horizontal_scroll.max(0.0);
            tabs.push(tab);
        }

        tabs
    }

    fn set_page_without_history(&mut self, page: Page) {
        self.clear_tooltip();
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
                source: self.active_tab().source.clone(),
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
        let mut tab = match &nav_tab.source {
            TabSource::Library => BrowseTab::library(nav_tab.tab_id),
            TabSource::Playlist(playlist_ix) => BrowseTab::playlist(nav_tab.tab_id, *playlist_ix),
            TabSource::Artist(artist_id) => BrowseTab::artist(nav_tab.tab_id, *artist_id),
            TabSource::Album(album_id) => BrowseTab::album(nav_tab.tab_id, *album_id),
            TabSource::Genre(genre_key) => BrowseTab::genre(nav_tab.tab_id, genre_key.clone()),
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

    fn set_theme(&mut self, theme_id: &str, cx: &mut Context<Self>) {
        if self.themes.iter().any(|theme| theme.id == theme_id) {
            self.theme_id = theme_id.to_string();
            // Mirror the new colors to the player so its independent
            // render path stays in sync.
            let new_colors = *self.colors();
            self.player.update(cx, |player, player_cx| {
                player.set_theme_colors(new_colors, player_cx);
            });
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

        match &tab.source {
            TabSource::Library => "All Music".to_string(),
            TabSource::Playlist(playlist_ix) => self
                .playlists
                .get(*playlist_ix)
                .map(|playlist| playlist.name.clone())
                .unwrap_or_else(|| "Missing Playlist".to_string()),
            TabSource::Artist(artist_id) => self
                .artist_by_id(*artist_id)
                .map(|artist| artist.name.clone())
                .unwrap_or_else(|| "Missing Artist".to_string()),
            TabSource::Album(album_id) => self
                .album_by_id(*album_id)
                .map(|album| album.title.clone())
                .unwrap_or_else(|| "Missing Album".to_string()),
            TabSource::Genre(genre_key) => self
                .genre_by_key(genre_key)
                .map(|genre| genre.name.clone())
                .unwrap_or_else(|| "Missing Genre".to_string()),
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
        if let Some(tab_ix) = self.tabs.iter().position(|tab| {
            matches!(&tab.source, TabSource::Library) && tab.search_query.trim().is_empty()
        }) {
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
            .position(|tab| matches!(&tab.source, TabSource::Playlist(ix) if *ix == playlist_ix))
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
            .position(|tab| matches!(&tab.source, TabSource::Artist(id) if *id == artist_id))
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
        self.queue_artist_metadata_demand(artist_id);
    }

    fn open_album_tab(&mut self, album_id: i64) {
        let previous = self.current_navigation_entry();
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| matches!(&tab.source, TabSource::Album(id) if *id == album_id))
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
        self.queue_album_cover_demand(album_id);
    }

    fn open_genre_tab(&mut self, genre_key: String) {
        if !self.genres.iter().any(|genre| genre.key == genre_key) {
            return;
        }

        let previous = self.current_navigation_entry();
        if let Some(tab_ix) = self
            .tabs
            .iter()
            .position(|tab| matches!(&tab.source, TabSource::Genre(key) if key == &genre_key))
        {
            self.active_tab = tab_ix;
        } else {
            let tab_id = self.allocate_tab_id();
            self.tabs.push(BrowseTab::genre(tab_id, genre_key.clone()));
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

    /// Nudge the tab bar's horizontal scroll by `delta` pixels (positive
    /// scrolls right, negative scrolls left). Returns `true` if the
    /// offset actually moved -- used by the arrow buttons to decide
    /// whether to call `cx.notify()`.
    pub(super) fn scroll_tab_bar_by(&mut self, delta: f32) -> bool {
        let max_scroll = f32::from(self.tab_bar_scroll_handle.max_offset().width).max(0.0);
        if max_scroll <= 0.0 {
            return false;
        }

        // The scroll handle stores offset.x as a non-positive value:
        // 0 == fully scrolled left, -max == fully scrolled right.
        // Convert to a positive "scrolled distance", clamp, then
        // store back as the negated value the handle expects.
        let current = (-f32::from(self.tab_bar_scroll_handle.offset().x)).clamp(0.0, max_scroll);
        let next = (current + delta).clamp(0.0, max_scroll);
        if (next - current).abs() < 0.5 {
            return false;
        }
        self.tab_bar_scroll_handle
            .set_offset(point(px(-next), self.tab_bar_scroll_handle.offset().y));
        true
    }

    /// Ensure the active tab is visible in the horizontally-scrollable
    /// tab strip. Called once per render from `Render::render` when
    /// `active_tab` changes (Ctrl+Tab, sidebar click, drag-open,
    /// reopen-closed-tab, etc.). The actual scroll math lives in
    /// gpui's `ScrollHandle::scroll_to_active_item`, applied during
    /// the next prepaint of the tracked scroll wrapper.
    pub(super) fn auto_scroll_active_tab_into_view(&mut self) {
        if self.active_tab == self.last_scrolled_active_tab {
            return;
        }
        self.last_scrolled_active_tab = self.active_tab;
        self.tab_bar_scroll_handle.scroll_to_item(self.active_tab);
    }

    fn artist_by_id(&self, artist_id: i64) -> Option<&Artist> {
        self.artists
            .iter()
            .find(|artist| artist.artist_id == artist_id)
    }

    fn album_by_id(&self, album_id: i64) -> Option<&Album> {
        self.albums.iter().find(|album| album.album_id == album_id)
    }

    fn genre_by_key(&self, genre_key: &str) -> Option<&Genre> {
        self.genres.iter().find(|genre| genre.key == genre_key)
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

        let removed = self.tabs.remove(tab_ix);
        self.push_closed_tab(ClosedTab {
            source: removed.source,
            search_query: removed.search_query,
        });
        if self.active_tab > tab_ix {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.sync_search_input_to_active_tab();
        self.context_menu_track = None;
    }

    fn close_all_tabs(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }

        // Drain in left-to-right order so the rightmost tab ends up
        // on top of the reopen stack -- matches the order the user
        // would have closed them one-by-one and is what
        // browser-style Ctrl+Shift+T users expect.
        for closed in self.tabs.drain(1..) {
            self.closed_tabs.push(ClosedTab {
                source: closed.source,
                search_query: closed.search_query,
            });
        }
        if self.closed_tabs.len() > CLOSED_TABS_MAX {
            let overflow = self.closed_tabs.len() - CLOSED_TABS_MAX;
            self.closed_tabs.drain(..overflow);
        }
        self.active_tab = 0;
        self.sync_search_input_to_active_tab();
        self.context_menu_track = None;
    }

    /// Record a closed tab on the reopen stack, evicting the oldest
    /// entry if we're already at the cap.
    fn push_closed_tab(&mut self, closed: ClosedTab) {
        if self.closed_tabs.len() >= CLOSED_TABS_MAX {
            self.closed_tabs.remove(0);
        }
        self.closed_tabs.push(closed);
    }

    /// Pop the most recently closed tab off the reopen stack and
    /// reinstate it as a new tab. No-ops when the stack is empty.
    /// Stale entries (e.g. a playlist that was deleted while the tab
    /// was closed) are skipped over so the user keeps walking back
    /// through history rather than seeing the action silently fail.
    fn reopen_closed_tab(&mut self) {
        while let Some(closed) = self.closed_tabs.pop() {
            if self.try_open_closed_tab(closed) {
                return;
            }
        }
    }

    /// Returns `true` if `closed` was successfully reinstated as a
    /// tab. Returns `false` when the underlying entity no longer
    /// exists (e.g. a deleted playlist or a genre no longer in the
    /// catalog), signalling the caller should try the next entry.
    fn try_open_closed_tab(&mut self, closed: ClosedTab) -> bool {
        match closed.source {
            TabSource::Library => {
                if closed.search_query.trim().is_empty() {
                    self.new_library_tab();
                } else {
                    self.new_search_tab(closed.search_query);
                }
                true
            }
            TabSource::Playlist(ix) => {
                if ix >= self.playlists.len() {
                    return false;
                }
                self.open_playlist_tab(ix);
                true
            }
            TabSource::Artist(id) => {
                self.open_artist_tab(id);
                true
            }
            TabSource::Album(id) => {
                self.open_album_tab(id);
                true
            }
            TabSource::Genre(key) => {
                if !self.genres.iter().any(|genre| genre.key == key) {
                    return false;
                }
                self.open_genre_tab(key);
                true
            }
        }
    }

    fn can_close_tab(&self, tab_ix: usize) -> bool {
        // The very first tab is the permanent "All Music" anchor and is
        // never closable. Every other tab (including additional All Music
        // tabs the user has opened later) can be closed, regardless of
        // source or search state. This is intentionally simpler than the
        // older rule that exempted any empty-search Library tab.
        tab_ix > 0 && tab_ix < self.tabs.len()
    }
}

/// Build the pre-lowercased searchable blob for a `Track`. Match
/// arguments mirror the columns the UI search bar previously
/// `format!()`-ed on every keystroke.
fn track_searchable_lower(
    title: &str,
    artist: &str,
    album: &str,
    genre: &str,
    year: &str,
    codec: &str,
    path: &Path,
) -> String {
    format!(
        "{} {} {} {} {} {} {}",
        title,
        artist,
        album,
        genre,
        year,
        codec,
        path.display()
    )
    .to_lowercase()
}

fn artist_searchable_lower(
    name: &str,
    bio: Option<&str>,
    album_count: usize,
    track_count: usize,
) -> String {
    format!(
        "{} {} {} {}",
        name,
        bio.unwrap_or_default(),
        album_count,
        track_count
    )
    .to_lowercase()
}

fn album_searchable_lower(
    title: &str,
    artist: &str,
    year: Option<&str>,
    track_count: usize,
) -> String {
    format!(
        "{} {} {} {}",
        title,
        artist,
        year.unwrap_or_default(),
        track_count
    )
    .to_lowercase()
}

impl From<tempo::library::Track> for Track {
    fn from(track: tempo::library::Track) -> Self {
        let album_initials = artwork::album_initials_for(&track.album, &track.title);
        let album_color = artwork::album_color_for(&track.album, &track.artist);
        let genre = track.genre.unwrap_or_else(|| "Unknown genre".to_string());
        let year = track.year.unwrap_or_else(|| "Unknown year".to_string());
        let searchable_lower = track_searchable_lower(
            &track.title,
            &track.artist,
            &track.album,
            &genre,
            &year,
            &track.codec,
            &track.path,
        );

        Self {
            artist_id: None,
            album_id: None,
            path: track.path,
            title: SharedString::from(track.title),
            artist: SharedString::from(track.artist),
            album: SharedString::from(track.album),
            genre: SharedString::from(genre),
            track_number: track.track_number,
            year: SharedString::from(year),
            date_added: track.date_added,
            duration: SharedString::from(format_duration(track.duration)),
            duration_value: track.duration,
            codec: SharedString::from(track.codec),
            bitrate: track.bitrate,
            file_size: track.file_size,
            plays: 0,
            liked: false,
            artwork: track.artwork.and_then(TrackArtwork::from_library),
            album_initials,
            album_color,
            searchable_lower,
        }
    }
}

impl From<CatalogTrack> for Track {
    fn from(track: CatalogTrack) -> Self {
        let album_initials = artwork::album_initials_for(&track.album, &track.title);
        let album_color = artwork::album_color_for(&track.album, &track.artist);
        let genre = track.genre.unwrap_or_else(|| "Unknown genre".to_string());
        let year = track.year.unwrap_or_else(|| "Unknown year".to_string());
        let searchable_lower = track_searchable_lower(
            &track.title,
            &track.artist,
            &track.album,
            &genre,
            &year,
            &track.codec,
            &track.path,
        );

        Self {
            artist_id: Some(track.artist_id),
            album_id: Some(track.album_id),
            path: track.path,
            title: SharedString::from(track.title),
            artist: SharedString::from(track.artist),
            album: SharedString::from(track.album),
            genre: SharedString::from(genre),
            track_number: track.track_number,
            year: SharedString::from(year),
            date_added: track.date_added,
            duration: SharedString::from(format_duration(track.duration)),
            duration_value: track.duration,
            codec: SharedString::from(track.codec),
            bitrate: track.bitrate,
            file_size: track.file_size,
            plays: track.play_count,
            liked: track.liked,
            artwork: track.artwork_path.map(TrackArtwork::File),
            album_initials,
            album_color,
            searchable_lower,
        }
    }
}

impl From<CatalogArtist> for Artist {
    fn from(artist: CatalogArtist) -> Self {
        let searchable_lower = artist_searchable_lower(
            &artist.name,
            artist.bio.as_deref(),
            artist.album_count,
            artist.track_count,
        );
        Self {
            artist_id: artist.artist_id,
            initials: artwork::initials_for(&artist.name),
            color: artwork::color_for(&artist.name, "artist"),
            name: artist.name,
            bio: artist.bio,
            photo_path: artist.photo_path,
            album_count: artist.album_count,
            track_count: artist.track_count,
            searchable_lower,
        }
    }
}

impl From<CatalogAlbum> for Album {
    fn from(album: CatalogAlbum) -> Self {
        let searchable_lower = album_searchable_lower(
            &album.title,
            &album.artist,
            album.year.as_deref(),
            album.track_count,
        );
        Self {
            album_id: album.album_id,
            artist_id: album.artist_id,
            initials: artwork::initials_for(&album.title),
            color: artwork::album_color_for(&album.title, &album.artist),
            title: album.title,
            artist: album.artist,
            year: album.year,
            artwork_path: album.artwork_path,
            track_count: album.track_count,
            searchable_lower,
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

fn format_duration_compact(duration: Duration) -> String {
    let total_minutes = duration.as_secs() / 60;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Open the OS file manager focused on `path`, ideally with `path`
/// selected. Falls back to opening the parent directory if a
/// "select-and-reveal" call isn't available.
///
/// Runs the actual platform calls on a short-lived background thread
/// so the UI thread is never blocked on file-manager startup or
/// dbus-send round-trips.
pub(super) fn reveal_in_file_manager(path: &Path) {
    // The track path may have been removed/renamed on disk between
    // catalog refresh and the user clicking the menu item; treat that
    // as a no-op rather than spawning a process for a nonexistent
    // target.
    if !path.exists() {
        perf::event(
            "reveal_in_file_manager.skip_missing",
            format!("path={}", path.display()),
        );
        return;
    }

    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("tempo-reveal-in-file-manager".into())
        .spawn(move || reveal_in_file_manager_blocking(&path))
        .ok();
}

#[cfg(target_os = "macos")]
fn reveal_in_file_manager_blocking(path: &Path) {
    // `open -R <path>` reveals + selects the file in Finder.
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status();
    perf::event(
        "reveal_in_file_manager.open_dash_r",
        format!("path={}", path.display()),
    );
}

#[cfg(target_os = "windows")]
fn reveal_in_file_manager_blocking(path: &Path) {
    // `explorer /select,<path>` opens Explorer with the file
    // highlighted. The comma is part of the option syntax and the
    // whole thing must be a single argument.
    let mut arg = std::ffi::OsString::from("/select,");
    arg.push(path.as_os_str());
    let _ = std::process::Command::new("explorer").arg(arg).status();
    perf::event(
        "reveal_in_file_manager.explorer_select",
        format!("path={}", path.display()),
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_in_file_manager_blocking(path: &Path) {
    // Try the freedesktop FileManager1 D-Bus interface first --
    // implemented by Nautilus, Nemo, Caja, Dolphin, PCManFM, and
    // Thunar (recent versions). When present it selects the file
    // in a file-manager window instead of just opening the parent
    // directory.
    //
    // Method signature: `ShowItems(as URIs, s startup_id)`. We pass
    // a single file:// URI and an empty startup id.
    let uri = format!("file://{}", path.display());
    let dbus_status = std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.FileManager1",
            "--type=method_call",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .status();

    if matches!(dbus_status, Ok(status) if status.success()) {
        perf::event(
            "reveal_in_file_manager.dbus_show_items",
            format!("path={}", path.display()),
        );
        return;
    }

    // Fallback: open the parent directory with `xdg-open`. This
    // doesn't select the file, but at least gets the user to the
    // right folder on systems without a FileManager1 implementation
    // (e.g. minimal WMs, custom file managers).
    let parent = path.parent().unwrap_or(path);
    let _ = std::process::Command::new("xdg-open").arg(parent).status();
    perf::event(
        "reveal_in_file_manager.xdg_open_parent",
        format!("parent={}", parent.display()),
    );
}

impl Render for TempoApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Per-frame top-level span. During scrolling this fires every frame
        // (~16 ms target at 60Hz). Wide gaps between frames or unusually
        // long render durations are the usual smoking gun for jank.
        let _frame_span = perf::span(
            "render.frame",
            format!(
                "page={} tab_kind={} scrolling={}",
                Self::page_label(self.page),
                Self::tab_kind_label(self.tabs.get(self.active_tab)),
                self.table_is_scrolling
            ),
        );
        // Keep the active tab visible in the horizontal tab strip.
        // No-op when active_tab hasn't changed since the last frame.
        self.auto_scroll_active_tab_into_view();
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
            .on_action(cx.listener(Self::close_all_tabs_action))
            .on_action(cx.listener(Self::reopen_closed_tab_action))
            .on_action(cx.listener(Self::next_tab_action))
            .on_action(cx.listener(Self::previous_tab_action))
            .on_action(cx.listener(Self::select_tab_1_action))
            .on_action(cx.listener(Self::select_tab_2_action))
            .on_action(cx.listener(Self::select_tab_3_action))
            .on_action(cx.listener(Self::select_tab_4_action))
            .on_action(cx.listener(Self::select_tab_5_action))
            .on_action(cx.listener(Self::select_tab_6_action))
            .on_action(cx.listener(Self::select_tab_7_action))
            .on_action(cx.listener(Self::select_tab_8_action))
            .on_action(cx.listener(Self::select_tab_9_action))
            .on_action(cx.listener(Self::select_tab_10_action))
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag_volume(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_volume_drag(cx) {
                        cx.stop_propagation();
                    }
                }),
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
            // Player bar: when the library is empty there's no
            // currently-playing track snapshot to render with, so we
            // fall back to an inline placeholder that knows about
            // scan state. Otherwise we embed the `PlayerEntity` as a
            // child element — its `cx.notify()` calls only invalidate
            // the player's own subtree, which is what unlocks the
            // localized-invalidation perf win.
            .child(if self.tracks.is_empty() {
                self.render_empty_player_bar(cx)
            } else {
                let _ = window;
                self.player.clone().into_any_element()
            })
            .when_some(
                self.context_menu_track
                    .filter(|track_ix| *track_ix < self.tracks.len()),
                |this, track_ix| this.child(self.render_context_menu(track_ix, cx)),
            )
            .when(self.playlist_context_menu.is_some(), |this| {
                this.child(self.render_playlist_context_menu(cx))
            })
            .when(self.playlist_delete_confirm.is_some(), |this| {
                this.child(self.render_playlist_delete_confirm(cx))
            })
            .when(self.column_menu_open, |this| {
                this.child(self.render_column_menu(cx))
            })
            .when(self.player.read(cx).settings_output_menu_open(), |this| {
                this.child(self.settings_output_device_menu(cx))
            })
            .when_some(self.tooltip.clone(), |this, tooltip| {
                this.child(self.render_tooltip(&tooltip))
            })
    }
}

impl TempoApp {
    fn page_label(page: Page) -> &'static str {
        match page {
            Page::Library => "library",
            Page::Artists => "artists",
            Page::Albums => "albums",
            Page::Genres => "genres",
            Page::Liked => "liked",
            Page::PlaybackHistory => "playback_history",
            Page::ScanErrors => "scan_errors",
            Page::Settings => "settings",
        }
    }

    fn tab_kind_label(tab: Option<&BrowseTab>) -> &'static str {
        match tab.map(|tab| &tab.source) {
            Some(TabSource::Library) => "library",
            Some(TabSource::Playlist(_)) => "playlist",
            Some(TabSource::Artist(_)) => "artist",
            Some(TabSource::Album(_)) => "album",
            Some(TabSource::Genre(_)) => "genre",
            None => "none",
        }
    }

    fn play_selected(&mut self, _: &PlaySelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_focus_handle.is_focused(window) {
            return;
        }

        if self.tracks.is_empty() {
            return;
        }

        self.play_track(self.active_selected_track(), cx);
        cx.notify();
    }

    fn toggle_pause(&mut self, _: &TogglePause, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_focus_handle.is_focused(window) {
            return;
        }

        if self.tracks.is_empty() {
            return;
        }

        self.toggle_playback(cx);
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

    fn close_all_tabs_action(&mut self, _: &CloseAllTabs, _: &mut Window, cx: &mut Context<Self>) {
        self.close_all_tabs();
        cx.notify();
    }

    fn reopen_closed_tab_action(
        &mut self,
        _: &ReopenClosedTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reopen_closed_tab();
        cx.notify();
    }

    fn next_tab_action(&mut self, _: &NextTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.tab_offset_by(1) {
            self.select_tab(target);
            cx.notify();
        }
    }

    fn previous_tab_action(&mut self, _: &PreviousTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.tab_offset_by(-1) {
            self.select_tab(target);
            cx.notify();
        }
    }

    fn select_tab_by_position(&mut self, tab_ix: usize, cx: &mut Context<Self>) {
        if tab_ix < self.tabs.len() && tab_ix != self.active_tab {
            self.select_tab(tab_ix);
            cx.notify();
        }
    }

    fn select_tab_1_action(&mut self, _: &SelectTab1, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(0, cx);
    }

    fn select_tab_2_action(&mut self, _: &SelectTab2, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(1, cx);
    }

    fn select_tab_3_action(&mut self, _: &SelectTab3, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(2, cx);
    }

    fn select_tab_4_action(&mut self, _: &SelectTab4, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(3, cx);
    }

    fn select_tab_5_action(&mut self, _: &SelectTab5, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(4, cx);
    }

    fn select_tab_6_action(&mut self, _: &SelectTab6, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(5, cx);
    }

    fn select_tab_7_action(&mut self, _: &SelectTab7, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(6, cx);
    }

    fn select_tab_8_action(&mut self, _: &SelectTab8, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(7, cx);
    }

    fn select_tab_9_action(&mut self, _: &SelectTab9, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(8, cx);
    }

    fn select_tab_10_action(&mut self, _: &SelectTab10, _: &mut Window, cx: &mut Context<Self>) {
        self.select_tab_by_position(9, cx);
    }

    /// Resolve the next/previous tab index with wrap-around. Returns
    /// `None` if there are zero or one tabs (no movement is possible).
    /// `delta` is signed so the same helper covers both directions; we
    /// rely on the rust modulo trick `(a + n) % n` to keep the result
    /// non-negative without depending on the sign of `%` for negatives.
    fn tab_offset_by(&self, delta: isize) -> Option<usize> {
        let count = self.tabs.len();
        if count <= 1 {
            return None;
        }
        let count_i = count as isize;
        let current = self.active_tab as isize;
        let target = ((current + delta) % count_i + count_i) % count_i;
        Some(target as usize)
    }

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(
            self.page,
            Page::Library | Page::Artists | Page::Albums | Page::Genres
        ) {
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
        self.play_random_track(cx);
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
