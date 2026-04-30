//! Frequency-reactive seekbar visualizers.
//!
//! Each public renderer in this module returns an element tree shaped
//! to drop in where the precomputed-peaks waveform usually sits inside
//! [`super::render::waveform_seekbar`]. The renderers do *not* paint
//! the seekbar's chrome (border, rounded corners, elapsed/duration
//! labels, click-to-seek surface, ✦ menu, playhead line, FPS overlay).
//! The caller wraps the returned element with that chrome so all four
//! visualizers stay visually consistent.
//!
//! ## Data flow
//!
//! 1. [`super::entity::PlayerEntity::audio_analyzer`] returns a clone
//!    of the live `AudioAnalyzer` when the playback backend is up.
//!    The `Render` impl polls `AudioAnalyzer::latest_frame` each
//!    repaint, producing an [`AnalysisFrame`].
//! 2. The render path assembles a [`VisualizerContext`] holding that
//!    frame plus the small bit of UI state each visualizer needs
//!    (scrolling history for the spectrogram, theme colors, painted
//!    width, etc.) and dispatches on [`super::VisualizerKind`].
//! 3. The visualizer returns an `AnyElement` filling the seekbar's
//!    inner content rect. Animation frames are requested by the
//!    render path; the visualizer doesn't have to.
//!
//! ## Why a sibling module rather than free functions in `render.rs`?
//!
//! Each visualizer has its own helpers (band smoothing, bar layout,
//! spectrogram coloring) that would clutter `render.rs`. Splitting
//! lets each one own its file-local state without leaking helpers
//! into the player-bar render namespace. The dispatch entrypoint
//! [`render`] is the only thing `render.rs` imports.

use gpui::{AnyElement, Context};

use tempo::audio_analyzer::AnalysisFrame;

use super::super::theme::ThemeColors;
use super::VisualizerKind;
use super::entity::PlayerEntity;

/// Per-frame inputs every visualizer renderer needs. Constructed by
/// the player-bar render path and handed to [`render`].
pub(super) struct VisualizerContext<'a> {
    pub frame: AnalysisFrame,
    pub colors: ThemeColors,
    /// Painted width of the seekbar inner content rect, in pixels.
    /// Renderers use this to choose bar/column counts. Zero on the
    /// first paint before bounds are known; renderers should fall
    /// back to a reasonable default.
    pub width: f32,
    /// Painted height of the seekbar inner content rect, in pixels.
    pub height: f32,
    /// Whether playback is currently advancing. Visualizers fade /
    /// freeze when paused so a stationary line/bar isn't mistaken
    /// for a stuck UI.
    pub is_playing: bool,
    /// Per-band smoothing state borrowed mutably from `PlayerEntity`.
    /// Visualizers that animate band magnitudes (dancing line,
    /// frequency bars) read the previous frame's smoothed values
    /// here, blend toward the new analyzer frame, and write back.
    /// Holding the buffer on the entity (rather than recomputing from
    /// scratch each frame) is what gives the line its momentum: the
    /// analyzer's 16 ms cache + a per-frame ease-toward keeps the
    /// motion frame-rate-independent and frees the visualizers from
    /// owning any state themselves.
    pub band_smoothed: &'a mut [f32; tempo::audio_analyzer::BAND_COUNT],
}

/// Dispatch on `kind` and return the visualizer's content element.
///
/// `Waveform` returns `None` -- the caller keeps the existing
/// precomputed-peaks rendering for that variant. The other variants
/// always return `Some(element)` even on silent input so the seekbar
/// doesn't visibly empty out between tracks.
pub(super) fn render(
    kind: VisualizerKind,
    ctx: VisualizerContext<'_>,
    cx: &mut Context<PlayerEntity>,
) -> Option<AnyElement> {
    match kind {
        VisualizerKind::Waveform => None,
        VisualizerKind::DancingLine => Some(dancing_line::render(ctx, cx)),
        VisualizerKind::FrequencyBars => Some(frequency_bars::render(ctx, cx)),
    }
}

/// Linear interpolation between two `0xRRGGBB` colors. Shared by all
/// three visualizers; defined here so each implementation file can
/// stay focused on its own DSP/layout.
pub(super) fn blend_color(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ar = ((a >> 16) & 0xff) as f32;
    let ag = ((a >> 8) & 0xff) as f32;
    let ab = (a & 0xff) as f32;
    let br = ((b >> 16) & 0xff) as f32;
    let bg = ((b >> 8) & 0xff) as f32;
    let bb = (b & 0xff) as f32;
    let r = (ar + (br - ar) * t).round() as u32;
    let g = (ag + (bg - ag) * t).round() as u32;
    let bl = (ab + (bb - ab) * t).round() as u32;
    (r << 16) | (g << 8) | bl
}

mod dancing_line;
mod frequency_bars;
