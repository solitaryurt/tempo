//! Reusable visualization primitives for the analytics dashboard
//! (and any future stats UI). Every helper here is a free function
//! that returns an `AnyElement` so callers can compose them inside
//! the existing GPUI render trees without taking on chart libraries
//! or dom mutation state.
//!
//! Design constraints:
//! - No external dependencies. Everything is built from `div()` and
//!   simple SVGs (used only where layout-with-divs would be painful,
//!   like the radial hour-of-day clock).
//! - Theme-aware: every component takes `ThemeColors` so colors stay
//!   consistent across light/dark themes.
//! - Stateless: charts render from data slices passed in. Callers
//!   own aggregation and caching.
//! - Stateful ids are added wherever GPUI requires them
//!   (`.id(...)`); IDs are namespaced by `id_prefix` so multiple
//!   charts of the same kind can coexist on a page without
//!   collisions.

use gpui::{
    AnyElement, Context, Image, ImageFormat, IntoElement, ParentElement, SharedString, Stateful,
    Styled, div, img, prelude::*, px, relative, rgb,
};
use std::sync::Arc;

use super::TempoApp;
use super::theme::ThemeColors;

/// Bundles the bits a chart primitive needs to attach tooltips to the
/// stateful sub-elements it produces. Threaded explicitly (rather than
/// via free-floating closures) so callers can keep using the same
/// `TempoApp::with_tooltip` machinery used everywhere else in the app
/// without primitives having to take a mutable closure.
pub(super) struct ChartCtx<'a, 'b> {
    pub(super) app: &'a TempoApp,
    pub(super) cx: &'a mut Context<'b, TempoApp>,
    /// Stable namespace prefix for `.id(...)` calls. Each tooltip-able
    /// sub-element appends a per-row index so multiple charts of the
    /// same kind can coexist on the page without ID collisions.
    pub(super) id_prefix: SharedString,
}

impl<'a, 'b> ChartCtx<'a, 'b> {
    fn wrap_tooltip(
        &mut self,
        element: Stateful<gpui::Div>,
        id_suffix: &str,
        label: SharedString,
    ) -> Stateful<gpui::Div> {
        let id = SharedString::from(format!("{}-{}", self.id_prefix, id_suffix));
        self.app.with_tooltip(element, id, label, self.cx)
    }
}

// ============================================================================
// Section panel + header
// ============================================================================

/// Outer card used for every analytics section. Provides the dark
/// surface, thin border, and consistent padding, matching the design
/// language used throughout Tempo.
pub(super) fn panel(colors: ThemeColors) -> gpui::Div {
    div()
        .border_1()
        .border_color(rgb(colors.border))
        .bg(rgb(colors.surface))
        .rounded_md()
        .overflow_hidden()
}

/// Section heading rendered in small all-caps with a faint label and a
/// right-aligned "code" / index. Mirrors the `LISTENING TIME 001`
/// style from the reference designs.
pub(super) fn section_header(
    title: &'static str,
    subtitle: Option<SharedString>,
    annotation: Option<SharedString>,
    colors: ThemeColors,
) -> AnyElement {
    let mut row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_4()
        .pt_3()
        .pb_2()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(colors.text_faint))
                        .child(title),
                )
                .when_some(subtitle, |this, subtitle| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(subtitle),
                    )
                }),
        );

    if let Some(annotation) = annotation {
        row = row.child(
            div()
                .text_xs()
                .text_color(rgb(colors.text_faint))
                .child(annotation),
        );
    }

    row.into_any_element()
}

// ============================================================================
// Horizontal bar list (ranked categories)
// ============================================================================

#[derive(Clone)]
pub(super) struct HBarRow {
    pub(super) label: SharedString,
    pub(super) value_label: SharedString,
    pub(super) ratio: f32,
    /// Optional per-row color override (defaults to theme accent).
    pub(super) color: Option<u32>,
    /// Tooltip text to show when the row is hovered. When `None` no
    /// tooltip is attached even if a `ChartCtx` is supplied.
    pub(super) tooltip: Option<SharedString>,
}

/// Vertical stack of labeled horizontal bars. Each row shows
/// `label .... value` above a thin progress bar whose width matches
/// `ratio` (0.0..=1.0). Reusable for genre, artist, file-format and
/// any other "top N categories" view.
///
/// Pass a `ChartCtx` to attach hover tooltips to each row that has a
/// non-`None` `HBarRow::tooltip`. When `ctx` is `None` rows render
/// without tooltips (and without the `.id(...)` call needed to make
/// them stateful).
pub(super) fn h_bar_list(
    rows: &[HBarRow],
    track_color: u32,
    bar_color: u32,
    colors: ThemeColors,
    ctx: Option<&mut ChartCtx<'_, '_>>,
) -> AnyElement {
    let mut container = div().px_4().pb_4().flex().flex_col().gap_3();

    if rows.is_empty() {
        return container
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child("No data"),
            )
            .into_any_element();
    }

    let mut ctx = ctx;

    for (ix, row) in rows.iter().enumerate() {
        let ratio = row.ratio.clamp(0.0, 1.0);
        let fg = row.color.unwrap_or(bar_color);

        let row_body = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(colors.text))
                            .child(row.label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child(row.value_label.clone()),
                    ),
            )
            .child(
                div()
                    .h(px(6.0))
                    .w_full()
                    .rounded_sm()
                    .bg(rgb(track_color))
                    .relative()
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(ratio))
                            .rounded_sm()
                            .bg(rgb(fg)),
                    ),
            );

        let row_el: AnyElement = match (ctx.as_mut(), row.tooltip.clone()) {
            (Some(ctx), Some(tooltip)) => {
                let stateful = row_body.id(SharedString::from(format!("hbar-row-{ix}")));
                ctx.wrap_tooltip(stateful, &format!("row-{ix}"), tooltip)
                    .into_any_element()
            }
            _ => row_body.into_any_element(),
        };

        container = container.child(row_el);
    }

    container.into_any_element()
}

// ============================================================================
// Stacked horizontal bar (e.g. file format split)
// ============================================================================

#[derive(Clone)]
pub(super) struct StackSegment {
    pub(super) label: SharedString,
    pub(super) value: f64,
    pub(super) color: u32,
    pub(super) tooltip: Option<SharedString>,
}

/// Single horizontal bar split into proportional segments. Used for
/// "by file count" / "by disk size" style breakdowns where the
/// total maps cleanly to a percentage strip.
///
/// `min_segment` is the minimum visible width (relative units) for a
/// non-zero segment so tiny categories don't collapse to invisibility.
///
/// Pass a `ChartCtx` and an `id_suffix` to enable per-segment
/// hover tooltips. The suffix lets multiple stacked bars in the same
/// panel share `ctx.id_prefix` without colliding.
pub(super) fn stacked_bar(
    segments: &[StackSegment],
    min_segment: f32,
    height: f32,
    colors: ThemeColors,
    ctx: Option<&mut ChartCtx<'_, '_>>,
    id_suffix: &str,
) -> AnyElement {
    let total: f64 = segments.iter().map(|segment| segment.value.max(0.0)).sum();
    if total <= 0.0 || segments.is_empty() {
        return div()
            .h(px(height))
            .w_full()
            .rounded_sm()
            .bg(rgb(colors.button))
            .into_any_element();
    }

    let mut container = div()
        .h(px(height))
        .w_full()
        .rounded_sm()
        .overflow_hidden()
        .flex()
        .bg(rgb(colors.button));

    let mut ctx = ctx;

    for (ix, segment) in segments.iter().enumerate() {
        let mut ratio = (segment.value.max(0.0) / total) as f32;
        if segment.value > 0.0 {
            ratio = ratio.max(min_segment);
        }
        if ratio <= 0.0 {
            continue;
        }

        let body = div()
            .h_full()
            .w(relative(ratio))
            .bg(rgb(segment.color))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(colors.album_tile_text))
            .overflow_hidden()
            .child(
                div()
                    .min_w_0()
                    .px_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(segment.label.clone()),
            );

        let cell: AnyElement = match (ctx.as_mut(), segment.tooltip.clone()) {
            (Some(ctx), Some(tooltip)) => {
                let stateful = body.id(SharedString::from(format!("stack-{id_suffix}-{ix}")));
                ctx.wrap_tooltip(stateful, &format!("{id_suffix}-{ix}"), tooltip)
                    .into_any_element()
            }
            _ => body.into_any_element(),
        };

        container = container.child(cell);
    }

    container.into_any_element()
}

// ============================================================================
// Legend
// ============================================================================

#[derive(Clone)]
pub(super) struct LegendItem {
    pub(super) label: SharedString,
    pub(super) value_label: SharedString,
    pub(super) color: u32,
}

/// Multi-column legend used beneath stacked bars / donuts. Wraps to
/// new rows automatically.
pub(super) fn legend_grid(items: &[LegendItem], colors: ThemeColors) -> AnyElement {
    let mut grid = div().flex().flex_wrap().gap_4().gap_y_2().px_4().pb_4();

    for item in items {
        grid = grid.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w(px(120.0))
                .child(
                    div()
                        .w(px(10.0))
                        .h(px(10.0))
                        .rounded_sm()
                        .bg(rgb(item.color)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors.text))
                        .child(item.label.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors.text_faint))
                        .child(item.value_label.clone()),
                ),
        );
    }

    grid.into_any_element()
}

// ============================================================================
// Vertical bar chart
// ============================================================================

#[derive(Clone)]
pub(super) struct VBar {
    pub(super) label: SharedString,
    pub(super) value: f64,
    pub(super) value_label: Option<SharedString>,
    pub(super) tooltip: Option<SharedString>,
}

/// Vertical bar chart with a fixed pixel height. Bars share the
/// available width minus inter-bar gaps. Useful for decade
/// histograms, bitrate distributions, weekly listening, etc.
///
/// When `ctx` is supplied each bar with a `VBar::tooltip` becomes a
/// stateful element with a hover tooltip. `id_suffix` lets multiple
/// bar charts in the same panel namespace their per-bar IDs.
pub(super) fn vertical_bar_chart(
    bars: &[VBar],
    height: f32,
    bar_color: u32,
    accent_max: bool,
    colors: ThemeColors,
    ctx: Option<&mut ChartCtx<'_, '_>>,
    id_suffix: &str,
) -> AnyElement {
    let max = bars
        .iter()
        .map(|bar| bar.value.max(0.0))
        .fold(0.0_f64, f64::max);

    let mut container = div().px_4().pb_4().flex().flex_col().gap_1().w_full();

    if bars.is_empty() || max <= 0.0 {
        return container
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .h(px(height))
                    .child("No data"),
            )
            .into_any_element();
    }

    let has_value_labels = bars.iter().any(|bar| bar.value_label.is_some());

    if has_value_labels {
        let mut value_row = div().flex().gap_1().w_full().pb_1();
        for bar in bars {
            value_row = value_row.child(
                div()
                    .flex_1()
                    .min_w(px(4.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(bar.value_label.clone().unwrap_or_default()),
            );
        }
        container = container.child(value_row);
    }

    let mut bars_row = div().flex().items_end().gap_1().h(px(height)).w_full();
    let mut ctx = ctx;
    for (ix, bar) in bars.iter().enumerate() {
        let ratio = (bar.value.max(0.0) / max) as f32;
        let is_max = accent_max && bar.value >= max;
        let fill = if is_max { colors.accent } else { bar_color };

        let bar_body = div()
            .flex_1()
            .min_w(px(4.0))
            .h_full()
            .flex()
            .flex_col()
            .justify_end()
            .child(
                div()
                    .w_full()
                    .h(relative(ratio.max(0.01)))
                    .rounded_sm()
                    .bg(rgb(fill)),
            );

        let bar_el: AnyElement = match (ctx.as_mut(), bar.tooltip.clone()) {
            (Some(ctx), Some(tooltip)) => {
                let stateful = bar_body.id(SharedString::from(format!("vbar-{id_suffix}-{ix}")));
                ctx.wrap_tooltip(stateful, &format!("{id_suffix}-{ix}"), tooltip)
                    .into_any_element()
            }
            _ => bar_body.into_any_element(),
        };

        bars_row = bars_row.child(bar_el);
    }

    let mut labels_row = div().flex().gap_1().w_full();
    for bar in bars {
        labels_row = labels_row.child(
            div()
                .flex_1()
                .min_w(px(4.0))
                .text_xs()
                .text_color(rgb(colors.text_faint))
                .overflow_hidden()
                .text_ellipsis()
                .child(bar.label.clone()),
        );
    }

    container
        .child(bars_row)
        .child(labels_row)
        .into_any_element()
}

// ============================================================================
// Heatmap (calendar-style)
// ============================================================================

#[derive(Clone)]
pub(super) struct HeatmapCell {
    pub(super) intensity: f32, // 0.0..=1.0
    /// Hover tooltip for the cell. When `None` the cell renders
    /// without an id / hover handler (avoids paying the per-cell
    /// statefulness cost for empty days when the caller doesn't want
    /// tooltips).
    pub(super) tooltip: Option<SharedString>,
    pub(super) is_empty: bool,
}

impl HeatmapCell {
    pub(super) fn empty() -> Self {
        Self {
            intensity: 0.0,
            tooltip: None,
            is_empty: true,
        }
    }
}

/// Generic heatmap rendered as a grid of small squares. `columns`
/// is the number of columns; cells flow column-major (typical of
/// calendar heatmaps where each column is a week and rows are
/// weekdays).
///
/// Rows must equal `cells.len() / columns` (callers are expected
/// to pad with `HeatmapCell::empty()` so the rectangle is full).
///
/// When `ctx` is supplied each cell with a non-`None`
/// `HeatmapCell::tooltip` becomes a stateful, hover-tooltip element.
/// `id_suffix` namespaces the per-cell IDs so multiple heatmaps in
/// the same panel can share `ctx.id_prefix` without colliding.
#[allow(clippy::too_many_arguments)]
pub(super) fn heatmap(
    cells: &[HeatmapCell],
    rows: usize,
    columns: usize,
    cell_size: f32,
    cell_gap: f32,
    base_color: u32,
    accent_color: u32,
    colors: ThemeColors,
    ctx: Option<&mut ChartCtx<'_, '_>>,
    id_suffix: &str,
) -> AnyElement {
    if cells.is_empty() || rows == 0 || columns == 0 {
        return div()
            .px_4()
            .pb_4()
            .text_xs()
            .text_color(rgb(colors.text_muted))
            .child("No data")
            .into_any_element();
    }

    let mut grid = div().px_4().pb_4().flex().flex_col().gap(px(cell_gap));
    let mut ctx = ctx;

    for row_ix in 0..rows {
        let mut row_el = div().flex().gap(px(cell_gap));
        for col_ix in 0..columns {
            let cell_ix = col_ix * rows + row_ix;
            let cell = cells.get(cell_ix);
            let bg = match cell {
                Some(cell) if cell.is_empty => colors.app,
                Some(cell) if cell.intensity <= 0.0 => base_color,
                Some(cell) => blend_color(base_color, accent_color, cell.intensity.clamp(0.0, 1.0)),
                None => colors.app,
            };
            let body = div()
                .w(px(cell_size))
                .h(px(cell_size))
                .rounded_sm()
                .bg(rgb(bg));

            let tooltip = cell.and_then(|c| c.tooltip.clone());
            let cell_el: AnyElement = match (ctx.as_mut(), tooltip) {
                (Some(ctx), Some(label)) => {
                    let id = format!("hm-{id_suffix}-{cell_ix}");
                    let stateful = body.id(SharedString::from(id.clone()));
                    ctx.wrap_tooltip(stateful, &id, label).into_any_element()
                }
                _ => body.into_any_element(),
            };
            row_el = row_el.child(cell_el);
        }
        grid = grid.child(row_el);
    }

    grid.into_any_element()
}

/// Linearly interpolate between two RGB colors. `t` is the blend
/// factor (0.0 = `from`, 1.0 = `to`).
pub(super) fn blend_color(from: u32, to: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let from_r = ((from >> 16) & 0xff) as f32;
    let from_g = ((from >> 8) & 0xff) as f32;
    let from_b = (from & 0xff) as f32;
    let to_r = ((to >> 16) & 0xff) as f32;
    let to_g = ((to >> 8) & 0xff) as f32;
    let to_b = (to & 0xff) as f32;

    let r = (from_r + (to_r - from_r) * t).round() as u32;
    let g = (from_g + (to_g - from_g) * t).round() as u32;
    let b = (from_b + (to_b - from_b) * t).round() as u32;

    (r << 16) | (g << 8) | b
}

// ============================================================================
// Donut chart (SVG)
// ============================================================================

#[derive(Clone)]
pub(super) struct DonutSlice {
    pub(super) value: f64,
    pub(super) color: u32,
    /// Reserved for future per-slice hover tooltips. Donut wedges are
    /// SVG-rendered today (no clip-path or transform support in GPUI),
    /// so per-slice hit zones are not yet wired through. Kept on the
    /// type so callers can pre-compute labels alongside slice data.
    #[allow(dead_code)]
    pub(super) tooltip: Option<SharedString>,
}

/// Donut chart rendered as a single inline SVG. SVG keeps the math
/// simple (we'd otherwise need to build N triangle fans by hand) and
/// fits Tempo's pattern of using SVGs for the few pie/clock-shaped
/// affordances (sidebar nav icons, hour-of-day clock).
pub(super) fn donut_svg(
    slices: &[DonutSlice],
    diameter: f32,
    thickness: f32,
    colors: ThemeColors,
) -> AnyElement {
    let total: f64 = slices.iter().map(|slice| slice.value.max(0.0)).sum();
    let radius = diameter / 2.0;
    let inner = radius - thickness;
    if inner <= 0.0 {
        return div().w(px(diameter)).h(px(diameter)).into_any_element();
    }

    if total <= 0.0 {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{diameter}" height="{diameter}" viewBox="0 0 {diameter} {diameter}">
<circle cx="{cx}" cy="{cx}" r="{r}" fill="none" stroke="#{empty:06x}" stroke-width="{stroke}"/>
</svg>"##,
            cx = radius,
            r = radius - thickness / 2.0,
            stroke = thickness,
            empty = colors.button,
        );
        let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
        return img(image)
            .w(px(diameter))
            .h(px(diameter))
            .into_any_element();
    }

    let mut paths = String::new();
    let mut start = -std::f64::consts::FRAC_PI_2; // 12 o'clock start
    for slice in slices {
        let portion = slice.value.max(0.0) / total;
        if portion <= 0.0 {
            continue;
        }
        let end = start + portion * std::f64::consts::TAU;
        // Full-circle slices need to be drawn as two arcs, otherwise
        // the SVG renderer collapses the path.
        if portion >= 0.999 {
            paths.push_str(&format!(
                r##"<circle cx="{cx}" cy="{cx}" r="{r}" fill="none" stroke="#{color:06x}" stroke-width="{stroke}"/>"##,
                cx = radius,
                r = radius - thickness / 2.0,
                stroke = thickness,
                color = slice.color,
            ));
            start = end;
            continue;
        }
        let large_arc = if (end - start) > std::f64::consts::PI {
            1
        } else {
            0
        };
        let r = radius - thickness / 2.0;
        let r_f64 = r as f64;
        let radius_f64 = radius as f64;
        let sx = radius_f64 + r_f64 * start.cos();
        let sy = radius_f64 + r_f64 * start.sin();
        let ex = radius_f64 + r_f64 * end.cos();
        let ey = radius_f64 + r_f64 * end.sin();
        paths.push_str(&format!(
            r##"<path d="M {sx:.3} {sy:.3} A {r:.3} {r:.3} 0 {large_arc} 1 {ex:.3} {ey:.3}" fill="none" stroke="#{color:06x}" stroke-width="{stroke}" stroke-linecap="butt"/>"##,
            stroke = thickness,
            color = slice.color,
        ));
        start = end;
    }

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{diameter}" height="{diameter}" viewBox="0 0 {diameter} {diameter}">{paths}</svg>"##,
    );
    let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
    img(image)
        .w(px(diameter))
        .h(px(diameter))
        .into_any_element()
}

// ============================================================================
// Hour-of-day radial chart (SVG)
// ============================================================================

/// Radial bar chart for a 24-hour distribution. Each bar's length
/// scales with the corresponding hour's value, drawn as a wedge from
/// the center outward. Mirrors the "HOUR OF DAY" widget in the
/// reference designs.
pub(super) fn radial_hours(values: &[f64; 24], diameter: f32, colors: ThemeColors) -> AnyElement {
    let radius = diameter / 2.0;
    let inner = radius * 0.45;
    let max = values.iter().cloned().fold(0.0_f64, f64::max);
    let mut paths = String::new();

    let max_label = (0..24)
        .max_by(|a, b| {
            values[*a]
                .partial_cmp(&values[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);

    for (hour, value) in values.iter().enumerate() {
        let portion = if max > 0.0 {
            (value / max).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let bar_radius = inner + (radius - inner) * portion;
        // 24 wedges, each spans 15 degrees. Start at 12 o'clock.
        let start_angle = (hour as f32) * 15.0_f32.to_radians() - std::f32::consts::FRAC_PI_2;
        let end_angle = start_angle + 14.0_f32.to_radians();
        let cx = radius;
        let cy = radius;
        let sx = cx + inner * start_angle.cos();
        let sy = cy + inner * start_angle.sin();
        let ex = cx + inner * end_angle.cos();
        let ey = cy + inner * end_angle.sin();
        let osx = cx + bar_radius * start_angle.cos();
        let osy = cy + bar_radius * start_angle.sin();
        let oex = cx + bar_radius * end_angle.cos();
        let oey = cy + bar_radius * end_angle.sin();
        let fill = if hour == max_label && max > 0.0 {
            colors.accent
        } else {
            colors.waveform_idle
        };
        paths.push_str(&format!(
            r##"<path d="M {sx:.2} {sy:.2} L {osx:.2} {osy:.2} A {br:.2} {br:.2} 0 0 1 {oex:.2} {oey:.2} L {ex:.2} {ey:.2} A {ir:.2} {ir:.2} 0 0 0 {sx:.2} {sy:.2} Z" fill="#{fill:06x}" stroke="#{stroke:06x}" stroke-width="0.4"/>"##,
            br = bar_radius,
            ir = inner,
            stroke = colors.app,
        ));
    }

    // Hour ticks at 0/6/12/18 on the inner ring for orientation.
    for (hour, label) in [(0_u32, "00"), (6, "06"), (12, "12"), (18, "18")] {
        let angle = (hour as f32) * 15.0_f32.to_radians() - std::f32::consts::FRAC_PI_2;
        let lx = radius + (radius - 4.0) * angle.cos();
        let ly = radius + (radius - 4.0) * angle.sin();
        paths.push_str(&format!(
            r##"<text x="{lx:.2}" y="{ly:.2}" fill="#{tc:06x}" font-size="9" text-anchor="middle" alignment-baseline="middle" font-family="monospace">{label}</text>"##,
            tc = colors.text_faint,
        ));
    }

    // Center "PEAK" label.
    if max > 0.0 {
        paths.push_str(&format!(
            r##"<text x="{cx:.2}" y="{cy_top:.2}" fill="#{tc:06x}" font-size="8" text-anchor="middle" font-family="monospace">PEAK</text>
<text x="{cx:.2}" y="{cy_bot:.2}" fill="#{ts:06x}" font-size="13" text-anchor="middle" font-family="monospace" font-weight="bold">{max_label:02}:00</text>"##,
            cx = radius,
            cy_top = radius - 4.0,
            cy_bot = radius + 10.0,
            tc = colors.text_faint,
            ts = colors.text_strong,
        ));
    }

    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{diameter}" height="{diameter}" viewBox="0 0 {diameter} {diameter}">{paths}</svg>"##,
    );
    let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
    img(image)
        .w(px(diameter))
        .h(px(diameter))
        .into_any_element()
}

// ============================================================================
// Sparkline / line chart (SVG)
// ============================================================================

/// Minimal line chart (sparkline) drawn as a single SVG polyline.
/// Used for "library growth" / cumulative-track-count widgets.
///
/// When `ctx` and `tooltips` are provided each data-point gets an
/// invisible vertical hit-zone overlay so hovering anywhere along
/// that column shows the tooltip for the corresponding sample.
/// `tooltips.len()` is expected to match `values.len()`; mismatches
/// silently render without per-point tooltips.
pub(super) fn sparkline(
    values: &[f64],
    width: f32,
    height: f32,
    colors: ThemeColors,
    ctx: Option<&mut ChartCtx<'_, '_>>,
    tooltips: &[Option<SharedString>],
    id_suffix: &str,
) -> AnyElement {
    if values.is_empty() {
        return div().w(px(width)).h(px(height)).into_any_element();
    }

    let max = values.iter().cloned().fold(f64::MIN, f64::max);
    let min = values.iter().cloned().fold(f64::MAX, f64::min);
    let range = (max - min).max(1.0);
    let last = values.len().saturating_sub(1).max(1);
    let pts: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(ix, value)| {
            let x = (ix as f32 / last as f32) * width;
            let y = height - (((value - min) / range) as f32 * height);
            format!("{x:.2},{y:.2}")
        })
        .collect();

    let polyline = pts.join(" ");
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">
<polyline fill="none" stroke="#{accent:06x}" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round" points="{polyline}"/>
</svg>"##,
        accent = colors.accent,
    );
    let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));

    let chart_image = img(image).w(px(width)).h(px(height)).into_any_element();

    // No ctx / mismatched tooltip count => return the bare image.
    let Some(ctx) = ctx else {
        return chart_image;
    };
    if tooltips.len() != values.len() {
        return chart_image;
    }

    // Overlay invisible column hit zones above the sparkline image.
    // Each column owns the horizontal slice closest to its sample so
    // hovering anywhere along that vertical strip shows the tooltip.
    let mut overlay = div().absolute().top_0().left_0().w_full().h_full().flex();
    for (ix, tooltip) in tooltips.iter().enumerate() {
        let mut zone = div()
            .h_full()
            .flex_1()
            .min_w(px(1.0))
            .id(SharedString::from(format!("spark-{id_suffix}-{ix}")));
        if let Some(tooltip) = tooltip.clone() {
            zone = ctx.wrap_tooltip(zone, &format!("{id_suffix}-{ix}"), tooltip);
        }
        overlay = overlay.child(zone);
    }

    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .child(chart_image)
        .child(overlay)
        .into_any_element()
}

// ============================================================================
// Ranked list (top N table)
// ============================================================================

#[derive(Clone)]
pub(super) struct RankedRow {
    pub(super) rank: usize,
    pub(super) primary: SharedString,
    pub(super) secondary: Option<SharedString>,
    pub(super) value_primary: SharedString,
    pub(super) value_secondary: Option<SharedString>,
    /// 0.0..=1.0; renders as a thin trailing bar matching the
    /// reference design's ranked tables.
    pub(super) ratio: f32,
    /// Optional hover-tooltip label. When `None` (or `ctx` is `None`
    /// in `ranked_list`), no tooltip is attached to the row.
    pub(super) tooltip: Option<SharedString>,
    /// Optional inline thumbnail rendered between the rank number and
    /// the primary text. Used by the analytics dashboard to surface
    /// album / artist artwork next to the corresponding entries.
    pub(super) image: Option<RowImage>,
}

/// Inline thumbnail descriptor for a `RankedRow`. Precomputed by the
/// caller so the chart primitive doesn't need to know about
/// [`super::Track`] or the rest of the artwork pipeline.
#[derive(Clone)]
pub(super) struct RowImage {
    /// Optional disk path or asset reference for the artwork. When
    /// `None` the fallback initials/color tile is used directly.
    pub(super) source: Option<RowImageSource>,
    pub(super) initials: SharedString,
    pub(super) color: u32,
    /// `true` for round/avatar-style art (artists), `false` for
    /// square album-style art.
    pub(super) round: bool,
}

#[derive(Clone)]
pub(super) enum RowImageSource {
    /// Filesystem path; rendered via gpui `img(path)`.
    File(std::path::PathBuf),
    /// Pre-decoded image (typically embedded artwork).
    Embedded(Arc<Image>),
}

pub(super) fn ranked_list(
    rows: &[RankedRow],
    column_headers: &[&'static str],
    colors: ThemeColors,
    ctx: Option<&mut ChartCtx<'_, '_>>,
    id_suffix: &str,
) -> AnyElement {
    let any_image = rows.iter().any(|row| row.image.is_some());
    let mut container = div().flex().flex_col().px_4().pb_4().gap_1();

    if !column_headers.is_empty() {
        let mut header = div()
            .flex()
            .items_center()
            .gap_3()
            .pb_2()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .text_xs()
            .text_color(rgb(colors.text_faint))
            .font_weight(gpui::FontWeight::BOLD)
            .child(div().w(px(28.0)).child(column_headers[0]));

        // Match the per-row image gutter so column headers stay
        // aligned with the row content beneath them.
        if any_image {
            header = header.child(div().w(px(28.0)).flex_none());
        }

        header = header.child(div().flex_1().min_w_0().child(if column_headers.len() > 1 {
            column_headers[1]
        } else {
            ""
        }));

        if column_headers.len() > 2 {
            header = header.child(div().w(px(70.0)).child(column_headers[2]));
        }
        if column_headers.len() > 3 {
            header = header.child(div().w(px(70.0)).child(column_headers[3]));
        }
        if column_headers.len() > 4 {
            header = header.child(div().w(px(40.0)).child(column_headers[4]));
        }
        container = container.child(header);
    }

    if rows.is_empty() {
        return container
            .child(
                div()
                    .py_3()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child("No data"),
            )
            .into_any_element();
    }

    let mut ctx = ctx;

    for (ix, row) in rows.iter().enumerate() {
        let mut line = div()
            .flex()
            .items_center()
            .gap_3()
            .py_1()
            .text_color(rgb(colors.text))
            .child(
                div()
                    .w(px(28.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(format!("{:02}", row.rank)),
            );

        if any_image {
            line = line.child(row_image_element(
                row.image.as_ref(),
                28.0,
                colors,
                SharedString::from(format!("ranked-{id_suffix}-img-{ix}")),
            ));
        }

        line = line
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
                            .text_color(rgb(colors.text_strong))
                            .child(row.primary.clone()),
                    )
                    .when_some(row.secondary.clone(), |this, secondary| {
                        this.child(
                            div()
                                .text_xs()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(rgb(colors.text_muted))
                                .child(secondary),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .text_color(rgb(colors.text))
                    .child(row.value_primary.clone()),
            );

        if let Some(value_secondary) = row.value_secondary.clone() {
            line = line.child(
                div()
                    .w(px(70.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(value_secondary),
            );
        }

        // Trailing strength bar.
        line = line.child(
            div()
                .w(px(40.0))
                .h(px(6.0))
                .rounded_sm()
                .bg(rgb(colors.button))
                .relative()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(relative(row.ratio.clamp(0.0, 1.0)))
                        .rounded_sm()
                        .bg(rgb(colors.accent)),
                ),
        );

        let row_el: AnyElement = match (ctx.as_mut(), row.tooltip.clone()) {
            (Some(ctx), Some(tooltip)) => {
                let stateful = line.id(SharedString::from(format!("ranked-{id_suffix}-{ix}")));
                ctx.wrap_tooltip(stateful, &format!("{id_suffix}-{ix}"), tooltip)
                    .into_any_element()
            }
            _ => line.into_any_element(),
        };

        container = container.child(row_el);
    }

    container.into_any_element()
}

/// Render a [`RowImage`] (album cover, artist photo, or the
/// initials/color fallback tile when no source is available). Kept
/// here in `charts.rs` rather than pulling in `super::artwork` so the
/// chart primitives stay decoupled from the rest of the app's
/// artwork pipeline.
fn row_image_element(
    image: Option<&RowImage>,
    size: f32,
    colors: ThemeColors,
    id: SharedString,
) -> AnyElement {
    let Some(image) = image else {
        // No image at all: emit an empty same-sized box so the
        // rank/title columns stay aligned with sibling rows that *do*
        // carry artwork.
        return div().w(px(size)).h(px(size)).flex_none().into_any_element();
    };

    let initials = image.initials.clone();
    let color = image.color;
    let round = image.round;

    let mut tile = div()
        .id(id)
        .w(px(size))
        .h(px(size))
        .flex_none()
        .border_1()
        .border_color(rgb(colors.border))
        .overflow_hidden();
    tile = if round {
        tile.rounded_full()
    } else {
        tile.rounded_sm()
    };

    let inner: AnyElement = match image.source.as_ref() {
        Some(RowImageSource::File(path)) => {
            let fb_initials = initials.clone();
            img(path.clone())
                .size_full()
                .object_fit(gpui::ObjectFit::Cover)
                .with_fallback(move || row_image_fallback(fb_initials.clone(), color, colors))
                .into_any_element()
        }
        Some(RowImageSource::Embedded(image)) => {
            let fb_initials = initials.clone();
            img(image.clone())
                .size_full()
                .object_fit(gpui::ObjectFit::Cover)
                .with_fallback(move || row_image_fallback(fb_initials.clone(), color, colors))
                .into_any_element()
        }
        None => row_image_fallback(initials, color, colors),
    };

    tile.child(inner).into_any_element()
}

fn row_image_fallback(initials: SharedString, color: u32, colors: ThemeColors) -> AnyElement {
    div()
        .size_full()
        .bg(rgb(color))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(colors.album_tile_text))
        .text_xs()
        .child(initials)
        .into_any_element()
}
