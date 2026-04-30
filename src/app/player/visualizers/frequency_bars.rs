//! Frequency-bars visualizer — the classic spectrum-analyzer look.
//!
//! Lays out [`BAND_COUNT`] vertical bars across the seekbar's inner
//! content rect. Each bar's height tracks the magnitude of one
//! log-spaced frequency band as supplied by [`AnalysisFrame::bands`];
//! the analyzer has already applied a sqrt curve so the values map
//! roughly linearly onto pixel heights.
//!
//! Layout note: the seekbar's inner rect already has `.px_2()` (8px
//! each side) of padding applied by the caller, so `ctx.width` is the
//! *visible* width we get to paint into. Bar widths are computed from
//! that minus the inter-bar gap budget. The element returned here
//! fills the rect; its absolute positioning is supplied by the
//! seekbar wrapper.
//!
//! TODO: peak-hold caps. Would need a `bar_peaks: [f32; BAND_COUNT]`
//! field on `PlayerEntity` plus a small per-frame decay step driven
//! from `_cx`. Skipped to keep this file self-contained.

use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div, px, rgb};

use tempo::audio_analyzer::BAND_COUNT;

use super::super::entity::PlayerEntity;
use super::VisualizerContext;
use super::blend_color;

/// Pixel gap between adjacent bars. Keeps each band visually distinct
/// without losing too much width on narrow seekbars.
const BAR_GAP_PX: f32 = 1.0;

/// Minimum painted bar height. A row of zero-height divs collapses to
/// nothing and the visualizer looks broken on silent input; a tiny
/// floor keeps the baseline visible.
const MIN_BAR_HEIGHT_PX: f32 = 2.0;

/// Multiplier applied to bar heights when playback is paused. Keeps
/// the bars onscreen (so the user can see *which* visualizer is
/// active) while making it obvious nothing is moving.
const PAUSED_DAMPEN: f32 = 0.5;

/// Fallback width for the very first paint, before GPUI has reported
/// real bounds. Picked to look reasonable for a typical player-bar
/// seekbar; the next paint will use the real measurement.
const FALLBACK_WIDTH_PX: f32 = 480.0;

/// Per-frame ease-toward factor for `band_smoothed`. Slightly snappier
/// than the dancing-line value because bars look more responsive when
/// they track transients more aggressively.
const SMOOTHING_ALPHA: f32 = 0.45;

pub(super) fn render(ctx: VisualizerContext<'_>, _cx: &mut Context<PlayerEntity>) -> AnyElement {
    let VisualizerContext {
        frame,
        colors,
        width,
        height,
        is_playing,
        band_smoothed,
    } = ctx;

    // Ease the smoothed band buffer toward the latest analyzer
    // values. Owned by `PlayerEntity` so the smoothing survives
    // across `Render` calls -- without persistent state the bars
    // would snap on every analyzer cache flush, which jitters at
    // higher refresh rates.
    let damping = if is_playing { 1.0 } else { PAUSED_DAMPEN };
    for (i, target) in frame.bands.iter().copied().enumerate() {
        let prev = band_smoothed[i];
        band_smoothed[i] = prev + SMOOTHING_ALPHA * (target * damping - prev);
    }
    let smoothed = *band_smoothed;

    // First paint hands us width=0 because bounds aren't known yet;
    // fall back to a sensible guess so the bars don't all collapse to
    // 1px and pop on the next frame.
    let usable_width = if width > 0.0 {
        width
    } else {
        FALLBACK_WIDTH_PX
    };

    // Total inter-bar gap budget. `BAND_COUNT - 1` gaps for `BAND_COUNT`
    // bars (no leading/trailing gap — the seekbar's own padding handles
    // visual breathing room).
    let total_gap = BAR_GAP_PX * (BAND_COUNT - 1) as f32;
    let bar_width = ((usable_width - total_gap) / BAND_COUNT as f32).max(1.0);

    // Available vertical space for bar height. We deliberately use the
    // full inner-rect height; the seekbar's chrome (border, playhead)
    // is overlaid by the caller and won't be visually clipped.
    let max_bar_height = height.max(MIN_BAR_HEIGHT_PX);

    // Build the bar row. Flex with `.items_end()` so each bar grows
    // *upward* from the bottom edge of the rect. Explicit spacer
    // children (rather than `.justify_between()`) give us stable bar
    // widths that don't reflow as the seekbar resizes — every bar is
    // exactly `bar_width` wide and every gap is exactly `BAR_GAP_PX`.
    let mut row = div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .flex()
        .flex_row()
        .items_end();

    for (i, magnitude) in smoothed.iter().copied().enumerate() {
        // Position along the spectrum in `[0.0, 1.0]`. Used both for
        // the color gradient (low -> waveform_played, high -> accent)
        // and as a stable identity for the gap insertion below.
        let t = if BAND_COUNT > 1 {
            i as f32 / (BAND_COUNT - 1) as f32
        } else {
            0.0
        };

        // Magnitude is already sqrt-scaled in `[0,1]` upstream and
        // already pause-dampened above; project onto pixel height.
        let bar_h = (magnitude * max_bar_height).max(MIN_BAR_HEIGHT_PX);

        // Spectrum gradient: bass on the left in the cool waveform
        // played color, treble on the right in the warm accent. The
        // shared `blend_color` helper interpolates in sRGB, which is
        // good enough for a moving visualizer.
        let color = blend_color(colors.waveform_played, colors.accent, t);

        let bar = div()
            .w(px(bar_width))
            .h(px(bar_h))
            .rounded_sm()
            .bg(rgb(color));

        // Insert a fixed-width spacer before every bar except the
        // first. Using explicit spacers (rather than `.justify_between`
        // alone) keeps the bar widths stable across frames — `between`
        // would let the gap shrink to zero on narrow widths.
        if i > 0 {
            row = row.child(div().w(px(BAR_GAP_PX)).h_full());
        }
        row = row.child(bar);
    }

    row.into_any_element()
}
