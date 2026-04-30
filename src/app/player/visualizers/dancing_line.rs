//! Dancing-line visualizer.
//!
//! Renders a smooth curve through one control point per log-spaced
//! frequency band: the x position is the band's slot along the
//! seekbar (bass left, treble right) and the y position is the
//! band's smoothed magnitude. The curve is drawn as a row of small
//! horizontal slats, each slat's vertical offset interpolated from
//! the two control points it sits between via Catmull-Rom-style
//! tangents so the line reads as a single flowing arc rather than
//! 32 discrete pegs.
//!
//! This is the "spectrum line" interpretation of a music-reactive
//! line: when a kick fires the leftmost section of the line lifts,
//! when a hat fires the rightmost section lifts, and sustained
//! tonal content lifts a narrow region in the middle. The shape
//! you see *is* the spectrum.

use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div, px, rgb};

use tempo::audio_analyzer::BAND_COUNT;

use super::super::entity::PlayerEntity;
use super::VisualizerContext;
use super::blend_color;

/// Default painted width to fall back on when the seekbar bounds
/// haven't been measured yet (first paint of a freshly opened
/// player).
const FALLBACK_WIDTH: f32 = 480.0;

/// Painted slats per pixel of seekbar width. Slats are 1 px wide and
/// butt up against each other (`slat_w + 1.0`) so the curve reads as
/// continuous; clamped at MIN/MAX so very narrow windows still get a
/// usable line and very wide windows don't blow up the element budget.
const TARGET_SLAT_PX: f32 = 2.0;
const MIN_SLATS: usize = 64;
const MAX_SLATS: usize = 480;

/// Vertical headroom kept clear above/below the line so the strokes
/// don't clip into the rounded-corner border. Subtracted from the
/// available half-height before applying band magnitudes.
const VERTICAL_PADDING_PX: f32 = 4.0;

/// Per-slat stroke height. Small values look anaemic on idle audio;
/// 2px reads as a clean line at any energy level.
const STROKE_HEIGHT_PX: f32 = 2.0;

/// Multiplier applied to the curve's amplitude when paused. The line
/// stays visible (still anchored on the band magnitudes that haven't
/// changed since pause) but the live shape is dampened so a paused
/// state is visually distinct from quiet playback.
const PAUSED_DAMPING: f32 = 0.4;

/// Smoothing factor applied per render frame: `new = old + alpha *
/// (target - old)`. Bigger = snappier (closer to raw analyzer
/// values), smaller = lazier (more momentum). 0.35 lands at "fast
/// enough to feel reactive but smooth enough to not jitter".
const SMOOTHING_ALPHA: f32 = 0.35;

/// Floor on the per-band height as a fraction of `max_amplitude`. Keeps
/// silent bands from collapsing fully to the centre line, which would
/// look like the visualizer froze. Tiny but non-zero.
const QUIET_FLOOR: f32 = 0.06;

pub(super) fn render(ctx: VisualizerContext<'_>, _cx: &mut Context<PlayerEntity>) -> AnyElement {
    let VisualizerContext {
        frame,
        colors,
        width,
        height,
        is_playing,
        band_smoothed,
    } = ctx;

    let painted_width = if width > 0.0 { width } else { FALLBACK_WIDTH };
    let painted_height = if height > 0.0 { height } else { 56.0 };

    let center_y = painted_height * 0.5;
    let max_amplitude = (center_y - VERTICAL_PADDING_PX).max(1.0);

    // Ease the smoothed band buffer toward the latest analyzer
    // values. The buffer lives on `PlayerEntity` so the previous
    // frame's smoothed state survives across `Render` calls -- the
    // visualizer never sees a "snapped" value, even when the
    // analyzer's 16 ms cache returns a sharply different frame.
    let damping = if is_playing { 1.0 } else { PAUSED_DAMPING };
    for (i, target) in frame.bands.iter().copied().enumerate() {
        let prev = band_smoothed[i];
        band_smoothed[i] = prev + SMOOTHING_ALPHA * (target * damping - prev);
    }
    let smoothed = *band_smoothed;

    let slat_count = ((painted_width / TARGET_SLAT_PX) as usize).clamp(MIN_SLATS, MAX_SLATS);
    let slat_w = painted_width / slat_count as f32;

    // For each slat, find which two band control points it sits
    // between and interpolate. The bands occupy positions
    // `(i + 0.5) / BAND_COUNT` along [0, 1] -- i.e. centred on each
    // logical slot -- so the leftmost slat sits *left* of the first
    // band's centre and uses an extrapolation toward an implicit
    // "band -1" with magnitude equal to band 0.
    let line_color = blend_color(
        colors.waveform_played,
        colors.waveform_played_peak,
        frame.rms_normalized().max(0.2),
    );
    let glow_color = blend_color(colors.waveform_bg, colors.accent_soft, 0.7);

    let mut elements: Vec<gpui::AnyElement> = Vec::with_capacity(slat_count * 2);

    for i in 0..slat_count {
        let t = (i as f32 + 0.5) / slat_count as f32;
        // Position along the band axis in `[0, BAND_COUNT-1]`. Bands
        // are centred at `idx + 0.5` in their own logical space, so
        // multiplying `t` by `BAND_COUNT` and subtracting the half
        // offset gives a fractional band index that correctly aligns
        // the curve's endpoints with the first / last band centres.
        let band_position = (t * BAND_COUNT as f32 - 0.5).clamp(0.0, (BAND_COUNT - 1) as f32);
        let i0 = band_position.floor() as usize;
        let i1 = (i0 + 1).min(BAND_COUNT - 1);
        let frac = band_position - i0 as f32;

        // Catmull-Rom-ish tangent-aware interpolation with two
        // neighbours on each side. Falls back to plain linear at
        // the boundaries where neighbours don't exist; the visible
        // difference vs. plain lerp is mainly in mid-frequency
        // sustained content where the curve gets a satisfying
        // arch instead of straight chord segments.
        let m0 = smoothed[i0];
        let m1 = smoothed[i1];
        let m_prev = if i0 > 0 { smoothed[i0 - 1] } else { m0 };
        let m_next = if i1 + 1 < BAND_COUNT {
            smoothed[i1 + 1]
        } else {
            m1
        };
        let t2 = frac * frac;
        let t3 = t2 * frac;
        // Catmull-Rom basis (tension = 0.5).
        let height_norm = 0.5
            * ((2.0 * m0)
                + (-m_prev + m1) * frac
                + (2.0 * m_prev - 5.0 * m0 + 4.0 * m1 - m_next) * t2
                + (-m_prev + 3.0 * m0 - 3.0 * m1 + m_next) * t3);
        // Raise the curve so a positive band magnitude pushes the
        // line *up* (visually intuitive: more energy = bigger spike).
        // Floor it so silent bands don't disappear into the centre.
        let height_norm = (height_norm.max(QUIET_FLOOR)).min(1.0);
        let offset_px = -height_norm * max_amplitude;
        let stroke_top = (center_y + offset_px - STROKE_HEIGHT_PX * 0.5).max(0.0);

        // Soft glow stroke whose thickness grows with the per-band
        // magnitude rather than the global RMS, so a single loud
        // band lights up its segment of the line specifically.
        let glow_h = STROKE_HEIGHT_PX + 4.0 * height_norm * damping;
        let glow_top = (center_y + offset_px - glow_h * 0.5).max(0.0);

        let x = i as f32 * slat_w;
        elements.push(
            div()
                .absolute()
                .left(px(x))
                .top(px(glow_top))
                .w(px(slat_w + 1.0))
                .h(px(glow_h))
                .opacity(0.30 * damping.max(0.5))
                .bg(rgb(glow_color))
                .into_any_element(),
        );
        elements.push(
            div()
                .absolute()
                .left(px(x))
                .top(px(stroke_top))
                .w(px(slat_w + 1.0))
                .h(px(STROKE_HEIGHT_PX))
                .bg(rgb(line_color))
                .into_any_element(),
        );
    }

    div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .bg(rgb(colors.waveform_bg))
        .children(elements)
        .into_any_element()
}
