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
    Animation, AnimationExt as _, AnyElement, Bounds, ClickEvent, ClipboardItem, Context, Corner,
    CursorStyle, Entity, FocusHandle, Image, ImageFormat, IntoElement, KeyDownEvent,
    ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    NavigationDirection, ObjectFit, ParentElement, PathPromptOptions, Pixels, Point, Render,
    ScrollStrategy, ScrollWheelEvent, SharedString, Size, Styled, Subscription,
    UniformListScrollHandle, Window, WindowBounds, WindowHandle, anchored, div, img, point,
    prelude::*, px, relative, rgb, size, uniform_list,
};
use rodio::{Decoder, Source as _};
use serde::{Deserialize, Serialize};
use tempo::{
    catalog::{
        AlbumDescriptionState, CatalogAlbum, CatalogArtist, CatalogMetadataActivity,
        CatalogMetadataError, CatalogStore, CatalogTrack, individual_artist_names,
        primary_artist_name,
    },
    library::{
        Artwork as LibraryArtwork, IndexingError, LibraryEvent, LibraryIndexer, LibraryWatcher,
        ScanProgress,
    },
    metadata_worker::{MetadataEvent, MetadataWorker},
    perf,
    playback::PlaybackController,
};

mod analytics;
mod artwork;
mod browse_grids;
mod charts;
mod equalizer_panel;
mod history;
mod hotkeys_panel;
mod library_state;
mod lifecycle;
mod mpris_art;
#[cfg(all(unix, not(target_os = "macos")))]
mod tray_panel;
use mpris_art::art_url_for_track;
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
    CloseAllTabs, CloseTab, CycleMiniPlayer, FocusSearch, HideWindow, MoveSelectionDown,
    MoveSelectionUp, NavigateBack, NavigateForward, NewTab, NextTab, OpenSettings, PlayRandomTrack,
    PlaySelected, PreviousTab, ReopenClosedTab, SelectTab1, SelectTab2, SelectTab3, SelectTab4,
    SelectTab5, SelectTab6, SelectTab7, SelectTab8, SelectTab9, SelectTab10, ToggleMiniPlayer,
    TogglePause,
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
    /// Unified error page combining scan errors with online-metadata
    /// failures. The serde alias keeps state files written by older
    /// builds (when the variant was `ScanErrors`) loadable; the alias
    /// is read-only -- new writes use `Errors`.
    #[serde(alias = "ScanErrors")]
    Errors,
    Analytics,
    Settings,
}

/// Stable category buckets surfaced as toggleable filter pills on the
/// Errors page. Scan failures land in `Scan`; metadata-job failures map
/// from `metadata_jobs.error_kind` (or fall through to `Network` when
/// the kind is missing). Order here matches the visual order of pills
/// in the badge bar.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub(super) enum ErrorCategory {
    Scan,
    Network,
    Http4xx,
    Http5xx,
    Parse,
    NoMatch,
}

impl ErrorCategory {
    /// Render label used both on the badge pills and inside the
    /// table's TYPE column.
    pub fn label(self) -> &'static str {
        match self {
            Self::Scan => "Scan",
            Self::Network => "Network",
            Self::Http4xx => "HTTP 4xx",
            Self::Http5xx => "HTTP 5xx",
            Self::Parse => "Parse",
            Self::NoMatch => "No match",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::Scan,
        Self::Network,
        Self::Http4xx,
        Self::Http5xx,
        Self::Parse,
        Self::NoMatch,
    ];

    /// Map a persisted `metadata_jobs.error_kind` string to a category.
    /// Returns `None` for unknown classifiers; callers default to
    /// `Network` so unclassified failures still show up under a sensible
    /// pill.
    pub fn from_error_kind(kind: &str) -> Option<Self> {
        match kind {
            "network" => Some(Self::Network),
            "http_4xx" => Some(Self::Http4xx),
            "http_5xx" => Some(Self::Http5xx),
            "parse" => Some(Self::Parse),
            "no_match" => Some(Self::NoMatch),
            _ => None,
        }
    }
}

/// Per-row view computed by the Errors page renderer. Whether the row
/// is a scan error or a metadata-job failure, we collapse it into a
/// uniform shape so the table renderer doesn't need to branch on
/// origin.
#[derive(Clone)]
pub(super) struct ErrorRowView {
    pub category: ErrorCategory,
    /// Display string for the PATH column. Scan rows put the file
    /// path; metadata rows put `<entity_type>: <name>` style labels.
    pub path_label: String,
    pub message: String,
}

/// Sub-section of the Settings page. The Settings page renders as a
/// two-pane layout: a left nav listing these sections and a right
/// detail pane that shows only the active section. Pagination-style
/// (clicking swaps the visible section), not scroll-spy.
///
/// Runtime-only state — deliberately not persisted to `state.json`.
/// Defaults to `Library` when the library has no roots configured (so
/// the onboarding card lands where the user needs to take action),
/// otherwise `Appearance`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum SettingsSection {
    Appearance,
    AudioOutput,
    Window,
    Library,
    OnlineMetadata,
    Hotkeys,
}

impl SettingsSection {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::AudioOutput => "Audio Output",
            Self::Window => "Window",
            Self::Library => "Library",
            Self::OnlineMetadata => "Online Metadata",
            Self::Hotkeys => "Hotkeys",
        }
    }

    pub(super) fn all() -> [Self; 6] {
        [
            Self::Appearance,
            Self::AudioOutput,
            Self::Window,
            Self::Library,
            Self::OnlineMetadata,
            Self::Hotkeys,
        ]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum BrowseViewMode {
    Grid,
    Table,
}

/// Time-range filter for the Analytics page. `All` is the default
/// (matches the dashboard's pre-filter behavior); the rest constrain
/// the playback-history aggregations to a rolling window. Library
/// stats (genre, codec, decades, etc.) are unaffected by the filter
/// because they describe the whole indexed library.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
enum AnalyticsTimeRange {
    SevenDays,
    ThirtyDays,
    #[default]
    NinetyDays,
    OneYear,
    All,
}

impl AnalyticsTimeRange {
    fn label(self) -> &'static str {
        match self {
            Self::SevenDays => "7D",
            Self::ThirtyDays => "30D",
            Self::NinetyDays => "90D",
            Self::OneYear => "1Y",
            Self::All => "ALL",
        }
    }

    fn long_label(self) -> &'static str {
        match self {
            Self::SevenDays => "Last 7 days",
            Self::ThirtyDays => "Last 30 days",
            Self::NinetyDays => "Last 90 days",
            Self::OneYear => "Last 12 months",
            Self::All => "All time",
        }
    }

    /// Window length in days, or `None` for "all time" (no cutoff).
    fn window_days(self) -> Option<u32> {
        match self {
            Self::SevenDays => Some(7),
            Self::ThirtyDays => Some(30),
            Self::NinetyDays => Some(90),
            Self::OneYear => Some(365),
            Self::All => None,
        }
    }

    fn all() -> [Self; 5] {
        [
            Self::SevenDays,
            Self::ThirtyDays,
            Self::NinetyDays,
            Self::OneYear,
            Self::All,
        ]
    }
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

/// Top-level window layout mode.
///
/// `Full` is the default many-pixel UI with sidebar, content pages, and
/// the player bar at the bottom. `Mini(MiniSize)` shrinks the window
/// down to a compact "now playing" surface — either a horizontal bar or
/// one of two squares — with hover-revealed transport / volume / seek
/// controls.
///
/// Mini mode is intentionally *runtime-only*: it is not serialized into
/// `state.json`, so launching Tempo always starts in `Full`. The user
/// re-enters mini mode via the 2D-glyph button in the bottom-right of
/// the now-playing info column or via `Ctrl+M`.
///
/// TODO(always-on-top): GPUI 0.2.2 has no runtime API to mark a window
/// as always-on-top. The mini player would benefit from "stay above
/// other windows" semantics; revisit when GPUI exposes a setter, or
/// when a platform shim (X11 `_NET_WM_STATE_ABOVE` / Wayland
/// layer-shell) is added.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WindowMode {
    Full,
    Mini(MiniSize),
}

/// Two discrete mini-player layouts, toggled by the size-cycle icon
/// inside the mini overlay (`CompactBar ↔ Square`).
///
/// `CompactBar` has a fixed shape (360x100) — there's nothing useful
/// to drag-resize on a horizontal strip. `Square` always opens at the
/// default 400x400; the user can drag-resize it freely while it's open
/// and the album art fills whatever rectangle the window currently
/// has, minus the bottom metadata strip. The size is *not* remembered
/// across cycles — re-entering `Square` always reopens at 400x400.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MiniSize {
    /// 360x100 horizontal bar with thumbnail on the left and
    /// title/artist/album stacked on the right.
    CompactBar,
    /// 400x400 default; user-resizable while open. Album art fills
    /// the available area above a thin metadata strip; controls
    /// overlay on hover.
    Square,
}

impl MiniSize {
    /// Pixel size used when (re)opening the GPUI window for this
    /// variant. Always returns the same value for a given variant —
    /// `Square` is intentionally not "sticky" across cycles, so each
    /// activation reopens at the default and the user is free to
    /// resize from there.
    pub(crate) fn default_window_size(self) -> Size<Pixels> {
        let (w, h) = match self {
            Self::CompactBar => (360.0, 100.0),
            Self::Square => (400.0, 400.0),
        };
        size(px(w), px(h))
    }

    /// Next size in the cycle order.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::CompactBar => Self::Square,
            Self::Square => Self::CompactBar,
        }
    }
}

/// Deferred window-swap request — used only when *transitioning
/// between* full and mini mode (full→mini and mini→full).
///
/// The render path consumes this on the next paint, opens a new window
/// with the resolved bounds, mounts the existing `Entity<TempoApp>` as
/// its root, and closes the old window. We have to swap-and-replace
/// rather than `window.resize()` for these transitions because:
///
/// - Hyprland (and several other Wayland compositors) ignore most
///   client-driven resize requests for tiled windows.
/// - The full→mini size delta is huge (1280×820 → 360×100), and a
///   full→mini in-place resize on Hyprland leaves the window tiled
///   into its old slot and the compositor re-runs its placement
///   rules anyway.
///
/// **Mini-mode size cycling** (CompactBar ↔ Square ↔ LargeSquare)
/// uses [`PendingWindowResize`] instead — once the window is already
/// floating, `window.resize()` is honored, so we keep the same window
/// (and the user's manual move/anchor of the floating window).
#[derive(Clone, Copy)]
pub(crate) struct PendingWindowSwap {
    /// Target content size for the new window.
    pub(crate) target_size: Size<Pixels>,
    /// If `Some`, the new window opens at exactly these bounds
    /// (used when leaving mini mode to restore the pre-mini
    /// position). If `None`, the render path keeps the new window
    /// centered on the current window's center so the swap feels
    /// "in place".
    pub(crate) explicit_bounds: Option<Bounds<Pixels>>,
}

/// Which visualizer renders inside the seekbar surface.
///
/// `Waveform` is the precomputed-peaks bar chart that has always lived
/// in the seekbar. The other variants render frequency-reactive views
/// driven by [`crate::audio_analyzer::AudioAnalyzer`]. The user picks
/// one from the ✦ menu on the seekbar; the choice is persisted in
/// `state.json` so it survives restarts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub(crate) enum VisualizerKind {
    /// Original cached-peaks waveform with a progressive playhead.
    /// Default for new users; the only visualizer that doesn't need a
    /// live audio tap.
    #[default]
    Waveform,
    /// Curve drawn through 32 points spaced along log frequency, each
    /// point's height tied to that band's magnitude. Reads as an
    /// oscilloscope-style "spectrum line" that dances with the audio.
    DancingLine,
    /// Vertical bars across log-spaced frequency bands. Classic
    /// spectrum-analyzer look.
    FrequencyBars,
}

impl VisualizerKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Waveform => "Waveform",
            Self::DancingLine => "Dancing Line",
            Self::FrequencyBars => "Frequency Bars",
        }
    }

    pub(crate) const ALL: [Self; 3] = [Self::Waveform, Self::DancingLine, Self::FrequencyBars];
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum WindowRestoreMode {
    BringHere,
    GoToWindow,
}

impl WindowRestoreMode {
    fn label(self) -> &'static str {
        match self {
            Self::BringHere => "Bring window here",
            Self::GoToWindow => "Go to window",
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
struct ErrorColumnWidths {
    index: f32,
    kind: f32,
    path: f32,
    error: f32,
}

impl Default for ErrorColumnWidths {
    fn default() -> Self {
        Self {
            index: 52.0,
            kind: 110.0,
            path: 380.0,
            error: 380.0,
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
enum ErrorColumn {
    Index,
    /// Category badge column (Scan / Network / Http4xx / Http5xx /
    /// Parse / NoMatch).
    Kind,
    Path,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnResizeTarget {
    Track(TableColumn),
    Artist(ArtistTableColumn),
    Album(AlbumTableColumn),
    Genre(GenreTableColumn),
    Error(ErrorColumn),
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
    /// Online-sourced album description (TheAudioDB / Wikipedia / etc.).
    /// `None` when not yet enriched or every fallback source produced
    /// nothing; `description_state` distinguishes those two cases for
    /// the UI.
    description: Option<String>,
    /// Tri-state used by the album-detail hero to decide between the
    /// real description, the synthetic local fallback, and a final
    /// "no online description available" copy.
    description_state: AlbumDescriptionState,
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
    /// Album IDs we've already demanded a description for in this
    /// session. Tracked separately from `albums` (which gates cover
    /// fetches) so two independent UI demands for the same album --
    /// one for a missing cover, one for a missing description -- both
    /// fire exactly once.
    album_descriptions: HashSet<i64>,
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

/// Drag payload used when a row inside the Up Next sidebar is being
/// dragged. Distinct from `TrackDrag` so a drop into the queue can
/// tell whether to *insert* a new entry (`TrackDrag` from the main
/// table / browse views) or *move* an existing one (`QueueRowDrag`
/// originating in the queue itself).
#[derive(Clone)]
struct QueueRowDrag {
    queue_position: usize,
    /// Track index carried for completeness so future cross-list
    /// drop targets (e.g. dragging a queue entry onto a playlist tab)
    /// can resolve the underlying track without re-looking up via
    /// `queue_position`. Currently only the queue itself is a drop
    /// target so the field is read indirectly via `Render` only.
    #[allow(dead_code)]
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

impl QueueRowDrag {
    fn new(queue_position: usize, track_ix: usize, track: &Track) -> Self {
        Self {
            queue_position,
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

impl Render for QueueRowDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Visually identical to `TrackDrag` so an in-list reorder feels
        // consistent with adding a new track from the main table.
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

/// Right-click menu state for an Up Next queue row in the right
/// sidebar. `queue_position` is the index into `self.queue`; the menu
/// resolves to a track via `self.queue[queue_position]` at action time
/// (with bounds-checking, since concurrent mutations could shrink the
/// queue between open and click).
#[derive(Clone, Copy)]
struct QueueContextMenu {
    queue_position: usize,
    position: Point<Pixels>,
}

/// Right-click menu state for a History row in the right sidebar.
/// `history_index` indexes into `self.playback_history` (the
/// underlying append-order vector, *not* the sorted display list) so
/// a "Remove from history" action is stable even after re-sorts.
#[derive(Clone, Copy)]
struct HistoryContextMenu {
    history_index: usize,
    position: Point<Pixels>,
}

/// Which view the right (Up Next) sidebar is currently displaying.
/// The header's "Up Next ▾" label opens a small dropdown that flips
/// this enum; the body branches on it. Persisted in `state.json`.
///
/// `Playlist(usize)` shows a single playlist's tracks; the index is
/// resolved against `self.playlists` at render time. Out-of-range
/// indices fall back to the queue view (so a deleted playlist
/// doesn't strand the sidebar).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
enum RightSidebarView {
    #[default]
    Queue,
    History,
    Playlist(usize),
}

/// Inline-rename state for a sidebar playlist nav item. Holds the
/// editing buffer; the focus handle lives on `TempoApp` so it survives
/// across re-renders.
struct PlaylistRename {
    playlist_ix: usize,
    input: TextInputState,
}

/// Mid-drag state for an equalizer slider. While `Some` on
/// `TempoApp::eq_slider_drag`, mouse-move events update the band's
/// gain based on vertical pointer travel from `start_y`.
#[derive(Clone, Copy)]
struct EqSliderDrag {
    band: usize,
    /// Y coordinate (in window space) where the drag started.
    start_y: f32,
    /// Gain in dB at the moment the drag started.
    start_gain_db: f32,
    /// Pixel height of the slider track at the moment the drag
    /// started. Used to convert vertical travel into a dB delta.
    track_height_px: f32,
}

/// Inline "Save as new" state. While `Some`, the panel shows a text
/// input bound to `input` instead of the standard footer buttons.
struct EqProfileSaveAs {
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

/// Transient bookkeeping for a play that has *started* but hasn't yet
/// crossed the [`crate::app::player::entity::PLAY_THRESHOLD_SECS`]
/// listening threshold. Stored on `TempoApp` until the player entity
/// emits [`crate::app::player::entity::PlayerEvent::PlayThresholdReached`],
/// at which point `commit_play_for_path` consumes it to write the
/// catalog `play_count` increment and (when `record_history` is
/// `true`) the `PlaybackHistoryEntry`.
///
/// Replaced wholesale on every `play_track_with_history` call: a
/// pending entry that never reaches the threshold is silently
/// discarded when the next play overwrites it, which is exactly the
/// "don't litter history when scrubbing through tracks" behavior we
/// want.
///
/// Not persisted — a deferred play that hadn't crossed the threshold
/// at quit time is effectively a "didn't really play it" and should
/// not survive a restart.
#[derive(Clone)]
struct PendingPlay {
    path: PathBuf,
    /// Mirrors the `record_history` arg passed into
    /// `play_track_with_history`. `false` for the device-switch
    /// reload path so picking a new output device doesn't double-
    /// count the in-progress play.
    record_history: bool,
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
    left_sidebar_collapsed: bool,
    #[serde(default)]
    right_sidebar_collapsed: bool,
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
    analytics_time_range: AnalyticsTimeRange,
    #[serde(default)]
    analytics_sidebar_collapsed: bool,
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
    #[serde(default = "default_window_restore_mode")]
    window_restore_mode: WindowRestoreMode,
    /// One-shot flag tracking whether the Liked-column position
    /// migration has run on this saved state. The migration moves a
    /// legacy trailing `Liked` (formerly inert `Loved`) column to the
    /// new default slot right after `#`. Once we've done it on a
    /// given state file, we leave the user's layout alone forever
    /// after -- so dragging Liked back to the end sticks across
    /// restarts.
    #[serde(default)]
    liked_column_migrated: bool,
    /// Persisted choice of which view the right (Up Next) sidebar
    /// shows. Defaults to `Queue` for users who didn't have this
    /// field in their saved state.
    #[serde(default)]
    right_sidebar_view: RightSidebarView,
    /// Persisted seekbar visualizer choice. Defaults to `Waveform`
    /// (the original behaviour) for users who didn't have this field
    /// in their saved state.
    #[serde(default)]
    seekbar_visualizer: VisualizerKind,
    /// Whether the equalizer is engaged. `false` (i.e. bypass on)
    /// means the EQ source passes audio through unmodified — the
    /// safe default for users who haven't opened the panel yet.
    #[serde(default)]
    eq_enabled: bool,
    /// Preamp gain in dB applied before the EQ band cascade.
    /// Range: [-12, +12].
    #[serde(default)]
    eq_preamp_db: f32,
    /// Per-band gains in dB. 10 entries; ISO octaves at
    /// [`tempo::equalizer::BAND_FREQS_HZ`]. Range per band:
    /// [-12, +12].
    #[serde(default = "default_eq_gains")]
    eq_gains_db: [f32; tempo::equalizer::BAND_COUNT],
    /// Reference to the profile (built-in or user) the live EQ
    /// values were last loaded from. `None` means the user is in
    /// "ad-hoc" mode (sliders changed without loading a profile).
    /// The UI uses this to render the "active profile" label and
    /// the dirty `*` indicator.
    #[serde(default)]
    eq_active_profile: Option<tempo::equalizer::EqProfileRef>,
    /// User-saved EQ profiles. Built-in profiles are not stored
    /// here — they're embedded in code and referenced by name.
    #[serde(default)]
    eq_profiles: Vec<tempo::equalizer::EqProfile>,
    /// User-bound global hotkeys, keyed by `HotkeyAction::storage_key()`.
    /// Empty for new users (they record their own combos in
    /// Settings → Hotkeys); preserved across versions.
    #[serde(default)]
    global_hotkeys: HashMap<String, tempo::hotkeys::KeyCombo>,
    /// `true` after the user has seen the "Tempo continues in the tray"
    /// toast that fires the first time they X-out of the window. We
    /// only show it once per install so it doesn't become noise.
    #[serde(default)]
    seen_tray_minimize_toast: bool,
}

fn default_eq_gains() -> [f32; tempo::equalizer::BAND_COUNT] {
    [0.0; tempo::equalizer::BAND_COUNT]
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
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
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
            analytics_time_range: AnalyticsTimeRange::default(),
            analytics_sidebar_collapsed: false,
            genre_grid_scroll_top: 0.0,
            artist_grid_scroll_top: 0.0,
            artist_table_scroll_top: 0.0,
            album_grid_scroll_top: 0.0,
            album_table_scroll_top: 0.0,
            playback_history: Vec::new(),
            playing_track_path: None,
            online_metadata_mode: default_online_metadata_mode(),
            window_restore_mode: default_window_restore_mode(),
            // Fresh app state already builds the right default layout,
            // so the migration is a no-op. Mark it done so we don't
            // re-run it spuriously on the second launch.
            liked_column_migrated: true,
            right_sidebar_view: RightSidebarView::default(),
            seekbar_visualizer: VisualizerKind::default(),
            eq_enabled: false,
            eq_preamp_db: 0.0,
            eq_gains_db: default_eq_gains(),
            eq_active_profile: None,
            eq_profiles: Vec::new(),
            global_hotkeys: HashMap::new(),
            seen_tray_minimize_toast: false,
        }
    }
}

fn default_online_metadata_mode() -> OnlineMetadataMode {
    OnlineMetadataMode::Off
}

fn default_window_restore_mode() -> WindowRestoreMode {
    WindowRestoreMode::BringHere
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
    error_column_widths: ErrorColumnWidths,
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
    /// Right-click context menu state for an Up Next queue row.
    /// `None` means no menu is open. Mirrors `playlist_context_menu`.
    queue_context_menu: Option<QueueContextMenu>,
    /// Right-click context menu state for a History row in the right
    /// sidebar. `None` means no menu is open.
    history_context_menu: Option<HistoryContextMenu>,
    /// Which view the right (Up Next) sidebar shows. Persisted in
    /// `state.json`. The header label acts as a dropdown trigger that
    /// flips this between `Queue` and `History`.
    right_sidebar_view: RightSidebarView,
    /// Whether the right-sidebar view-picker dropdown is currently
    /// open. The trigger lives in the queue header (`render_queue`).
    right_sidebar_view_menu_open: bool,
    /// Mouse-down position recorded when the view-picker dropdown
    /// was opened. The dropdown panel anchors here. Updated each time
    /// the dropdown is toggled open.
    right_sidebar_view_menu_position: Point<Pixels>,
    /// Currently visible section of the Settings page. Runtime-only
    /// (not persisted) — initialized in `TempoApp::new` based on
    /// whether any library roots are configured.
    settings_section: SettingsSection,
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
    /// Active time-range filter on the Analytics page. Persisted in
    /// app state so the user's last choice sticks across launches.
    analytics_time_range: AnalyticsTimeRange,
    /// Whether the Analytics filter sidebar is collapsed. Independent
    /// of `right_sidebar_collapsed` because the analytics sidebar is
    /// a different surface (filters vs. queue/history) and users
    /// typically want both states remembered separately.
    analytics_sidebar_collapsed: bool,
    queue: Vec<usize>,
    /// Index into `self.queue` of the entry that is currently
    /// playing, or was most recently played from the queue. `None`
    /// means playback is not "in" the queue right now (e.g. the user
    /// double-clicked a row in the main table, or the queue was
    /// empty when the current track started). Used purely for the
    /// Up Next sidebar's active-row indicator and for forward
    /// auto-advance; clearing the queue or playing from outside the
    /// queue resets it to `None`.
    queue_cursor: Option<usize>,
    library_roots: Vec<PathBuf>,
    playlists: Vec<Playlist>,
    playback_history: Vec<PlaybackHistoryEntry>,
    /// In-flight deferred play awaiting the 15 s listening threshold.
    /// `Some` from the moment `play_track_with_history` succeeds until
    /// the player emits `PlayThresholdReached` (which calls
    /// `commit_play_for_path` to consume it) or until the next
    /// `play_track_with_history` overwrites it (which is how
    /// quick-skip discards a sub-threshold play). Cleared on stop /
    /// library reload paths via `reset_pending_play`. Not persisted
    /// across app restarts — see [`PendingPlay`] docs.
    pending_play: Option<PendingPlay>,
    theme_id: String,
    themes: Vec<Theme>,
    library_root_label: String,
    library_status: String,
    online_metadata_mode: OnlineMetadataMode,
    window_restore_mode: WindowRestoreMode,
    scan_progress: ScanProgress,
    scan_errors: Vec<IndexingError>,
    /// Snapshot of failed metadata-job rows refreshed on the activity
    /// poll. Cleared to empty when the user disables online metadata
    /// mode (so stale errors don't linger forever); re-populated on
    /// the next poll while in `Automatic`.
    metadata_errors: Vec<CatalogMetadataError>,
    /// Runtime-only set of `ErrorCategory` filters currently enabled
    /// on the Errors page badge bar. Intentionally NOT persisted --
    /// resets to all-on each launch.
    active_error_filters: std::collections::BTreeSet<ErrorCategory>,
    /// Last status string from a manual "Resync metadata" Settings
    /// click. Surfaced under the button label until the next poll
    /// updates the activity counters. Runtime-only.
    metadata_resync_status: Option<String>,
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
    errors_scroll_handle: UniformListScrollHandle,
    playback_history_scroll_handle: UniformListScrollHandle,
    liked_scroll_handle: UniformListScrollHandle,
    /// Scroll handle for the Up Next queue's `uniform_list` in the
    /// right sidebar. Kept on the app so scroll position survives
    /// across re-renders (e.g. theme changes, view-picker toggles).
    queue_sidebar_scroll_handle: UniformListScrollHandle,
    /// Scroll handle for the History view's `uniform_list` in the
    /// right sidebar.
    history_sidebar_scroll_handle: UniformListScrollHandle,
    /// Scroll handle for the per-playlist view's `uniform_list` in
    /// the right sidebar. Shared across playlists; resets each time
    /// the user switches which playlist is being viewed (which is
    /// the natural behavior of `track_scroll` against a list whose
    /// `item_count` changed).
    playlist_sidebar_scroll_handle: UniformListScrollHandle,
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
    /// Denormalized mirror of `self.player.read(cx).seekbar_visualizer()`.
    /// Same rationale as `volume_snapshot`: the player owns the
    /// authoritative choice and emits [`PlayerEvent::StateMutated`]
    /// after the user picks one; this field is read by `app_state`
    /// (the serializer) without needing `cx`.
    seekbar_visualizer_snapshot: VisualizerKind,

    // === Equalizer ===
    /// Shared EQ state — same `Arc` the audio thread reads. UI
    /// mutations go through [`tempo::equalizer::EqState`]'s atomic
    /// setters which both update the value and bump the version
    /// counter the audio thread polls.
    eq_state: tempo::equalizer::EqState,
    /// User-saved EQ profiles. Built-ins are referenced by name
    /// from [`tempo::equalizer::BUILTIN_PRESETS`].
    eq_profiles: Vec<tempo::equalizer::EqProfile>,
    /// Reference to the profile (built-in or user) currently loaded
    /// into the live sliders. `None` = ad-hoc edits. When `Some`,
    /// the panel header shows the profile's name plus a `*` marker
    /// when the live values diverge from the stored profile values.
    eq_active_profile: Option<tempo::equalizer::EqProfileRef>,
    /// Whether the EQ panel is open. The trigger button toggles
    /// this; click-away closes it. Anchored at [`Self::eq_panel_anchor`].
    eq_panel_open: bool,
    eq_panel_anchor: Point<Pixels>,
    /// Whether the profile-picker dropdown inside the EQ panel is
    /// open. Closed automatically on profile selection.
    eq_profile_menu_open: bool,
    /// Mid-drag slider state. While `Some`, mouse-move events on
    /// the panel update the dragged band's gain.
    eq_slider_drag: Option<EqSliderDrag>,
    /// Pending profile-deletion confirmation. Holds the user
    /// profile's id; built-ins can't be deleted.
    eq_profile_delete_confirm: Option<String>,
    /// Inline "Save as new" input state. While `Some`, the panel
    /// shows a text input where the user types a name for a new
    /// user profile to be created from the live values.
    eq_profile_save_as: Option<EqProfileSaveAs>,
    eq_profile_save_as_focus_handle: Option<FocusHandle>,
    /// Held subscription that forwards [`player::PlayerEvent`]s into
    /// `TempoApp::handle_player_event`. Dropping this subscription
    /// would silently break cross-region updates (e.g. table active
    /// row would stop refreshing on track change), so it lives on the
    /// app for the app's lifetime.
    _player_subscription: Subscription,
    _save_on_quit: Option<Subscription>,
    /// Observer fired by GPUI when the main window's "active" state
    /// (= has keyboard focus, on every backend) flips. We use it to
    /// flip [`window_hidden`] back to `false` when the compositor
    /// restores us via a path outside our control (taskbar click,
    /// alt-tab, etc.). Held for the app's lifetime; dropping it
    /// would mean the tray menu's "Show / Hide Tempo" label could
    /// drift out of sync with the actual window state.
    _activation_subscription: Option<Subscription>,
    /// Top-level window layout mode (full vs mini). Runtime-only;
    /// not serialized to `state.json`.
    pub(crate) window_mode: WindowMode,
    /// Bounds the window had immediately before entering mini mode,
    /// so we can restore them when returning to full. Captured the
    /// frame `RequestEnterMini` is processed (see `enter_mini_mode`).
    saved_full_bounds: Option<Bounds<Pixels>>,
    /// `Some(_)` when any mini-mode change (enter / exit / size
    /// cycle) has been requested. Causes a window-recreate (close
    /// current, open a new one with the target bounds, mount the
    /// same `Entity<TempoApp>` as root). See [`PendingWindowSwap`]
    /// for why all three transitions go through this rather than
    /// in-place `window.resize` — Wayland compositors generally
    /// treat resize as advisory and ignore it for tiled windows,
    /// so a real reopen is the only reliable way to change the
    /// window's actual size.
    pending_window_swap: Option<PendingWindowSwap>,
    /// Same indirection for the OS title bar (used to surface the
    /// current track in mini mode so the system task switcher /
    /// taskbar shows useful info).
    pending_window_title: Option<String>,

    // === Global hotkeys (evdev) ===
    /// Background watcher reading `/dev/input/event*`. `None` if we
    /// failed to spawn the device threads (uncommon -- the service
    /// itself is constructed even when no devices were openable; the
    /// `init_status` field tells us whether any keyboards are being
    /// listened to). Kept alive for the app's lifetime; dropping it
    /// signals the watcher threads to exit.
    pub(super) hotkey_service: Option<tempo::hotkeys::HotkeyService>,
    /// Action whose binding is currently being recorded (user clicked
    /// "Record" in Settings → Hotkeys; we're waiting on the next
    /// physical keypress to capture). `None` outside of recording.
    pub(super) recording_action: Option<tempo::hotkeys::HotkeyAction>,

    // === MPRIS D-Bus server ===
    /// MPRIS server publishing playback state to the session bus so
    /// the standard media keys (XF86AudioPlay/Next/Prev), GNOME's
    /// "now playing" widget, KDE Connect, etc. all just work without
    /// per-app config. `None` when we couldn't acquire the bus name
    /// (most often: a second Tempo instance is already running).
    pub(super) mpris_service: Option<tempo::mpris::MprisService>,
    /// Local single-instance socket. Later launches send a focus
    /// request here and exit so users get the existing playback
    /// process/window instead of a second Tempo.
    pub(super) single_instance_server: Option<tempo::single_instance::SingleInstanceServer>,

    // === System tray (StatusNotifierItem) ===
    /// Linux SNI tray icon, kept alive for the app's lifetime. `None`
    /// on non-Linux targets or when the D-Bus session is unreachable.
    /// The tray, not the main window, owns Tempo's process lifetime:
    /// closing the X is intercepted in `main.rs` and minimizes the
    /// window instead, so audio keeps playing. Only the tray menu's
    /// "Quit Tempo" item triggers `cx.quit()`.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(super) tray_service: Option<tempo::tray::TrayService>,

    // === Window lifecycle ===
    /// Handle to the most recently created Tempo window. Updated by
    /// [`TempoApp::set_main_window`] from `main.rs` at boot and from
    /// the mini-mode window-swap path in [`TempoApp::render`]. Used
    /// by the tray, MPRIS `Raise`, and global-hotkey `ShowWindow`
    /// path through [`TempoApp::focus_main_window`].
    pub(super) main_window: Option<WindowHandle<Self>>,
    /// `true` after the user has dismissed (or simply seen) the
    /// "Tempo continues in the tray" toast that fires the first time
    /// the X-button is intercepted into a minimize. Persisted in
    /// `AppState` so re-launching doesn't re-show it.
    pub(super) seen_tray_minimize_toast: bool,
    /// Tracks whether the main window has been minimized to the tray
    /// (true) or is currently visible (false). Drives the tray
    /// menu's "Show Tempo" / "Hide Tempo" label flip via
    /// [`TempoApp::set_window_hidden`]. Updated by every code path
    /// that hides or shows the window. Not persisted — fresh
    /// launches always start with the window visible.
    pub(super) window_hidden: bool,
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
        // Build the shared EQ state up front, seeded from saved
        // settings so the engine starts with the user's last EQ
        // applied (no first-buffer "wrong EQ" pop on startup).
        let eq_state = tempo::equalizer::EqState::new();
        eq_state.load_profile(&state.eq_gains_db, state.eq_preamp_db, !state.eq_enabled);
        let player = cx.new(|player_cx| {
            player::PlayerEntity::new(
                volume,
                state.output_device.clone(),
                state.seekbar_visualizer,
                catalog.clone(),
                initial_theme_colors,
                eq_state.clone(),
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
            left_sidebar_collapsed: state.left_sidebar_collapsed,
            right_sidebar_collapsed: state.right_sidebar_collapsed,
            column_widths: ColumnWidths::default(),
            artist_table_column_widths: ArtistTableColumnWidths::default(),
            album_table_column_widths: AlbumTableColumnWidths::default(),
            genre_table_column_widths: GenreTableColumnWidths::default(),
            error_column_widths: ErrorColumnWidths::default(),
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
            queue_context_menu: None,
            history_context_menu: None,
            right_sidebar_view: state.right_sidebar_view,
            right_sidebar_view_menu_open: false,
            right_sidebar_view_menu_position: Point::default(),
            settings_section: if roots.is_empty() {
                SettingsSection::Library
            } else {
                SettingsSection::Appearance
            },
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
            analytics_time_range: state.analytics_time_range,
            analytics_sidebar_collapsed: state.analytics_sidebar_collapsed,
            queue: Vec::new(),
            queue_cursor: None,
            library_roots: roots,
            playlists,
            theme_id,
            themes,
            library_root_label,
            library_status,
            online_metadata_mode: state.online_metadata_mode,
            window_restore_mode: state.window_restore_mode,
            scan_progress: ScanProgress::default(),
            scan_errors: Vec::new(),
            metadata_errors: Vec::new(),
            active_error_filters: ErrorCategory::ALL.iter().copied().collect(),
            metadata_resync_status: None,
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
            errors_scroll_handle: UniformListScrollHandle::new(),
            playback_history_scroll_handle: UniformListScrollHandle::new(),
            liked_scroll_handle: UniformListScrollHandle::new(),
            queue_sidebar_scroll_handle: UniformListScrollHandle::new(),
            history_sidebar_scroll_handle: UniformListScrollHandle::new(),
            playlist_sidebar_scroll_handle: UniformListScrollHandle::new(),
            table_is_scrolling: false,
            table_scroll_generation: 0,
            catalog,
            playback_history: state.playback_history,
            pending_play: None,
            _library_watcher: library_watcher,
            metadata_event_tx,
            metadata_demand_queue,
            metadata_status_expanded: false,
            _metadata_worker: None,
            player,
            volume_snapshot: volume,
            output_device_snapshot: state.output_device.clone(),
            seekbar_visualizer_snapshot: state.seekbar_visualizer,
            eq_state,
            eq_profiles: state.eq_profiles,
            eq_active_profile: state.eq_active_profile,
            eq_panel_open: false,
            eq_panel_anchor: Point::default(),
            eq_profile_menu_open: false,
            eq_slider_drag: None,
            eq_profile_delete_confirm: None,
            eq_profile_save_as: None,
            eq_profile_save_as_focus_handle: None,
            _player_subscription: player_subscription,
            _save_on_quit: Some(save_on_quit),
            _activation_subscription: None,
            window_mode: WindowMode::Full,
            saved_full_bounds: None,
            pending_window_swap: None,
            pending_window_title: None,
            hotkey_service: None,
            recording_action: None,
            mpris_service: None,
            single_instance_server: None,
            #[cfg(all(unix, not(target_os = "macos")))]
            tray_service: None,
            main_window: None,
            seen_tray_minimize_toast: state.seen_tray_minimize_toast,
            window_hidden: false,
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
        // Boot the global hotkey service (evdev) and the MPRIS D-Bus
        // server. Failures (no /dev/input access, no session bus,
        // already-running instance) are logged and swallowed -- the
        // app still works without them, just without remote-control
        // surfaces.
        perf::time("startup.start_global_hotkeys", "", || {
            app.start_global_hotkeys(state.global_hotkeys.clone(), cx)
        });
        perf::time("startup.start_mpris", "", || app.start_mpris_server(cx));
        // System tray (StatusNotifierItem). Linux only; Windows
        // and macOS ship in follow-up PRs via native APIs. Failures
        // (no D-Bus session, no SNI watcher, sandboxed without
        // policy) are logged and swallowed.
        #[cfg(all(unix, not(target_os = "macos")))]
        perf::time("startup.start_tray", "", || app.start_tray_service(cx));
        // Observe activation transitions so taskbar / alt-tab
        // restores flip `window_hidden` back to false (and the tray
        // menu re-renders "Hide Tempo"). We only flip false-on-
        // activation; deactivation doesn't imply the window was
        // hidden — another app may simply have focus.
        app._activation_subscription =
            Some(cx.observe_window_activation(window, |this, window, _cx| {
                if window.is_window_active() && this.window_hidden {
                    this.set_window_hidden(false);
                }
            }));
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

    /// Manual "Resync metadata" entry point invoked from the Online
    /// Metadata settings panel. Spawns a background thread that calls
    /// [`CatalogStore::resync_metadata_enrichment`] and reports
    /// completion via `metadata_resync_status`. The worker picks up
    /// the newly-pending jobs on its normal claim loop -- we don't
    /// have to touch it from here.
    fn run_metadata_resync(&mut self) {
        let Some(catalog) = self.catalog.clone() else {
            self.metadata_resync_status = Some("Catalog unavailable".to_string());
            return;
        };
        if self.online_metadata_mode != OnlineMetadataMode::Automatic {
            self.metadata_resync_status = Some("Enable Online Metadata to resync".to_string());
            return;
        }

        self.metadata_resync_status = Some("Resync started...".to_string());

        std::thread::Builder::new()
            .name("tempo-metadata-resync".into())
            .spawn(move || match catalog.resync_metadata_enrichment() {
                Ok(count) => perf::event(
                    "metadata.resync.user_triggered",
                    format!("enqueued={count}"),
                ),
                Err(error) => perf::event(
                    "metadata.resync.user_triggered_error",
                    format!("error={error:#}"),
                ),
            })
            .ok();
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

    /// On-demand counterpart to [`queue_album_cover_demand`] for album
    /// descriptions. Called by the album-detail hero when it renders
    /// without a description in hand. Gated on `OnlineMetadataMode::Automatic`
    /// so toggling enrichment off in Settings cleanly stops new
    /// demands without touching jobs already in flight.
    fn queue_album_profile_demand(&self, album_id: i64) {
        if self.online_metadata_mode != OnlineMetadataMode::Automatic {
            return;
        }
        let Some(catalog) = self.catalog.clone() else {
            return;
        };
        let Ok(mut demand_queue) = self.metadata_demand_queue.lock() else {
            return;
        };
        if !demand_queue.album_descriptions.insert(album_id) {
            return;
        }
        drop(demand_queue);

        std::thread::Builder::new()
            .name("tempo-metadata-demand".into())
            .spawn(move || {
                if let Err(error) = catalog.enqueue_album_profile_demand(album_id) {
                    perf::event(
                        "metadata.demand.album_profile_error",
                        format!("album_id={album_id} error={error:#}"),
                    );
                }
            })
            .ok();
    }

    /// Whether the unified Errors page has anything to show today.
    /// Used by the sidebar to gate visibility of the Errors entry.
    pub(super) fn has_any_errors(&self) -> bool {
        !self.scan_errors.is_empty() || !self.metadata_errors.is_empty()
    }

    /// Combined count for the sidebar badge. Counts both scan errors
    /// and metadata-job failures regardless of the active badge
    /// filters (the filters are a *display* concern; the sidebar
    /// reflects the underlying state).
    pub(super) fn total_error_count(&self) -> usize {
        self.scan_errors.len() + self.metadata_errors.len()
    }

    /// Materialize every error -- scan + metadata-job -- into a uniform
    /// shape, filtered by `active_error_filters`. Sort order: most
    /// recently observed first. Scan errors don't carry a per-row
    /// timestamp so we render them after metadata errors of the same
    /// poll batch (most recent metadata first, then scans).
    pub(super) fn visible_error_rows(&self) -> Vec<ErrorRowView> {
        let mut rows: Vec<ErrorRowView> =
            Vec::with_capacity(self.scan_errors.len() + self.metadata_errors.len());

        for error in self.metadata_errors.iter() {
            let category = error
                .error_kind
                .as_deref()
                .and_then(ErrorCategory::from_error_kind)
                .unwrap_or(ErrorCategory::Network);
            if !self.active_error_filters.contains(&category) {
                continue;
            }
            rows.push(ErrorRowView {
                category,
                path_label: format!("{}: {}", error.entity_type, error.entity_label),
                message: error.message.clone(),
            });
        }

        // Scan errors are always category=Scan; honour the filter.
        if self.active_error_filters.contains(&ErrorCategory::Scan) {
            for error in self.scan_errors.iter().rev() {
                rows.push(ErrorRowView {
                    category: ErrorCategory::Scan,
                    path_label: error.path.display().to_string(),
                    message: error.message.clone(),
                });
            }
        }

        rows
    }

    pub(super) fn toggle_error_filter(&mut self, category: ErrorCategory) {
        if self.active_error_filters.contains(&category) {
            self.active_error_filters.remove(&category);
        } else {
            self.active_error_filters.insert(category);
        }
    }

    pub(super) fn set_all_error_filters(&mut self, enabled: bool) {
        if enabled {
            self.active_error_filters = ErrorCategory::ALL.iter().copied().collect();
        } else {
            self.active_error_filters.clear();
        }
    }

    /// Per-category total counts -- used by the badge bar to show
    /// pill counts and to dim categories with zero rows. Includes the
    /// full underlying state regardless of the active filter set.
    pub(super) fn error_counts_by_category(
        &self,
    ) -> std::collections::HashMap<ErrorCategory, usize> {
        let mut counts: std::collections::HashMap<ErrorCategory, usize> =
            std::collections::HashMap::new();
        counts.insert(ErrorCategory::Scan, self.scan_errors.len());
        for error in &self.metadata_errors {
            let category = error
                .error_kind
                .as_deref()
                .and_then(ErrorCategory::from_error_kind)
                .unwrap_or(ErrorCategory::Network);
            *counts.entry(category).or_insert(0) += 1;
        }
        counts
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

        let key = event.keystroke.key.as_str().to_lowercase();

        match key.as_str() {
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
                    match key.as_str() {
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
            | Page::Errors
            | Page::Analytics
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
    description: Option<&str>,
) -> String {
    format!(
        "{} {} {} {} {}",
        title,
        artist,
        year.unwrap_or_default(),
        track_count,
        description.unwrap_or_default(),
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
            album.description.as_deref(),
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
            description: album.description,
            description_state: album.description_state,
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

        // Drain pending mini-mode window adjustments. Event handlers
        // (e.g. `RequestEnterMini`) don't get a `&mut Window`, so
        // they stash a target-bounds value here and the swap happens
        // on the next paint where we have `&mut Window` + `&mut App`.
        //
        // The swap is implemented as "open a new window with the
        // target bounds, mounting the same `Entity<TempoApp>` as its
        // root, then close the current window" rather than
        // `window.resize(...)`. Hyprland and several other Wayland
        // compositors ignore most client-driven resize requests, so
        // a real reopen is the only reliable way to actually shrink
        // the window for the mini player. The shared entity means
        // all in-memory state (current track, queue, history, audio
        // backend) survives the swap — only the window itself is
        // disposable.
        if let Some(swap) = self.pending_window_swap.take() {
            // Capture pre-mini bounds the moment we transition into
            // mini mode (`saved_full_bounds.is_none()`) so a later
            // `RequestExitMini` can restore exactly the same size +
            // position. Done here rather than in the event handler
            // because event handlers don't get `&mut Window`.
            if matches!(self.window_mode, WindowMode::Mini(_)) && self.saved_full_bounds.is_none() {
                self.saved_full_bounds = Some(match window.window_bounds() {
                    WindowBounds::Windowed(b)
                    | WindowBounds::Maximized(b)
                    | WindowBounds::Fullscreen(b) => b,
                });
            }

            // Resolve the target bounds. If the caller specified
            // explicit bounds (the restore-to-full path), use them
            // verbatim. Otherwise center the new window's content
            // rect on the current window's center so size cycles
            // don't drift to the corner of the screen.
            let target_bounds = if let Some(b) = swap.explicit_bounds {
                b
            } else {
                let current = window.bounds();
                let cx_pt = current.origin.x + current.size.width / 2.0;
                let cy_pt = current.origin.y + current.size.height / 2.0;
                let new_origin = Point {
                    x: cx_pt - swap.target_size.width / 2.0,
                    y: cy_pt - swap.target_size.height / 2.0,
                };
                Bounds {
                    origin: new_origin,
                    size: swap.target_size,
                }
            };
            let entity = cx.entity();
            let current_handle = window.window_handle();
            // Defer both the open and the close until *after* the
            // current effect cycle. Calling `cx.open_window` directly
            // here would re-enter the entity-update of `TempoApp`
            // (because GPUI calls `window.draw` synchronously during
            // `open_window`, which renders the new window's root,
            // which is the same entity we're inside right now) and
            // panic with "cannot update TempoApp while it is already
            // being updated". Deferring drops the outer render's
            // lease before the new window's first draw runs.
            //
            // Pick the toplevel "kind" hint for the replacement
            // window. Tiling Wayland compositors (Hyprland, river,
            // sway) generally float windows whose Wayland
            // `xdg_toplevel` is flagged as transient (via
            // `xdg_toplevel.set_parent`). GPUI's
            // `WindowKind::Floating` does exactly that at window
            // creation: it calls `toplevel.set_parent(focused_window)`,
            // which Hyprland and friends read as "transient → float
            // by default". Using it on the mini window means users
            // don't need to add a `windowrulev2 = float, class:tempo`
            // rule themselves.
            //
            // For the full window we go back to `Normal` so the
            // restored full window tiles like a regular app window
            // (matching the behaviour of the very first launch).
            //
            // TODO(always-on-top): GPUI 0.2.2 doesn't expose a
            // runtime always-on-top API. Once it does (or once a
            // platform shim using `_NET_WM_STATE_ABOVE` / a
            // wlr-layer-shell client is added), set the new window
            // to top-most when entering mini mode and unset on exit.
            let target_kind = match self.window_mode {
                WindowMode::Mini(_) => gpui::WindowKind::Floating,
                WindowMode::Full => gpui::WindowKind::Normal,
            };
            cx.defer(move |cx| {
                let options = gpui::WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(target_bounds)),
                    app_id: Some("tempo".into()),
                    kind: target_kind,
                    ..Default::default()
                };
                match cx.open_window(options, |_window, _cx| entity) {
                    Ok(handle) => {
                        // Re-register the X-button-to-minimize
                        // interceptor on the new window and update
                        // the entity's `main_window` pointer so the
                        // tray and MPRIS Raise route to the latest
                        // window.
                        let close_handle = handle;
                        handle
                            .update(cx, |app, window, cx| {
                                window.on_window_should_close(cx, move |window, cx| {
                                    window.minimize_window();
                                    #[cfg(all(unix, not(target_os = "macos")))]
                                    tempo::hypr::hide_window();
                                    close_handle
                                        .update(cx, |app, _window, cx| {
                                            app.on_window_close_intercepted(cx);
                                        })
                                        .ok();
                                    false
                                });
                                app.set_main_window(handle);
                            })
                            .ok();
                        // Close the previous window after the new
                        // one has been mounted (deferred again to
                        // sequence cleanly after the new window's
                        // first draw).
                        cx.defer(move |cx| {
                            current_handle
                                .update(cx, |_view, window, _cx| window.remove_window())
                                .ok();
                        });
                    }
                    Err(error) => {
                        perf::event("mini.window_swap_failed", format!("error={error:#}"));
                    }
                }
            });
        }
        if let Some(new_title) = self.pending_window_title.take() {
            window.set_window_title(&new_title);
        }

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
            .on_action(cx.listener(Self::toggle_mini_player_action))
            .on_action(cx.listener(Self::cycle_mini_player_action))
            .on_action(cx.listener(Self::hide_window_action))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(Self::navigate_back_mouse),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(Self::navigate_forward_mouse),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    if this.close_transient_menus(cx) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    if this.close_transient_menus(cx) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag_volume(event, cx) {
                    cx.stop_propagation();
                }
                if this.eq_slider_drag.is_some() {
                    this.drag_eq_slider(f32::from(event.position.y));
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_volume_drag(cx) {
                        cx.stop_propagation();
                    }
                    if this.end_eq_slider_drag() {
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
            // Top region (sidebar + content) is hidden in mini
            // mode; the mini player fills the whole window via the
            // child below.
            .when(matches!(self.window_mode, WindowMode::Full), |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(self.render_left_sidebar(cx))
                        .child(self.render_content(window, cx)),
                )
            })
            // Player bar: when the library is empty there's no
            // currently-playing track snapshot to render with, so we
            // fall back to an inline placeholder that knows about
            // scan state. Otherwise we embed the `PlayerEntity` as a
            // child element — its `cx.notify()` calls only invalidate
            // the player's own subtree, which is what unlocks the
            // localized-invalidation perf win.
            //
            // In mini mode the player entity itself fills the entire
            // window (it branches its own `Render` impl on
            // `window_mode`), so the same `self.player.clone()`
            // covers both layouts.
            .child(if self.tracks.is_empty() {
                self.render_empty_player_bar(cx)
            } else {
                let _ = window;
                if matches!(self.window_mode, WindowMode::Mini(_)) {
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.player.clone())
                        .into_any_element()
                } else {
                    self.player.clone().into_any_element()
                }
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
            .when(self.queue_context_menu.is_some(), |this| {
                this.child(self.render_queue_context_menu(cx))
            })
            .when(self.history_context_menu.is_some(), |this| {
                this.child(self.render_history_context_menu(cx))
            })
            .when(self.right_sidebar_view_menu_open, |this| {
                this.child(self.render_right_sidebar_view_menu(cx))
            })
            .when(self.column_menu_open, |this| {
                this.child(self.render_column_menu(cx))
            })
            .when(self.player.read(cx).settings_output_menu_open(), |this| {
                this.child(self.settings_output_device_menu(cx))
            })
            .when(self.eq_panel_open, |this| {
                this.child(self.render_eq_panel(cx))
            })
            .when(self.eq_panel_open && self.eq_profile_menu_open, |this| {
                this.child(self.render_eq_profile_menu_overlay(cx))
            })
            .when_some(self.tooltip.clone(), |this, tooltip| {
                this.child(self.render_tooltip(&tooltip))
            })
    }
}

impl TempoApp {
    fn close_transient_menus(&mut self, cx: &mut Context<Self>) -> bool {
        let mut closed = false;
        if self.context_menu_track.take().is_some() {
            closed = true;
        }
        if self.playlist_context_menu.take().is_some() {
            closed = true;
        }
        if self.queue_context_menu.take().is_some() {
            closed = true;
        }
        if self.history_context_menu.take().is_some() {
            closed = true;
        }
        if self.column_menu_open {
            self.column_menu_open = false;
            closed = true;
        }
        if self.right_sidebar_view_menu_open {
            self.right_sidebar_view_menu_open = false;
            closed = true;
        }
        if self.close_eq_panel() {
            closed = true;
        }
        if self.player.update(cx, |player, player_cx| {
            let closed = player.close_seekbar_menu();
            if closed {
                player_cx.notify();
            }
            closed
        }) {
            closed = true;
        }
        // Output device picker (player-bar and Settings-anchored
        // variants both use this). Without this branch the dropdown
        // stays open until the user picks a device — see
        // `PlayerEntity::close_output_menu`.
        if self.player.update(cx, |player, player_cx| {
            let was_open = player.output_menu_source().is_some();
            if was_open {
                player.close_output_menu(player_cx);
            }
            was_open
        }) {
            closed = true;
        }
        closed
    }

    fn page_label(page: Page) -> &'static str {
        match page {
            Page::Library => "library",
            Page::Artists => "artists",
            Page::Albums => "albums",
            Page::Genres => "genres",
            Page::Liked => "liked",
            Page::PlaybackHistory => "playback_history",
            Page::Errors => "errors",
            Page::Analytics => "analytics",
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

        if self
            .playlist_rename_focus_handle
            .as_ref()
            .is_some_and(|focus_handle| focus_handle.is_focused(window))
        {
            self.commit_playlist_rename();
            cx.notify();
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
            self.search_input.insert(" ");
            self.schedule_current_search_input(cx);
            cx.notify();
            return;
        }

        if self
            .playlist_rename_focus_handle
            .as_ref()
            .is_some_and(|focus_handle| focus_handle.is_focused(window))
        {
            if let Some(rename) = self.playlist_rename.as_mut() {
                rename.input.insert(" ");
                cx.notify();
            }
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

    fn hide_window_action(&mut self, _: &HideWindow, _: &mut Window, cx: &mut Context<Self>) {
        self.hide_main_window(cx);
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

    fn toggle_mini_player_action(
        &mut self,
        _: &ToggleMiniPlayer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.window_mode {
            WindowMode::Full => self.enter_mini_mode(cx),
            WindowMode::Mini(_) => self.exit_mini_mode(cx),
        }
        cx.notify();
    }

    /// Ctrl+Shift+M action handler.
    ///
    /// - When already in mini mode, rotates to the next mini size
    ///   (`CompactBar → Square → LargeSquare → CompactBar`).
    /// - When in full mode, enters mini mode at the default size,
    ///   so this keybinding doubles as an alternate enter-mini
    ///   shortcut alongside Ctrl+M.
    fn cycle_mini_player_action(
        &mut self,
        _: &CycleMiniPlayer,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.window_mode {
            WindowMode::Full => self.enter_mini_mode(cx),
            WindowMode::Mini(_) => self.cycle_mini_size(cx),
        }
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
