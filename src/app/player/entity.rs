//! [`PlayerEntity`] — the audio playback subsystem as a standalone GPUI
//! entity, split out of [`TempoApp`] so that per-second playback ticks
//! and per-frame waveform shimmer animations only invalidate the player
//! bar instead of the entire app tree.
//!
//! ## Design overview
//!
//! `PlayerEntity` owns:
//! - the audio backend ([`PlaybackController`]) and its derived status
//!   labels,
//! - the currently-playing track *path* (not index — see "Identity"
//!   below),
//! - the volume slider state (including the in-flight drag),
//! - the output device picker state,
//! - the per-track waveform cache,
//! - the now-playing hover/alt-press affordances on the player bar.
//!
//! `TempoApp` retains everything that needs to *coordinate* across
//! regions:
//! - the `tracks` `Vec` and its index reverse-map,
//! - tab/queue state,
//! - "play this track" orchestration (which has to increment play
//!   counts, record history, and re-select rows in the active table —
//!   none of which `PlayerEntity` knows about),
//! - all keyboard action handlers,
//! - all modal/overlay state.
//!
//! ## Identity
//!
//! `PlayerEntity::playing_track_path` is a `PathBuf`, not a `usize`.
//! The previous monolithic design indexed into `tracks` with `usize`,
//! but every library reload reset the index to 0 (which was
//! incorrect — the user's currently-playing track moves in the list).
//! Keying by path means playback survives reloads, and cross-region
//! callers (table active-row highlight, history page) compare by
//! `track.path == player.playing_track_path()`.
//!
//! ## Communication
//!
//! - **Parent → Player**: direct method calls via `player.update(cx,
//!   |p, cx| p.start_playback(...))`.
//! - **Player → Parent**: typed [`PlayerEvent`]s emitted via
//!   `cx.emit(...)`. The parent subscribes once at construction. Events
//!   are deferred (flushed at the end of the current effect cycle) so
//!   the parent reacts on the next pass; this avoids re-entrant entity
//!   updates.
//! - For tab navigation (Now-Playing link clicks) and auto-advance
//!   (track finished → choose next via mode/queue), the player emits
//!   events and the parent does the work — `PlayerEntity` deliberately
//!   does not hold a `WeakEntity<TempoApp>` for these paths to keep the
//!   coupling one-directional. The only state `PlayerEntity` reaches
//!   *into* the parent for is the `tracks` table, and even that is
//!   indirected via events rather than direct upcalls.
//!
//! ## Why no `WeakEntity<TempoApp>` field
//!
//! An earlier draft of this module gave `PlayerEntity` a
//! `WeakEntity<TempoApp>` for "convenience". Resist the temptation:
//! - It introduces re-entrancy hazards (`app.update` inside a
//!   `player.update` will panic if both are leased).
//! - It makes it impossible to test `PlayerEntity` in isolation.
//! - Every legitimate need to talk to the parent is already expressible
//!   as an event.
//!
//! If something *seems* to need it, prefer adding a new
//! [`PlayerEvent`] variant.

use super::*;
use std::time::Instant;

/// Minimum playback position (in seconds) that must be reached before
/// a track counts as "played" — incrementing the catalog play_count
/// and being appended to the playback history. Skipping a track
/// before this threshold leaves no trace, so quickly hopping between
/// tracks while browsing doesn't litter history.
pub(crate) const PLAY_THRESHOLD_SECS: u64 = 15;

/// Events emitted by [`PlayerEntity`] for the parent to react to.
///
/// All variants are `Clone` so a single subscriber can fan them out to
/// multiple internal handlers; the events themselves are cheap (paths
/// are `PathBuf` clones, ~40 bytes).
///
/// The variants split into two conceptual groups:
///
/// 1. **State-change notifications** — `PlayingTrackChanged`,
///    `IsPlayingChanged`, `TrackFinished`, `StateMutated`. The parent
///    reacts (rerender, save, auto-advance). Emitted by player methods
///    after state mutation.
/// 2. **Command requests** — `NowPlayingLinkClicked`,
///    `RequestPlayPause`, `RequestPlayPrev`, `RequestPlayNext`,
///    `RequestPlayRandom`, `RequestSeekFromWaveformClick`,
///    `RequestSelectOutputDevice`. Emitted by user clicks inside the
///    player bar that need cross-region work (tab opening, track
///    resolution from the active tab's index list, etc.). The parent
///    handles the cross-region bits and calls back into the player
///    via direct `update`.
#[derive(Debug, Clone)]
pub(crate) enum PlayerEvent {
    // === State-change notifications ===
    /// The currently-playing track changed (or playback stopped, in
    /// which case `path` is `None`). The parent rerenders so table /
    /// history active-row highlights update.
    PlayingTrackChanged { path: Option<PathBuf> },

    /// Play/pause flipped. The parent rerenders so the table's
    /// transport-icon column (Ⅱ vs ▶) updates.
    IsPlayingChanged(bool),

    /// The currently-playing track finished naturally. The parent
    /// applies the playback mode (Loop / Shuffle / Straight), resolves
    /// the next track from the active tab's index list, and calls back
    /// into [`PlayerEntity::start_playback`].
    ///
    /// Carries the *finished* path so the parent can disambiguate from
    /// any stop/seek it issued itself.
    TrackFinished { finished_path: PathBuf },

    /// The currently-playing track has been listened to for at least
    /// [`PLAY_THRESHOLD_SECS`] seconds (measured by playback position,
    /// not wall clock — pauses don't count, but a seek past the
    /// threshold does). Emitted exactly once per `start_playback`
    /// call. The parent uses this to commit the play to history and
    /// the catalog `play_count`, so quickly skipping between tracks
    /// (under 15 s) doesn't litter the history.
    PlayThresholdReached { path: PathBuf },

    /// State that should be persisted has changed (volume committed
    /// after drag, output device chosen, playback mode cycled, etc.).
    /// The parent calls `save_app_state()` which is itself debounced.
    StateMutated,

    // === Command requests ===
    /// The user clicked one of the title/artist/album labels in the
    /// Now-Playing strip. The parent opens the corresponding tab.
    NowPlayingLinkClicked { kind: NowPlayingLink, path: PathBuf },

    /// Transport play/pause clicked. The parent calls
    /// `TempoApp::toggle_playback(cx)` which has the smart
    /// pause/resume/restart logic that needs `tracks`.
    RequestPlayPause,

    /// Transport previous (◀) clicked. Parent resolves the previous
    /// track from `current_track_indices()` and calls
    /// `play_track_with_history`.
    RequestPlayPrev,

    /// Transport next (▶) clicked.
    RequestPlayNext,

    /// Transport shuffle (↻) clicked. Parent picks a random track via
    /// `current_track_indices()` and `shuffle_key`.
    RequestPlayRandom,

    /// User clicked the waveform seekbar. `ratio` is the click's
    /// position along the seekbar in `0.0..=1.0`, computed from the
    /// seekbar's actual painted bounds (so it stays correct across
    /// window resizes). Parent computes the seek target via the
    /// currently-playing track's `duration_value` and handles
    /// backend-empty recovery.
    RequestSeekFromWaveformClick { ratio: f32 },

    /// User picked an output device from the dropdown. Parent
    /// switches the audio backend; if playback was active, it
    /// restarts the current track.
    RequestSelectOutputDevice(String),
}

/// The audio playback subsystem entity.
///
/// See module docs for the rationale behind the split. The struct is
/// intentionally laid out by concern (audio backend, identity, volume,
/// output picker, waveform, hover) rather than alphabetically, so
/// future readers can see the boundaries at a glance.
pub(crate) struct PlayerEntity {
    // === Audio backend ===
    /// `None` until [`PlayerEntity::initialize`] finishes its deferred
    /// startup probe; also `None` if the host has no audio devices.
    /// Most methods short-circuit when this is `None`.
    pub(super) playback: Option<PlaybackController>,
    /// Human-readable status string for the settings page and the
    /// Now-Playing alt-overlay. Kept here (not derived) because the
    /// strings are user-localizable copy ("Playback paused", "Seeked
    /// to 1:23", error messages), not pure functions of state.
    pub(super) playback_status: String,

    // === Identity ===
    /// Path of the currently-loaded track. `None` until the user
    /// triggers `start_playback` for the first time. Survives library
    /// reloads — see the module-level "Identity" note.
    pub(super) playing_track_path: Option<PathBuf>,
    pub(super) is_playing: bool,
    pub(super) playback_mode: PlaybackMode,
    /// `true` once the current playback has crossed
    /// [`PLAY_THRESHOLD_SECS`] of position and emitted
    /// [`PlayerEvent::PlayThresholdReached`]. Reset to `false` at the
    /// start of every `start_playback` call (and by stop / library
    /// reload) so each fresh play has to earn its history entry. The
    /// playback heartbeat tick reads this flag to fire the event at
    /// most once per play.
    pub(super) play_threshold_reached: bool,

    // === Volume slider ===
    pub(super) volume: f32,
    /// Previous non-zero volume, used to restore from mute. Volume
    /// drags update this in place so a mid-drag mute round-trips
    /// to the dragged-to value rather than to whatever was set hours
    /// ago.
    pub(super) pre_mute_volume: f32,
    /// True while the user is mid-drag on the volume bar. While true,
    /// `set_volume` skips the persistence request — `finish_volume_drag`
    /// emits one [`PlayerEvent::StateMutated`] when the drag ends.
    pub(super) volume_dragging: bool,
    pub(super) volume_bar_scroll_handle: gpui::ScrollHandle,
    /// Tracks the painted bounds of the waveform seekbar so click
    /// handling can compute a ratio against the actual rendered
    /// width — relying on `window.viewport_size()` plus the fixed
    /// player-bar layout constants gave wrong answers immediately
    /// after a window resize, since the seekbar's `flex_1` width is
    /// the only piece of the bar that grows with the viewport.
    pub(super) waveform_seekbar_scroll_handle: gpui::ScrollHandle,
    pub(super) seekbar_menu_open: bool,
    /// Click position recorded when the ✦ menu was opened. The
    /// menu floats via [`super::super::menu::menu_at`] anchored at
    /// this point so it draws above the rest of the app
    /// (table/grid views included). Without an anchored overlay the
    /// menu would render as a child of the seekbar surface and get
    /// clipped/z-ordered behind the page content above.
    pub(super) seekbar_menu_position: Point<Pixels>,
    pub(super) seekbar_fps_enabled: bool,
    pub(super) seekbar_fps_last_frame: Option<Instant>,
    pub(super) seekbar_fps_smoothed: f32,
    /// Pointer hover over the seekbar. Used by frequency-reactive
    /// visualizers to fade the precomputed-peaks waveform in over
    /// the live visualization, so the user can see roughly where a
    /// click will seek to. `Waveform` mode ignores this.
    pub(super) seekbar_hovered: bool,
    /// Eased `[0.0, 1.0]` overlay intensity for the hover-revealed
    /// waveform. Updated each frame by [`Self::sample_seekbar_hover_intensity`]
    /// from `seekbar_hovered`; the easing makes the fade in/out feel
    /// physical rather than snapping.
    pub(super) seekbar_hover_intensity: f32,
    /// Wall clock at the previous hover-intensity sample. `None`
    /// means no prior frame; the next sample will pin to the target.
    pub(super) seekbar_hover_last_sampled: Option<Instant>,
    /// Which visualizer draws inside the seekbar surface. Persisted in
    /// `state.json`. `Waveform` is the original behaviour and the only
    /// variant that doesn't depend on the live audio analyzer.
    pub(super) seekbar_visualizer: VisualizerKind,
    /// Smoothed per-band magnitudes carried across frames so the
    /// frequency-reactive visualizers can ease toward each new
    /// analyzer frame instead of snapping. The entity owns this so
    /// the smoothing survives across `Render` calls; visualizers
    /// borrow it mutably from the [`super::visualizers::VisualizerContext`].
    pub(super) band_smoothed: [f32; tempo::audio_analyzer::BAND_COUNT],

    // === Output device picker ===
    /// Saved/preferred output device name, persisted to app state.
    /// May not match the *current* device (e.g. unplugged USB DAC).
    /// `playback.as_ref().map(|p| p.output_name())` is the truth at
    /// runtime; this field is the persistence-side mirror.
    pub(super) output_device: Option<String>,
    /// Which call site opened the output picker. `None` means the menu
    /// is closed. The parent reads this to know whether to render the
    /// settings-anchored variant.
    pub(super) output_menu_source: Option<OutputMenuSource>,
    pub(super) output_menu_position: Point<Pixels>,

    // === Now-Playing strip hover ===
    pub(super) now_playing_info_hovered: bool,
    pub(super) hovered_now_playing_link: Option<NowPlayingLink>,
    /// Mirror of `window.modifiers().alt` — captured in
    /// `on_modifiers_changed` so the alt-overlay can be rendered as a
    /// pure function of state at the next paint.
    pub(super) alt_pressed: bool,

    // === Waveform cache ===
    /// Per-track waveform peaks, keyed by track path. `Arc<[f32]>`
    /// means the per-frame `cached_waveform` lookup is a refcount
    /// bump, not a clone of the ~360-float buffer. Keying by path
    /// (instead of a `Vec<Option<...>>` parallel to `tracks`) means
    /// the cache is self-managing: track adds/removes don't require
    /// any resize/sync work, and library reloads keep the entries
    /// for tracks that survive.
    pub(super) waveform_cache: HashMap<PathBuf, Arc<[f32]>>,
    /// Set of paths currently being decoded off the foreground thread.
    /// Used to avoid scheduling duplicate decode tasks when the user
    /// hovers over the same track repeatedly during a slow disk.
    pub(super) waveform_loading: HashSet<PathBuf>,

    // === Waveform morph animation ===
    //
    // When the active waveform buffer changes (loading shimmer →
    // real peaks, real peaks A → real peaks B on track skip, etc.),
    // we animate each column's height from its previous value to
    // its new target over `WAVEFORM_MORPH_DURATION`. Without this,
    // the bars snap instantaneously, which is jarring — especially
    // on song change, where the seekbar visibly "pops" to a new
    // shape mid-frame.
    //
    // The morph is driven entirely from the render path: we compare
    // the buffer returned by `cached_waveform` to
    // `waveform_displayed_buffer` from the previous render, and on
    // mismatch, snapshot the previous heights as `waveform_morph_from`
    // and stamp `waveform_morph_started`. Render then lerps from
    // `morph_from` toward the new buffer, easing out, until
    // `morph_started.elapsed() >= WAVEFORM_MORPH_DURATION` at which
    // point it clears the morph state.
    //
    // Why store the *full-resolution* (360-bar) source even though
    // the seekbar paints at variable widths: width-aware
    // downsampling depends on the painted size and is recomputed
    // each frame, so the morph has to interpolate the source
    // buffers, not the post-downsample heights. (If we lerped
    // post-downsample buffers, a window resize mid-morph would
    // produce different bar counts on consecutive frames and the
    // lerp would be undefined.)
    /// Source buffer the seekbar last consumed from `cached_waveform`.
    /// Compared (via `Arc::ptr_eq`) against the buffer returned this
    /// frame: a mismatch means the active waveform changed and the
    /// morph should restart toward the new target. The loading-
    /// shimmer generator returns a fresh `Arc` every call, which
    /// is exactly what we want — ptr_eq stays false so the morph
    /// retargets each shimmer frame in tiny increments (visually
    /// just the existing shimmer; the morph is short relative to
    /// the shimmer's amplitude so it doesn't fight it).
    pub(super) waveform_displayed_source: Option<Arc<[f32]>>,
    /// Per-bar heights actually *painted* on the previous frame
    /// (after lerp). When the source changes mid-morph, these are
    /// snapshotted into `waveform_morph_from` so the new morph
    /// starts from the user's current visual state — not from the
    /// previous *target*, which would cause a visible snap.
    pub(super) waveform_displayed_heights: Option<Arc<[f32]>>,
    /// Heights at the start of the current morph (per-bar, length
    /// = `WAVEFORM_SEGMENTS`). `None` when no morph is active.
    pub(super) waveform_morph_from: Option<Arc<[f32]>>,
    /// When the current morph started; combined with
    /// `WAVEFORM_MORPH_DURATION` to compute progress each frame.
    pub(super) waveform_morph_started: Option<Instant>,

    // === Catalog (for waveform cache I/O and play-count increments) ===
    /// Cloned from `TempoApp` at construction. `CatalogStore` is
    /// `Arc`-backed (per Phase 1.1) so cloning is a refcount bump;
    /// owning our own handle keeps the waveform-decode hot path
    /// independent of `app.update` and avoids re-entrancy.
    pub(super) catalog: Option<CatalogStore>,

    // === Render snapshot ===
    /// Snapshot of the currently-playing track's render-relevant
    /// fields, pushed by `TempoApp` after each `play_track`. `None`
    /// means "library has tracks but the player hasn't been told
    /// which is current" — should not happen at steady state, but
    /// defensively rendered as the empty placeholder.
    ///
    /// Crucially, this lets `PlayerEntity::render` execute without
    /// borrowing the parent's `tracks` `Vec`, which is what enables
    /// the entity to be embedded as a child element with localized
    /// per-frame invalidation.
    pub(super) playing_track: Option<PlayingTrackSnapshot>,
    /// Snapshot of [`ThemeColors`], updated on theme change. Copied
    /// (not borrowed) because `ThemeColors: Copy` and the parent's
    /// theme list shouldn't outlive theme reloads.
    pub(super) theme_colors: ThemeColors,
    /// Shared 10-band equalizer state. Cloned into every
    /// [`PlaybackController`] so the audio thread reads UI changes
    /// lock-free. The authoritative source of truth is `TempoApp`,
    /// which mutates this directly when the user adjusts a slider;
    /// the player merely holds a clone so the audio backend it
    /// constructs picks up the same handle.
    pub(super) eq_state: tempo::equalizer::EqState,
}

impl gpui::EventEmitter<PlayerEvent> for PlayerEntity {}

/// Snapshot of a [`Track`]'s render-relevant fields, pushed onto
/// [`PlayerEntity`] by `TempoApp` after each successful `play_track`
/// (or after library reload, theme change, etc.). The player's render
/// path consumes this snapshot directly so it doesn't need a borrow
/// of the parent's `tracks` `Vec` — that's the foundation that lets
/// `Entity<PlayerEntity>` be embedded as a child element and have its
/// per-frame ticks invalidate *only* the player bar.
///
/// All fields are cheap to clone:
/// - `SharedString` is `Arc<str>` (refcount bump).
/// - `PathBuf` is ~40 bytes.
/// - `TrackArtwork::Embedded(Arc<Image>)` is a refcount bump;
///   `TrackArtwork::File(PathBuf)` is also cheap.
/// - `album_initials` is the only owned `String` (1–2 chars).
///
/// The snapshot is updated wholesale (not field-by-field) on every
/// track change. Theme changes update `colors` independently via
/// [`PlayerEntity::set_theme_colors`].
#[derive(Clone)]
pub(crate) struct PlayingTrackSnapshot {
    pub(crate) path: PathBuf,
    pub(crate) title: SharedString,
    pub(crate) artist: SharedString,
    pub(crate) album: SharedString,
    pub(crate) year: SharedString,
    pub(crate) codec: SharedString,
    pub(crate) duration: SharedString,
    pub(crate) duration_value: Duration,
    pub(crate) bitrate: Option<u32>,
    pub(crate) artwork: Option<TrackArtwork>,
    pub(crate) album_initials: String,
    pub(crate) album_color: u32,
}

impl PlayingTrackSnapshot {
    /// Capture a snapshot from a `Track`. All fields are cheap clones
    /// (`SharedString`/`Arc`/short `String`s).
    pub(crate) fn from_track(track: &Track) -> Self {
        Self {
            path: track.path.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            year: track.year.clone(),
            codec: track.codec.clone(),
            duration: track.duration.clone(),
            duration_value: track.duration_value,
            bitrate: track.bitrate,
            artwork: track.artwork.clone(),
            album_initials: track.album_initials.clone(),
            album_color: track.album_color,
        }
    }
}

/// Outcome of a [`PlayerEntity::seek_from_waveform_click`] call,
/// describing what (if anything) the parent needs to do to recover.
#[must_use]
pub(crate) struct SeekClickOutcome {
    /// Position the player attempted to seek to. Always in
    /// `0..=track_duration`.
    pub(crate) target: Duration,
    /// `true` if the audio backend was empty (e.g. after a stop) so
    /// the seek had no effect. The parent should restart playback
    /// for the current track and then re-issue the seek.
    pub(crate) needs_restart: bool,
}

// ============================================================================
// Construction and lifecycle
// ============================================================================

impl PlayerEntity {
    /// Construct a new player entity. `initial_volume` and
    /// `initial_output_device` come from the persisted app state;
    /// `catalog` is cloned from `TempoApp` so the entity can read/write
    /// the waveform cache and play-count tables without an upcall.
    ///
    /// The audio backend ([`PlaybackController`]) is *not* opened
    /// synchronously — call [`PlayerEntity::start_deferred_init`] from
    /// `TempoApp::new` after the entity is in the entity map, so the
    /// 25–50ms cpal device-enumeration cost doesn't block the first
    /// frame.
    pub(crate) fn new(
        initial_volume: f32,
        initial_output_device: Option<String>,
        initial_visualizer: VisualizerKind,
        catalog: Option<CatalogStore>,
        theme_colors: ThemeColors,
        eq_state: tempo::equalizer::EqState,
        _cx: &mut Context<Self>,
    ) -> Self {
        let volume = initial_volume.clamp(0.0, 1.0);
        Self {
            playback: None,
            playback_status: "Initializing audio output...".to_string(),
            playing_track_path: None,
            is_playing: false,
            playback_mode: PlaybackMode::Straight,
            play_threshold_reached: false,
            volume,
            pre_mute_volume: if volume > 0.0 { volume } else { 1.0 },
            volume_dragging: false,
            volume_bar_scroll_handle: gpui::ScrollHandle::new(),
            waveform_seekbar_scroll_handle: gpui::ScrollHandle::new(),
            seekbar_menu_open: false,
            seekbar_menu_position: Point::default(),
            seekbar_fps_enabled: false,
            seekbar_fps_last_frame: None,
            seekbar_fps_smoothed: 0.0,
            seekbar_hovered: false,
            seekbar_hover_intensity: 0.0,
            seekbar_hover_last_sampled: None,
            seekbar_visualizer: initial_visualizer,
            band_smoothed: [0.0; tempo::audio_analyzer::BAND_COUNT],
            output_device: initial_output_device,
            output_menu_source: None,
            output_menu_position: Point::default(),
            now_playing_info_hovered: false,
            hovered_now_playing_link: None,
            alt_pressed: false,
            waveform_cache: HashMap::new(),
            waveform_loading: HashSet::new(),
            waveform_displayed_source: None,
            waveform_displayed_heights: None,
            waveform_morph_from: None,
            waveform_morph_started: None,
            catalog,
            playing_track: None,
            theme_colors,
            eq_state,
        }
    }

    /// Push the currently-playing track snapshot. Called by
    /// `TempoApp` after each successful `play_track` and after
    /// library reloads (so the new `tracks[0]` shows in the player
    /// bar). Setting to `None` reverts to the empty placeholder.
    pub(crate) fn set_playing_track(&mut self, snapshot: Option<PlayingTrackSnapshot>) {
        self.playing_track = snapshot;
    }

    /// Push the active theme colors. `ThemeColors: Copy` so this is
    /// a single 32-byte memcpy. Called from the parent's
    /// `set_theme` and at startup.
    pub(crate) fn set_theme_colors(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        if self.theme_colors != colors {
            self.theme_colors = colors;
            cx.notify();
        }
    }

    /// Initialize playback off the main startup path. Output device
    /// enumeration on cpal can take 25–50 ms, and rodio has to acquire
    /// the stream lock; doing it eagerly delays the first frame for no
    /// UI benefit (no track is playing yet). On systems without an
    /// audio device, the failure surfaces in the status bar a moment
    /// later instead of blocking the window.
    pub(crate) fn start_deferred_init(&self, cx: &mut Context<Self>) {
        let preferred_output = self.output_device.clone();
        let volume = self.volume;
        let eq_state = self.eq_state.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    perf::time_result("startup.playback_init_deferred", "", || {
                        PlaybackController::new(preferred_output.as_deref(), volume, eq_state)
                    })
                })
                .await;

            let _ = this.update(cx, |player, cx| match result {
                Ok(playback) => {
                    let device_label = playback.output_name().to_string();
                    player.playback = Some(playback);
                    player.output_device = Some(device_label);
                    player.playback_status = "Audio output ready".to_string();
                    cx.notify();
                }
                Err(error) => {
                    player.playback_status = format!("Playback unavailable: {error:#}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Start the 250ms playback heartbeat that drives waveform progress
    /// and triggers auto-advance. Only fires `cx.notify()` when the
    /// integer-second progress label changes (so a 4 Hz tick produces
    /// at most 1 Hz of repaints), and only emits
    /// [`PlayerEvent::TrackFinished`] when the audio backend reports
    /// empty queue.
    pub(crate) fn start_playback_tick(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Track the last broadcast position in whole seconds so we
            // only notify when the visible progress label actually
            // changes. The waveform highlight + progress bar update at
            // the same coarse granularity, so finer-grained ticks
            // produced no visible difference but forced the entire
            // root view to re-render four times a second on the
            // pre-Entity-split architecture.
            let mut last_emitted_seconds: i64 = -1;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;

                if this
                    .update(cx, |player, cx| {
                        if !player.is_playing {
                            return;
                        }

                        let playback_finished = player
                            .playback
                            .as_ref()
                            .is_some_and(|playback| playback.is_empty());

                        if playback_finished {
                            // Don't drive auto-advance in-place — emit
                            // an event and let the parent (which owns
                            // tabs + queue) decide what comes next.
                            // Track transitions always need a repaint;
                            // reset the throttle so the next render
                            // doesn't compare against a stale time.
                            if let Some(finished_path) = player.playing_track_path.clone() {
                                cx.emit(PlayerEvent::TrackFinished { finished_path });
                            }
                            last_emitted_seconds = -1;
                            cx.notify();
                            return;
                        }

                        let current_seconds = player
                            .playback
                            .as_ref()
                            .map(|playback| playback.position().as_secs() as i64)
                            .unwrap_or(0);

                        // Fire `PlayThresholdReached` exactly once per
                        // play, the first tick on which the playback
                        // position has advanced past 15 s. The flag is
                        // reset by `start_playback` / `stop` /
                        // `reset_for_library_reload`, so each fresh
                        // play has to earn its history entry. Using
                        // playback position (not wall clock) means
                        // pauses don't count toward the threshold but
                        // a deliberate forward seek does — both match
                        // the user-facing definition of "I listened to
                        // 15 seconds of this track".
                        if !player.play_threshold_reached
                            && current_seconds >= PLAY_THRESHOLD_SECS as i64
                            && let Some(path) = player.playing_track_path.clone()
                        {
                            player.play_threshold_reached = true;
                            cx.emit(PlayerEvent::PlayThresholdReached { path });
                        }

                        if current_seconds != last_emitted_seconds {
                            last_emitted_seconds = current_seconds;
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

    /// Replace the catalog handle when the library catalog is
    /// reopened (e.g. after a settings change that picks a different
    /// data dir). Waveforms are keyed by absolute path so the cache
    /// stays valid across catalog swaps; the handle is the only
    /// thing that needs replacing.
    ///
    /// Currently unused — `TempoApp` keeps the same catalog for the
    /// app's lifetime — but exposed for future "switch profile"
    /// flows in Settings.
    #[allow(dead_code)]
    pub(crate) fn replace_catalog(&mut self, catalog: Option<CatalogStore>) {
        self.catalog = catalog;
    }

    /// Reset transient state when the library is reloaded. Drops
    /// `playing_track_path` and stops playback so the player bar
    /// shows the empty state until the user picks a new track.
    pub(crate) fn reset_for_library_reload(&mut self, cx: &mut Context<Self>) {
        if let Some(playback) = &self.playback {
            playback.stop();
        }
        let was_playing = self.is_playing;
        let had_track = self.playing_track_path.is_some();
        self.is_playing = false;
        self.playing_track_path = None;
        self.play_threshold_reached = false;
        self.waveform_cache.clear();
        self.waveform_loading.clear();
        self.waveform_displayed_source = None;
        self.waveform_displayed_heights = None;
        self.waveform_morph_from = None;
        self.waveform_morph_started = None;
        if was_playing {
            cx.emit(PlayerEvent::IsPlayingChanged(false));
        }
        if had_track {
            cx.emit(PlayerEvent::PlayingTrackChanged { path: None });
        }
        cx.notify();
    }
}

// ============================================================================
// State queries (read-only; called from cross-region active-row checks
// and from the TempoApp render path)
// ============================================================================

impl PlayerEntity {
    pub(crate) fn is_playing(&self) -> bool {
        self.is_playing
    }

    /// Path of the track currently loaded in the audio backend, if
    /// any. Cross-region callers (table, history) compare against
    /// `track.path` to highlight the active row.
    pub(crate) fn playing_track_path(&self) -> Option<&Path> {
        self.playing_track_path.as_deref()
    }

    pub(crate) fn playback_mode(&self) -> PlaybackMode {
        self.playback_mode
    }

    pub(crate) fn volume(&self) -> f32 {
        self.volume
    }

    /// Persisted output-device label (may differ from the live device
    /// if the saved one was unplugged). Used by the state snapshot
    /// serializer.
    pub(crate) fn output_device(&self) -> Option<&str> {
        self.output_device.as_deref()
    }

    /// `true` while the user is mid-drag on the volume bar. The
    /// global mouse-move handler in `TempoApp::render` already
    /// receives a `bool` from `drag_volume(...)`; this query is
    /// retained for any future caller that needs to peek the drag
    /// state without taking the mutable borrow.
    #[allow(dead_code)]
    pub(crate) fn is_volume_dragging(&self) -> bool {
        self.volume_dragging
    }

    /// Whether the settings-anchored output device menu should be
    /// rendered as a child of root this frame.
    pub(crate) fn settings_output_menu_open(&self) -> bool {
        self.output_menu_source == Some(OutputMenuSource::Settings)
    }

    pub(crate) fn close_seekbar_menu(&mut self) -> bool {
        if !self.seekbar_menu_open {
            return false;
        }
        self.seekbar_menu_open = false;
        true
    }

    /// `Some` if the audio backend has been initialized successfully.
    /// Render code uses this to decide whether to render transport
    /// controls vs the "Initializing..." placeholder.
    pub(crate) fn has_playback(&self) -> bool {
        self.playback.is_some()
    }

    /// Free-form status string for the settings page / debug overlays.
    /// `playback_status_label()` is the canonical short label used in
    /// the player bar; this is the long-form ("Seeked to 1:23",
    /// "Playback unavailable: ...").
    #[allow(dead_code)]
    pub(crate) fn playback_status(&self) -> &str {
        &self.playback_status
    }

    pub(crate) fn playback_status_label(&self) -> &'static str {
        if self.playback.is_none() {
            "Unavailable"
        } else if self.is_playing {
            "Playing"
        } else {
            "Paused"
        }
    }

    pub(crate) fn current_output_label(&self) -> String {
        self.playback
            .as_ref()
            .map(|playback| playback.output_name().to_string())
            .or_else(|| self.output_device.clone())
            .unwrap_or_else(|| "No output device".to_string())
    }

    /// Live position of the audio backend, clamped to zero when
    /// nothing is loaded. Used by the player bar's progress label and
    /// waveform playhead.
    pub(crate) fn playback_position(&self) -> Duration {
        self.playback
            .as_ref()
            .filter(|playback| !playback.is_empty())
            .map(PlaybackController::position)
            .unwrap_or_default()
    }

    pub(crate) fn playback_mode_label(&self) -> &'static str {
        match self.playback_mode {
            PlaybackMode::Straight => "Straight play",
            PlaybackMode::Loop => "Loop",
            PlaybackMode::Shuffle => "Shuffle",
        }
    }
}

// ============================================================================
// Commands — state mutations triggered by UI / parent. Each one
// emits the right [`PlayerEvent`] before returning.
// ============================================================================

impl PlayerEntity {
    /// Load the file at `path` into the audio backend and start
    /// playback. The parent should call this from `play_track` *after*
    /// it has updated its own bookkeeping (play count, history).
    ///
    /// Returns `Ok(())` on success; on failure, `playback_status` is
    /// updated and `is_playing` is set to false.
    pub(crate) fn start_playback(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let start = Instant::now();
        let path_label = path.display().to_string();
        let prior_playing = self.is_playing;
        let prior_path = self.playing_track_path.as_deref().map(PathBuf::from);

        self.playing_track_path = Some(path.clone());
        // Each fresh play must earn its history entry: clear the
        // threshold flag so the tick will re-fire
        // `PlayThresholdReached` once this new playback crosses 15 s.
        // This applies even when re-playing the same path (loop, manual
        // restart) — replays are intentional and should each count once.
        self.play_threshold_reached = false;

        let Some(playback) = &self.playback else {
            self.is_playing = false;
            if prior_playing {
                cx.emit(PlayerEvent::IsPlayingChanged(false));
            }
            if prior_path.as_deref() != Some(path.as_path()) {
                cx.emit(PlayerEvent::PlayingTrackChanged {
                    path: Some(path.clone()),
                });
            }
            return Err("Audio output not initialized".to_string());
        };

        let result = match playback.play_path(&path) {
            Ok(()) => {
                self.is_playing = true;
                self.playback_status = "Playing".to_string();
                Ok(())
            }
            Err(error) => {
                self.is_playing = false;
                self.playback_status = format!("Playback failed: {error:#}");
                Err(format!("{error:#}"))
            }
        };

        if prior_path.as_deref() != Some(path.as_path()) {
            cx.emit(PlayerEvent::PlayingTrackChanged {
                path: Some(path.clone()),
            });
        }
        if self.is_playing != prior_playing {
            cx.emit(PlayerEvent::IsPlayingChanged(self.is_playing));
        }

        perf::log_duration(
            "player.start_playback",
            start.elapsed(),
            format!("path={path_label}"),
        );
        result
    }

    /// Pause/resume the current track. If nothing is loaded, this is
    /// a no-op (parent handles "nothing playing → play first track"
    /// in its own `toggle_playback`).
    pub(crate) fn pause(&mut self, cx: &mut Context<Self>) {
        if !self.is_playing {
            return;
        }
        if let Some(playback) = &self.playback {
            playback.pause();
        }
        self.is_playing = false;
        self.playback_status = "Playback paused".to_string();
        cx.emit(PlayerEvent::IsPlayingChanged(false));
    }

    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) -> bool {
        if self.is_playing {
            return false;
        }
        if self
            .playback
            .as_ref()
            .is_some_and(|playback| playback.is_empty())
        {
            // Backend has nothing loaded; parent should call
            // start_playback directly. Returning false signals "I
            // didn't resume; you must restart from a path."
            return false;
        }
        if let Some(playback) = &self.playback {
            playback.resume();
            self.is_playing = true;
            self.playback_status = "Playing".to_string();
            cx.emit(PlayerEvent::IsPlayingChanged(true));
        }
        true
    }

    /// Stop the audio backend without changing
    /// [`PlayerEntity::playing_track_path`]. After a stop, the next
    /// resume requires a fresh `start_playback` because rodio drops
    /// its source on stop.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(playback) = &self.playback {
            playback.stop();
        }
        if self.is_playing {
            self.is_playing = false;
            cx.emit(PlayerEvent::IsPlayingChanged(false));
        }
    }

    /// Seek to an absolute position. If the backend is empty (e.g.
    /// after a stop), returns `false` so the parent can re-seed via
    /// `start_playback`.
    pub(crate) fn seek(&mut self, position: Duration, cx: &mut Context<Self>) -> bool {
        let backend_empty = self
            .playback
            .as_ref()
            .is_some_and(|playback| playback.is_empty());
        if backend_empty {
            // Caller must restart playback from current path before
            // seeking.
            return false;
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
        cx.notify();
        true
    }

    /// Compute the seek target from a click on the waveform seekbar.
    /// `click_x` and `viewport_width` are in pixels. The caller passes
    /// `track_duration` because the player no longer holds the track
    /// list.
    ///
    /// Returns a [`SeekClickOutcome`] so the parent can recover from
    /// an empty-backend seek without a fragile post-hoc state check.
    pub(crate) fn seek_from_waveform_click(
        &mut self,
        ratio: f32,
        track_duration: Duration,
        cx: &mut Context<Self>,
    ) -> SeekClickOutcome {
        let target = track_duration.mul_f32(ratio.clamp(0.0, 1.0));

        let needs_restart = !self.seek(target, cx);
        SeekClickOutcome {
            target,
            needs_restart,
        }
    }

    pub(crate) fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0.0, 1.0);

        if self.volume > 0.0 {
            self.pre_mute_volume = self.volume;
        }

        if let Some(playback) = &self.playback {
            playback.set_volume(self.volume);
        }

        // Skip the persistence request while the user is mid-drag;
        // the background save thread already coalesces high-frequency
        // calls, but skipping the snapshot allocation entirely costs
        // nothing and shaves the per-frame work to almost zero.
        // `finish_volume_drag` saves once when the drag ends.
        if !self.volume_dragging {
            cx.emit(PlayerEvent::StateMutated);
        }
    }

    pub(crate) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if self.volume > 0.0 {
            self.pre_mute_volume = self.volume;
            self.set_volume(0.0, cx);
        } else {
            self.set_volume(self.pre_mute_volume.max(0.1), cx);
        }
    }

    pub(crate) fn set_max_volume(&mut self, cx: &mut Context<Self>) {
        self.set_volume(1.0, cx);
    }

    pub(crate) fn begin_volume_drag(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> Point<Pixels> {
        self.volume_dragging = true;
        let position = event.position;
        self.set_volume_from_mouse(position, cx);
        position
    }

    /// Returns `Some(position)` if the drag is in progress and the
    /// caller should display the volume tooltip at that point.
    /// Returns `None` if the drag wasn't active. The parent shows the
    /// tooltip itself because the tooltip overlay lives at root.
    pub(crate) fn drag_volume(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> Option<Point<Pixels>> {
        if !self.volume_dragging {
            return None;
        }

        if !event.dragging() {
            self.finish_volume_drag(cx);
            return None;
        }

        let position = event.position;
        self.set_volume_from_mouse(position, cx);
        Some(position)
    }

    /// Returns true if a drag was in progress and the parent should
    /// hide the volume tooltip + persist the final value.
    pub(crate) fn finish_volume_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.volume_dragging {
            return false;
        }
        self.volume_dragging = false;
        // Persist the final volume once the drag ends. While
        // dragging, `set_volume` deliberately skips the save request;
        // this catches up with one debounced write at drop.
        cx.emit(PlayerEvent::StateMutated);
        true
    }

    fn set_volume_from_mouse(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let volume = self.volume_from_x(position.x);
        self.set_volume(volume, cx);
    }

    fn volume_from_x(&self, x: Pixels) -> f32 {
        let bounds = self.volume_bar_scroll_handle.bounds();
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return self.volume;
        }
        ((f32::from(x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0)
    }

    /// Tooltip label for the volume drag overlay (`"Volume 87%"`).
    pub(crate) fn volume_tooltip_label(&self) -> SharedString {
        SharedString::from(format!("Volume {}%", (self.volume * 100.0).round() as u8))
    }

    pub(crate) fn cycle_playback_mode(&mut self, cx: &mut Context<Self>) {
        self.playback_mode = match self.playback_mode {
            PlaybackMode::Straight => PlaybackMode::Loop,
            PlaybackMode::Loop => PlaybackMode::Shuffle,
            PlaybackMode::Shuffle => PlaybackMode::Straight,
        };
        self.playback_status = format!("{} mode", self.playback_mode_label());
        cx.emit(PlayerEvent::StateMutated);
    }

    /// Switch the audio backend to a new output device by name. If
    /// the device is invalid or unavailable, falls back to creating a
    /// fresh `PlaybackController`. Caller (parent) is responsible for
    /// restarting playback if `was_playing` is true (the parent has
    /// the path resolution).
    ///
    /// Returns `Ok(was_playing_before)` on success so the parent
    /// knows whether to re-issue `start_playback`.
    pub(crate) fn select_output_device(
        &mut self,
        output_name: String,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        self.output_menu_source = None;
        let was_playing = self.is_playing;

        let result = if let Some(playback) = &mut self.playback {
            playback.set_output(&output_name, self.volume)
        } else {
            match PlaybackController::new(Some(&output_name), self.volume, self.eq_state.clone()) {
                Ok(playback) => {
                    self.playback = Some(playback);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        };

        match result {
            Ok(()) => {
                self.output_device = self
                    .playback
                    .as_ref()
                    .map(|playback| playback.output_name().to_string());
                self.playback_status = if was_playing {
                    "Playing".to_string()
                } else {
                    "Playback paused".to_string()
                };
                cx.emit(PlayerEvent::StateMutated);
                if was_playing {
                    // Audio backend reset — the rodio sink is empty.
                    // Parent must call start_playback to resume.
                    self.is_playing = false;
                    cx.emit(PlayerEvent::IsPlayingChanged(false));
                }
                Ok(was_playing)
            }
            Err(error) => {
                if self.is_playing {
                    self.is_playing = false;
                    cx.emit(PlayerEvent::IsPlayingChanged(false));
                }
                self.playback_status = format!("Playback unavailable: {error:#}");
                Err(format!("{error:#}"))
            }
        }
    }

    pub(crate) fn toggle_output_menu(
        &mut self,
        source: OutputMenuSource,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.output_menu_position = position;
        self.output_menu_source = if self.output_menu_source == Some(source) {
            None
        } else {
            Some(source)
        };
        cx.notify();
    }

    /// Close any open output device picker (player-bar or settings
    /// anchored). Currently unused — clicks outside the menu are
    /// handled by GPUI's overlay dismissal — but exposed so a future
    /// `Esc`-to-close action can call into it directly.
    #[allow(dead_code)]
    pub(crate) fn close_output_menu(&mut self, cx: &mut Context<Self>) {
        if self.output_menu_source.is_some() {
            self.output_menu_source = None;
            cx.notify();
        }
    }

    /// Read accessor for the player bar render path — which output
    /// menu is currently open, if any. Used by `TempoApp::render` to
    /// decide whether to render the *Settings*-anchored variant of
    /// the dropdown (the player-anchored variant is rendered inside
    /// `PlayerEntity::render` itself).
    #[allow(dead_code)]
    pub(crate) fn output_menu_source(&self) -> Option<OutputMenuSource> {
        self.output_menu_source
    }

    pub(crate) fn output_menu_position(&self) -> Point<Pixels> {
        self.output_menu_position
    }

    /// Update the alt-modifier mirror; called from
    /// `on_modifiers_changed` listeners on the player bar. Returns
    /// true if the alt-overlay needs to repaint.
    pub(crate) fn set_alt_pressed(&mut self, pressed: bool) -> bool {
        let needs_repaint = self.alt_pressed != pressed && self.now_playing_info_hovered;
        self.alt_pressed = pressed;
        needs_repaint
    }

    /// Now-playing info row hover begin/end. The current
    /// `window.modifiers().alt` is captured so the alt-overlay
    /// renders correctly without a separate ModifiersChanged event.
    pub(crate) fn set_now_playing_info_hovered(&mut self, hovered: bool, alt: bool) {
        self.now_playing_info_hovered = hovered;
        self.alt_pressed = alt;
    }

    pub(crate) fn toggle_seekbar_menu(&mut self, position: Point<Pixels>) {
        self.seekbar_menu_position = position;
        self.seekbar_menu_open = !self.seekbar_menu_open;
    }

    /// Pointer hover state setter for the seekbar surface. Returns
    /// true if the value changed -- the caller can skip a `notify`
    /// when the state is already correct (hover events fire on every
    /// frame the pointer is inside the bounds).
    pub(crate) fn set_seekbar_hovered(&mut self, hovered: bool) -> bool {
        if self.seekbar_hovered == hovered {
            return false;
        }
        self.seekbar_hovered = hovered;
        true
    }

    /// Advance and read the hover-fade intensity. Eases toward
    /// `seekbar_hovered ? 1.0 : 0.0` with a fixed time constant so
    /// the fade looks the same at any frame rate. Returns the eased
    /// value in `[0.0, 1.0]`. The render path is responsible for
    /// requesting another animation frame while this value is in
    /// transit (i.e. not equal to the target) so the easing actually
    /// progresses; a polled-only sampler would freeze mid-fade as
    /// soon as the steady-state animation gates close.
    pub(crate) fn sample_seekbar_hover_intensity(&mut self) -> f32 {
        let target: f32 = if self.seekbar_hovered { 1.0 } else { 0.0 };
        let now = Instant::now();
        let Some(previous) = self.seekbar_hover_last_sampled.replace(now) else {
            // First sample after a reset / startup: snap to target so
            // we don't see a fade-from-zero on the very first paint.
            self.seekbar_hover_intensity = target;
            return target;
        };
        let dt = now.duration_since(previous).as_secs_f32().min(0.1);
        // Time constant for the exponential ease. ~120 ms feels
        // responsive without being abrupt; matches what the OS uses
        // for tooltip fade.
        const TAU_SECS: f32 = 0.12;
        let alpha = 1.0 - (-dt / TAU_SECS).exp();
        self.seekbar_hover_intensity += (target - self.seekbar_hover_intensity) * alpha;
        // Snap when we're within a sub-pixel of the target so the
        // animation system can stop requesting frames.
        if (self.seekbar_hover_intensity - target).abs() < 0.005 {
            self.seekbar_hover_intensity = target;
        }
        self.seekbar_hover_intensity
    }

    pub(crate) fn toggle_seekbar_fps(&mut self) {
        self.seekbar_fps_enabled = !self.seekbar_fps_enabled;
        self.seekbar_fps_last_frame = None;
        self.seekbar_fps_smoothed = 0.0;
    }

    /// Currently selected seekbar visualizer.
    pub(crate) fn seekbar_visualizer(&self) -> VisualizerKind {
        self.seekbar_visualizer
    }

    /// Pick a visualizer. Emits [`PlayerEvent::StateMutated`] so the
    /// parent persists the choice. Resets the smoothed-band state so
    /// switching mid-track doesn't carry over momentum from the
    /// previous visualizer.
    pub(crate) fn set_seekbar_visualizer(&mut self, kind: VisualizerKind, cx: &mut Context<Self>) {
        if self.seekbar_visualizer == kind {
            return;
        }
        self.seekbar_visualizer = kind;
        self.band_smoothed = [0.0; tempo::audio_analyzer::BAND_COUNT];
        cx.emit(PlayerEvent::StateMutated);
        cx.notify();
    }

    /// Borrow a clone of the live audio analyzer, if the playback
    /// backend is up. Renderers call this on every paint when a
    /// frequency-reactive visualizer is active.
    pub(crate) fn audio_analyzer(&self) -> Option<tempo::audio_analyzer::AudioAnalyzer> {
        self.playback.as_ref().map(|p| p.analyzer())
    }

    pub(crate) fn sample_seekbar_fps(&mut self) -> f32 {
        let now = Instant::now();
        let Some(previous) = self.seekbar_fps_last_frame.replace(now) else {
            return self.seekbar_fps_smoothed;
        };

        let delta = now.duration_since(previous).as_secs_f32();
        if delta <= 0.0 {
            return self.seekbar_fps_smoothed;
        }

        // Ignore long gaps from toggling the overlay, tab stalls, or
        // debugger/build pauses. Those are not steady-state frame rate
        // and otherwise make the smoothed counter look stuck near 7 FPS.
        if delta > 0.25 {
            return self.seekbar_fps_smoothed;
        }

        let instant_fps = (1.0 / delta).clamp(0.0, 240.0);
        self.seekbar_fps_smoothed = if self.seekbar_fps_smoothed <= 0.0 {
            instant_fps
        } else {
            self.seekbar_fps_smoothed * 0.85 + instant_fps * 0.15
        };
        self.seekbar_fps_smoothed
    }

    /// Currently unused — `PlayerEntity::render` reads
    /// `self.now_playing_info_hovered` directly. Kept for
    /// inspector/debug callers.
    #[allow(dead_code)]
    pub(crate) fn now_playing_info_hovered(&self) -> bool {
        self.now_playing_info_hovered
    }

    /// Mirror of `window.modifiers().alt` captured at the last
    /// `on_modifiers_changed` event. The render path queries
    /// `window.modifiers().alt` directly (authoritative), so this
    /// accessor is unused today; retained for headless / inspector
    /// scenarios that don't have a `Window` in scope.
    #[allow(dead_code)]
    pub(crate) fn alt_pressed(&self) -> bool {
        self.alt_pressed
    }

    pub(crate) fn hovered_now_playing_link(&self) -> Option<NowPlayingLink> {
        self.hovered_now_playing_link
    }

    pub(crate) fn set_hovered_now_playing_link(&mut self, link: Option<NowPlayingLink>) {
        self.hovered_now_playing_link = link;
    }

    /// Emit a [`PlayerEvent::NowPlayingLinkClicked`] with the current
    /// playing-track path. The parent opens the appropriate tab. The
    /// click is dropped silently if no track is loaded, so the
    /// listener doesn't need to special-case the empty player bar.
    ///
    /// Today the player bar invokes the parent's `open_*_tab_for_track`
    /// directly because it has the `track_ix` in scope (it's
    /// rendering that row's tile). This method is the event-driven
    /// equivalent for any future caller (e.g. media keys, MPRIS
    /// "OpenUri") that doesn't have that index.
    #[allow(dead_code)]
    pub(crate) fn click_now_playing_link(&self, kind: NowPlayingLink, cx: &mut Context<Self>) {
        if let Some(path) = self.playing_track_path.clone() {
            cx.emit(PlayerEvent::NowPlayingLinkClicked { kind, path });
        }
    }
}

// ============================================================================
// Waveform cache (called from render path; produces sync data + may
// schedule async decode)
// ============================================================================

impl PlayerEntity {
    /// Look up the waveform peaks for `path`. On a hit, returns
    /// `(buffer, false)` — a refcount-bumped slice of the cached
    /// peaks. On a miss, schedules the decode on a background thread,
    /// returns a synthetic shimmer waveform with `loading=true` so
    /// the UI animates a placeholder until the decode completes, and
    /// emits `cx.notify()` from the spawned task on completion.
    ///
    /// The shimmer frame is regenerated on each call so animation
    /// during loading doesn't require keeping a `phase` cursor on
    /// `self`. Once the player bar moves to `with_animation` (Phase 3
    /// #17), this synchronous sampling can be replaced with an
    /// animation-driven phase.
    pub(crate) fn cached_waveform(
        &mut self,
        source: &WaveformSource,
        cx: &mut Context<Self>,
    ) -> (Arc<[f32]>, bool) {
        let start = Instant::now();
        let path = source.path.clone();

        if let Some(waveform) = self.waveform_cache.get(&path) {
            perf::log_duration_if_slow(
                "player.cached_waveform.hit",
                start.elapsed(),
                Duration::from_millis(2),
                format!("path={} segments={}", path.display(), waveform.len()),
            );
            // Refcount bump only — caller does not need to mutate the
            // buffer.
            return (Arc::clone(waveform), self.waveform_loading.contains(&path));
        }

        if !self.waveform_loading.contains(&path) {
            self.waveform_loading.insert(path.clone());
            let expected_path = path.clone();
            let source_owned = source.clone();
            let catalog = self.catalog.clone();
            perf::event(
                "player.waveform.request",
                format!("path={}", expected_path.display()),
            );
            cx.spawn(async move |this, cx| {
                let waveform: Arc<[f32]> = cx
                    .background_executor()
                    .spawn(async move {
                        let peaks = decode_or_load_waveform(&source_owned, catalog);
                        Arc::<[f32]>::from(peaks)
                    })
                    .await;

                let _ = this.update(cx, |player, cx| {
                    // Only insert if we're still expecting this path —
                    // if the user blew away the library while the
                    // decode was in flight, don't repopulate.
                    if player.waveform_loading.remove(&expected_path) {
                        player
                            .waveform_cache
                            .insert(expected_path.clone(), waveform);
                        cx.notify();
                    }
                });
            })
            .detach();
        }

        (
            Arc::<[f32]>::from(generate_loading_waveform(waveform_loading_phase())),
            true,
        )
    }

    /// Drop the cached waveform for a single path — used by the
    /// scanner when a file is replaced.
    pub(crate) fn invalidate_waveform_for_path(&mut self, path: &Path) {
        self.waveform_cache.remove(path);
        self.waveform_loading.remove(path);
    }

    /// Drop *all* cached waveforms and any in-flight loaders. Used
    /// when the catalog is replaced wholesale (library reload).
    pub(crate) fn clear_waveform_cache(&mut self) {
        self.waveform_cache.clear();
        self.waveform_loading.clear();
        // Cancel any in-flight morph; the new track will start its
        // own morph from the loading shimmer once render runs again.
        self.waveform_displayed_source = None;
        self.waveform_displayed_heights = None;
        self.waveform_morph_from = None;
        self.waveform_morph_started = None;
    }

    /// Resolve the heights to *paint* this frame, given the source
    /// buffer the cache returned. Drives the morph state machine:
    ///
    /// - If `source` is the same `Arc` as last frame and no morph
    ///   is in progress, return `source` directly (zero-copy).
    /// - If `source` differs from the last-painted source (track
    ///   changed, or the loading shimmer's per-frame `Arc` changed,
    ///   or the cache filled in real peaks), snapshot the heights
    ///   we last *painted* (so the user sees a smooth continuation
    ///   of their current visual state) and start a new morph
    ///   toward the new source.
    /// - While a morph is in progress, lerp from `morph_from` to
    ///   `source` per-bar with an ease-out curve. Once the morph
    ///   duration elapses, clear the morph state and return
    ///   `source` directly.
    ///
    /// Returns the heights to paint, plus a flag indicating whether
    /// a morph is still active (caller should keep `with_animation`
    /// wrapping the bars row to drive repaints until the morph
    /// finishes).
    ///
    /// # Buffer-length contract
    ///
    /// `source` and any stored morph state always have length
    /// `WAVEFORM_SEGMENTS`; the lerp assumes parity. Width-aware
    /// downsampling happens *after* the morph (in render), so the
    /// returned slice is always 360 floats.
    pub(super) fn resolve_waveform_heights(
        &mut self,
        source: Arc<[f32]>,
        loading: bool,
    ) -> (Arc<[f32]>, bool) {
        // While loading, the shimmer is *itself* the animation —
        // every frame `cached_waveform` returns a fresh `Arc` with
        // updated heights from the wall-clock phase. Trying to morph
        // between consecutive shimmer frames would freeze the bars
        // (we'd always be lerping `t≈0` back toward the previous
        // frame's heights). Just paint the shimmer source directly,
        // and remember it so the *exit* from loading (real peaks
        // arriving) triggers a morph from the user's most recent
        // shimmer state, not from blank.
        if loading {
            self.waveform_morph_from = None;
            self.waveform_morph_started = None;
            self.waveform_displayed_source = Some(Arc::clone(&source));
            self.waveform_displayed_heights = Some(Arc::clone(&source));
            return (source, false);
        }

        let source_changed = self
            .waveform_displayed_source
            .as_ref()
            .is_none_or(|prev| !Arc::ptr_eq(prev, &source));

        if source_changed {
            // The "from" is the heights we *painted last frame* —
            // not the previous source. If a morph was already in
            // progress, that's the lerp midpoint; if not, it's the
            // previous source itself. Either way, the user's eye
            // sees one continuous curve.
            //
            // First-paint case (`displayed_heights == None`): no
            // visual continuity to preserve, so skip the morph and
            // jump straight to the new source. Otherwise the very
            // first frame after app launch would morph from
            // height-zero to the target, which reads as a one-time
            // grow-in animation that doesn't match the user's
            // mental model ("the seekbar just appeared, with these
            // heights").
            if let Some(prev_heights) = self.waveform_displayed_heights.clone() {
                self.waveform_morph_from = Some(prev_heights);
                self.waveform_morph_started = Some(Instant::now());
            } else {
                self.waveform_morph_from = None;
                self.waveform_morph_started = None;
            }
            self.waveform_displayed_source = Some(Arc::clone(&source));
        }

        let morph_done = match (self.waveform_morph_started, &self.waveform_morph_from) {
            (Some(started), Some(_)) => started.elapsed() >= WAVEFORM_MORPH_DURATION,
            _ => true,
        };

        if morph_done {
            self.waveform_morph_from = None;
            self.waveform_morph_started = None;
            self.waveform_displayed_heights = Some(Arc::clone(&source));
            return (source, false);
        }

        // Active morph: lerp per-bar with cubic ease-out so the
        // motion accelerates fast and settles gently. `t` is in
        // `[0, 1)`; cubic ease-out is `1 - (1 - t)^3`.
        let from = self
            .waveform_morph_from
            .as_ref()
            .expect("morph_from set when morph_started is set");
        let started = self
            .waveform_morph_started
            .expect("morph_started set when morph_from is set");
        let raw_t = started.elapsed().as_secs_f32() / WAVEFORM_MORPH_DURATION.as_secs_f32();
        let t = raw_t.clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);

        // Length mismatch is unexpected (both are
        // `WAVEFORM_SEGMENTS`-sized) but defend against it: if
        // `from` is shorter, just paint the target (no lerp) for
        // the trailing bars.
        let n = source.len();
        let from_len = from.len();
        let lerped: Arc<[f32]> = (0..n)
            .map(|ix| {
                if ix < from_len {
                    from[ix] + (source[ix] - from[ix]) * eased
                } else {
                    source[ix]
                }
            })
            .collect::<Vec<f32>>()
            .into();

        self.waveform_displayed_heights = Some(Arc::clone(&lerped));
        (lerped, true)
    }
}

// ============================================================================
// Free functions: waveform decode pipeline
// (Free functions, not methods, because they don't touch entity state
// — they're invoked from the background executor.)
// ============================================================================

pub(super) fn decode_or_load_waveform(
    track: &WaveformSource,
    catalog: Option<CatalogStore>,
) -> Vec<f32> {
    let start = Instant::now();
    if let Some(catalog) = catalog.as_ref()
        && let Ok(Some(waveform)) =
            catalog.load_waveform(&track.path, WAVEFORM_SEGMENTS, WAVEFORM_CACHE_VERSION)
    {
        perf::log_duration(
            "player.waveform.load_or_generate",
            start.elapsed(),
            format!("source=cache path={}", track.path.display()),
        );
        return waveform;
    }

    let Some(waveform) = decode_waveform(track) else {
        let waveform = generate_fallback_waveform(track);
        perf::log_duration(
            "player.waveform.load_or_generate",
            start.elapsed(),
            format!("source=fallback path={}", track.path.display()),
        );
        return waveform;
    };

    if let Some(catalog) = catalog.as_ref() {
        let _ = catalog.save_waveform(
            &track.path,
            WAVEFORM_SEGMENTS,
            WAVEFORM_CACHE_VERSION,
            &waveform,
        );
    }

    perf::log_duration(
        "player.waveform.load_or_generate",
        start.elapsed(),
        format!("source=decode path={}", track.path.display()),
    );
    waveform
}

fn decode_waveform(track: &WaveformSource) -> Option<Vec<f32>> {
    let start = Instant::now();
    let waveform = decode_waveform_sampled(track).or_else(|| decode_waveform_full(track));
    perf::log_duration(
        "player.waveform.decode",
        start.elapsed(),
        format!(
            "path={} success={}",
            track.path.display(),
            waveform.is_some()
        ),
    );
    waveform
}

fn decode_waveform_sampled(track: &WaveformSource) -> Option<Vec<f32>> {
    let file = fs::File::open(&track.path).ok()?;
    let file_size = file.metadata().ok()?.len();
    let mut builder = Decoder::builder()
        .with_data(file)
        .with_byte_len(file_size)
        .with_seekable(true)
        .with_coarse_seek(true)
        .with_gapless(false);

    if let Some(extension) = track
        .path
        .extension()
        .and_then(|extension| extension.to_str())
    {
        builder = builder.with_hint(extension);
    }

    let mut decoder = builder.build().ok()?;
    let duration = decoder.total_duration().unwrap_or(track.duration_value);

    if duration < WAVEFORM_SAMPLED_MIN_DURATION {
        return None;
    }

    let sample_rate = decoder.sample_rate().get() as usize;
    let channels = decoder.channels().get() as usize;
    let total_samples = (duration.as_secs_f64() * sample_rate as f64 * channels as f64)
        .ceil()
        .max(1.0) as usize;
    let samples_per_bin = (total_samples / WAVEFORM_SEGMENTS).max(1);
    let sample_window = (samples_per_bin / 10).clamp(
        WAVEFORM_MIN_SAMPLE_FRAMES * channels,
        WAVEFORM_MAX_SAMPLE_FRAMES * channels,
    );
    let segment_seconds = duration.as_secs_f64() / WAVEFORM_SEGMENTS as f64;

    if !segment_seconds.is_finite() || segment_seconds <= 0.0 {
        return None;
    }

    let mut peaks = vec![0.0_f32; WAVEFORM_SEGMENTS];
    let mut saw_sample = false;

    for (segment, peak) in peaks.iter_mut().enumerate() {
        if segment > 0 {
            let target = Duration::from_secs_f64(segment_seconds * segment as f64);
            decoder.try_seek(target).ok()?;
        }

        for _ in 0..sample_window {
            let Some(sample) = decoder.next() else {
                break;
            };

            *peak = peak.max(sample.abs());
            saw_sample = true;
        }
    }

    saw_sample.then(|| normalize_waveform_peaks(peaks))
}

fn decode_waveform_full(track: &WaveformSource) -> Option<Vec<f32>> {
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

    Some(normalize_waveform_peaks(peaks))
}

fn normalize_waveform_peaks(peaks: Vec<f32>) -> Vec<f32> {
    let max_peak = peaks.iter().copied().fold(0.0_f32, f32::max).max(0.001);
    peaks
        .into_iter()
        .map(|peak| 8.0 + (peak / max_peak).sqrt() * 50.0)
        .collect()
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

pub(super) fn generate_loading_waveform(phase: f32) -> Vec<f32> {
    // `phase` is expected to be a small, bounded radian value
    // (see `waveform_loading_phase`). The two sinusoids combine
    // into a travelling wave so the *whole* seekbar visibly moves;
    // earlier versions kept the per-bar position scaled small
    // enough that aliasing in `sin()` (when `phase` was the raw
    // unbounded wall-clock millis / 90) collapsed everything past
    // the first column into incoherent noise. Coherence here
    // matters because the user reads the shimmer as "loading,
    // working" — frozen bars read as "stuck".
    (0..WAVEFORM_SEGMENTS)
        .map(|ix| {
            // 2.5 full wavelengths across the seekbar, sweeping
            // left → right at one cycle per `phase += 2π`.
            let position = ix as f32 / WAVEFORM_SEGMENTS as f32 * std::f32::consts::TAU * 2.5;
            let sweep = ((position - phase).sin() + 1.0) * 0.5;
            // Faster, finer ripple riding on top so individual
            // bars wiggle even when they're near the trough of
            // the main sweep.
            let ripple = ((position * 1.7 + phase * 1.6).sin() + 1.0) * 0.5;
            (10.0 + (sweep * 0.7 + ripple * 0.3) * 42.0).round()
        })
        .collect()
}

pub(super) fn waveform_loading_phase() -> f32 {
    // One full sweep every ~1.2s. The modulo keeps the phase
    // bounded in `[0, 2π)` regardless of how long the process
    // has been running — without it, `now_millis as f32 / 200.0`
    // grows to ~1e10 and wrecks `sin()` precision (every bar past
    // the first ends up snapping to a near-constant value because
    // floats can't represent the fractional radians distinct from
    // the integer-multiple-of-π part). Bounding the phase here
    // means `generate_loading_waveform` always sees a small,
    // well-conditioned angle.
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as f64)
        .unwrap_or_default();
    let cycle_ms = 1200.0_f64;
    let normalized = (millis % cycle_ms) / cycle_ms;
    (normalized as f32) * std::f32::consts::TAU
}
