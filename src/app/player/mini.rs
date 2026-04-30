//! Mini-player layout — a compact "now playing" surface that takes
//! over the entire window when the user toggles mini mode.
//!
//! Two layouts (toggled via the size button in the hover overlay):
//!
//! 1. **CompactBar** (360x100) — horizontal: thumbnail on the left,
//!    title/artist/album stacked on the right; controls overlay covers
//!    the right column on hover.
//! 2. **Square** — opens at a 400x400 default and is user-resizable
//!    while open; the album art stretches to fill the available area
//!    above a thin metadata strip; controls overlay fades in over the
//!    artwork on hover. The size is *not* remembered across cycles —
//!    re-entering Square always reopens at 400x400.
//!
//! ## Communication
//!
//! Click handlers emit the same [`super::PlayerEvent`] variants as the
//! full bar — `RequestPlayPause`, `RequestPlayPrev`, `RequestPlayNext`,
//! `RequestPlayRandom`, `RequestSeekFromWaveformClick { ratio }` — so
//! the parent's existing `handle_player_event` arms work unchanged.
//! Two new variants are emitted from inside the mini overlay:
//!
//! - [`super::PlayerEvent::RequestExitMini`] — return-to-full button.
//! - [`super::PlayerEvent::RequestCycleMiniSize`] — size cycle button.
//!
//! The mini player reuses the full bar's [`super::render::transport_overlay`]
//! and [`super::render::waveform_seekbar`] helpers so the controls look
//! and behave identically across modes.

use super::render::{
    render_marquee_text, transport_overlay, volume_speaker_icon, waveform_seekbar,
};
use super::*;
use entity::PlayingTrackSnapshot;

/// Height of the bottom metadata strip in the Square mini-player.
/// Lifted to module scope so `render_mini_player` can subtract it from
/// the window height when sizing the album-art region's aspect ratio
/// for the Cover/Contain heuristic.
const SQUARE_STRIP_HEIGHT: f32 = 58.0;

/// Tolerance for treating the Square mini-player's art region as
/// "still squarish enough" to fill (Cover) instead of letterbox
/// (Contain). 0.20 = ±20% from a 1:1 aspect, i.e. art region aspects
/// in roughly `0.833..=1.200` get the fill treatment.
const SQUARE_FILL_ASPECT_TOLERANCE: f32 = 0.20;

/// Render the mini player at the given size. Returns an [`AnyElement`]
/// covering the entire window.
pub(super) fn render_mini_player(
    player: &mut PlayerEntity,
    snapshot: &PlayingTrackSnapshot,
    size: MiniSize,
    window: &mut Window,
    cx: &mut Context<PlayerEntity>,
) -> AnyElement {
    let colors = player.theme_colors;
    let path = snapshot.path.clone();

    // Pull a fresh waveform/visualizer pair, exactly like the full bar
    // does, so the seekbar in the mini player is a proper copy of the
    // full visualizer instead of a flat progress line.
    let (waveform_source, waveform_loading) = player.cached_waveform_for_path(&path, cx);
    let (waveform, morph_active) =
        player.resolve_waveform_heights(waveform_source, waveform_loading);
    let playback_position = player.playback_position().min(snapshot.duration_value);
    let is_playing = player.is_playing;
    let volume = player.volume;
    let mode_icon = match player.playback_mode {
        PlaybackMode::Straight => "→",
        PlaybackMode::Loop => "↻",
        PlaybackMode::Shuffle => "⤨",
    };
    let mode_active = player.playback_mode != PlaybackMode::Straight;
    let playback_progress = if snapshot.duration_value.is_zero() {
        0.0
    } else {
        (playback_position.as_secs_f32() / snapshot.duration_value.as_secs_f32()).clamp(0.0, 1.0)
    };

    if player.seekbar_fps_enabled {
        window.request_animation_frame();
    }
    let seekbar_fps = if player.seekbar_fps_enabled {
        Some(player.sample_seekbar_fps())
    } else {
        None
    };

    let visualizer_kind = player.seekbar_visualizer;
    let analysis_frame = if matches!(visualizer_kind, VisualizerKind::Waveform) {
        None
    } else {
        window.request_animation_frame();
        Some(
            player
                .audio_analyzer()
                .map(|a| a.latest_frame())
                .unwrap_or_else(|| tempo::audio_analyzer::AnalysisFrame::silent(44_100)),
        )
    };
    let seekbar_hover_intensity = if matches!(visualizer_kind, VisualizerKind::Waveform) {
        0.0
    } else {
        let intensity = player.sample_seekbar_hover_intensity();
        let target: f32 = if player.seekbar_hovered { 1.0 } else { 0.0 };
        if (intensity - target).abs() > f32::EPSILON {
            window.request_animation_frame();
        }
        intensity
    };

    match size {
        MiniSize::CompactBar => render_compact_bar(
            player,
            snapshot,
            colors,
            is_playing,
            volume,
            mode_icon,
            mode_active,
            playback_progress,
            playback_position,
            waveform,
            waveform_loading,
            morph_active,
            seekbar_fps,
            visualizer_kind,
            analysis_frame,
            seekbar_hover_intensity,
            cx,
        ),
        MiniSize::Square => {
            // Decide Cover vs. Contain for the album art based on
            // how square the available art region is. The art region
            // is the window minus the metadata strip at the bottom;
            // if that rectangle's aspect is within
            // `SQUARE_FILL_ASPECT_TOLERANCE` of 1:1 we let the cover
            // fill (Cover, small crop) so the user doesn't see
            // black/theme bars; once the user resizes far enough off
            // square we revert to Contain so the whole cover is
            // visible (letterboxed). `bounds.size` is in logical
            // pixels — what we want for ratio math.
            let bounds_size = window.bounds().size;
            let art_w = f32::from(bounds_size.width);
            let art_h = (f32::from(bounds_size.height) - SQUARE_STRIP_HEIGHT).max(1.0);
            let art_aspect = art_w / art_h;
            let art_fit = if (art_aspect - 1.0).abs() <= SQUARE_FILL_ASPECT_TOLERANCE {
                ObjectFit::Cover
            } else {
                ObjectFit::Contain
            };
            render_square(
                player,
                snapshot,
                colors,
                is_playing,
                volume,
                mode_icon,
                mode_active,
                playback_progress,
                playback_position,
                waveform,
                waveform_loading,
                morph_active,
                seekbar_fps,
                visualizer_kind,
                analysis_frame,
                seekbar_hover_intensity,
                art_fit,
                cx,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_compact_bar(
    player: &mut PlayerEntity,
    snapshot: &PlayingTrackSnapshot,
    colors: ThemeColors,
    is_playing: bool,
    volume: f32,
    mode_icon: &'static str,
    mode_active: bool,
    playback_progress: f32,
    playback_position: Duration,
    waveform: Arc<[f32]>,
    waveform_loading: bool,
    morph_active: bool,
    seekbar_fps: Option<f32>,
    visualizer_kind: VisualizerKind,
    analysis_frame: Option<tempo::audio_analyzer::AnalysisFrame>,
    seekbar_hover_intensity: f32,
    cx: &mut Context<PlayerEntity>,
) -> AnyElement {
    let snap_thumb = snapshot.clone();
    let snap_title = snapshot.clone();
    let snap_artist = snapshot.clone();
    let snap_album = snapshot.clone();
    let waveform_handle = player.waveform_seekbar_scroll_handle.clone();

    div()
        .id("mini-player-compact")
        .size_full()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_3()
        .bg(rgb(colors.player))
        .child(
            // Album thumbnail on the left, sized to fit a 100px-tall
            // bar with a little vertical padding.
            div()
                .flex_none()
                .w(px(80.0))
                .h(px(80.0))
                .child(artwork::album_tile_with_hover_border(
                    &snap_thumb.as_track_view(),
                    80.0,
                    Some(colors.accent),
                    colors,
                )),
        )
        .child(
            // Right column: metadata stack + hover overlay covers it.
            div()
                .id("mini-info")
                .flex_1()
                .min_w_0()
                .h_full()
                .relative()
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.0))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(colors.text_strong))
                        .child(render_marquee_text(
                            snap_title.title.clone(),
                            SharedString::from(format!(
                                "mini-title-marquee-{}",
                                snap_title.path.display()
                            )),
                            240.0,
                            8.6,
                            colors.text_strong,
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(rgb(colors.text_muted))
                        .child(render_marquee_text(
                            snap_artist.artist.clone(),
                            SharedString::from(format!(
                                "mini-artist-marquee-{}",
                                snap_artist.path.display()
                            )),
                            240.0,
                            7.8,
                            colors.text_muted,
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(rgb(colors.text_faint))
                        .child(render_marquee_text(
                            snap_album.album.clone(),
                            SharedString::from(format!(
                                "mini-album-marquee-{}",
                                snap_album.path.display()
                            )),
                            240.0,
                            7.8,
                            colors.text_faint,
                        )),
                )
                .child(mini_overlay(
                    is_playing,
                    volume,
                    mode_icon,
                    mode_active,
                    playback_progress,
                    playback_position,
                    snapshot.duration.clone(),
                    waveform,
                    waveform_loading,
                    morph_active,
                    seekbar_fps,
                    visualizer_kind,
                    analysis_frame,
                    seekbar_hover_intensity,
                    waveform_handle,
                    &mut player.band_smoothed,
                    colors,
                    /* compact = */ true,
                    cx,
                )),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_square(
    player: &mut PlayerEntity,
    snapshot: &PlayingTrackSnapshot,
    colors: ThemeColors,
    is_playing: bool,
    volume: f32,
    mode_icon: &'static str,
    mode_active: bool,
    playback_progress: f32,
    playback_position: Duration,
    waveform: Arc<[f32]>,
    waveform_loading: bool,
    morph_active: bool,
    seekbar_fps: Option<f32>,
    visualizer_kind: VisualizerKind,
    analysis_frame: Option<tempo::audio_analyzer::AnalysisFrame>,
    seekbar_hover_intensity: f32,
    art_fit: ObjectFit,
    cx: &mut Context<PlayerEntity>,
) -> AnyElement {
    let waveform_handle = player.waveform_seekbar_scroll_handle.clone();
    // Strip height is fixed (see `SQUARE_STRIP_HEIGHT`) so it always
    // shows two lines of metadata. The artwork takes the remainder
    // of the window (whatever the user has dragged the window to).
    // `art_fit` is computed by the caller from the window's current
    // aspect: near-square art regions use `Cover` (the cover fills
    // the area, with a small crop) and farther-from-square regions
    // use `Contain` (the whole cover is visible, letterboxed).

    // Stretchy album-art element. Inlined here (rather than reusing
    // `artwork::album_tile_with_hover_border`, which forces a square
    // pixel size) so the cover scales with the available rectangle.
    // The outer flex container centers the image on both axes so
    // letterboxed (Contain) layouts pin the cover in the middle of
    // its region.
    let initials_for_fallback = snapshot.album_initials.clone();
    let album_color_for_fallback = snapshot.album_color;
    let art_element = match &snapshot.artwork {
        Some(TrackArtwork::Embedded(image)) => img(image.clone())
            .size_full()
            .object_fit(art_fit)
            .with_fallback(move || {
                artwork::album_tile_fallback(
                    initials_for_fallback.clone(),
                    album_color_for_fallback,
                    colors,
                )
            })
            .into_any_element(),
        Some(TrackArtwork::File(path)) => img(path.clone())
            .size_full()
            .object_fit(art_fit)
            .with_fallback(move || {
                artwork::album_tile_fallback(
                    initials_for_fallback.clone(),
                    album_color_for_fallback,
                    colors,
                )
            })
            .into_any_element(),
        None => artwork::album_tile_fallback(
            snapshot.album_initials.clone(),
            snapshot.album_color,
            colors,
        ),
    };

    div()
        .id("mini-player-square")
        .size_full()
        .flex()
        .flex_col()
        .bg(rgb(colors.player))
        .child(
            // Artwork region — fills all remaining vertical space
            // above the metadata strip. Hover anywhere here reveals
            // the controls overlay (which scrim-dims the art rather
            // than hiding it, so the cover stays the visual focus).
            div()
                .id("mini-art")
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .bg(rgb(colors.player))
                .child(
                    // Inner centering container. With
                    // `ObjectFit::Contain` the image preserves its
                    // aspect ratio inside this flex box, and the
                    // `items_center / justify_center` flex rules
                    // pin the (possibly letterboxed) cover to the
                    // exact center of the artwork region.
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(art_element),
                )
                .child(mini_overlay(
                    is_playing,
                    volume,
                    mode_icon,
                    mode_active,
                    playback_progress,
                    playback_position,
                    snapshot.duration.clone(),
                    waveform,
                    waveform_loading,
                    morph_active,
                    seekbar_fps,
                    visualizer_kind,
                    analysis_frame,
                    seekbar_hover_intensity,
                    waveform_handle,
                    &mut player.band_smoothed,
                    colors,
                    /* compact = */ false,
                    cx,
                )),
        )
        .child(
            // Bottom metadata strip. Marquee widths are deliberately
            // generous so they scroll on most window sizes; the
            // outer `overflow_hidden + text_ellipsis` clips to the
            // window's actual width.
            div()
                .flex_none()
                .h(px(SQUARE_STRIP_HEIGHT))
                .w_full()
                .px_3()
                .py_1()
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.0))
                .border_t_1()
                .border_color(rgb(colors.button_hover))
                .bg(rgb(colors.player))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(colors.text_strong))
                        .child(render_marquee_text(
                            snapshot.title.clone(),
                            SharedString::from(format!(
                                "mini-square-title-marquee-{}",
                                snapshot.path.display()
                            )),
                            320.0,
                            8.6,
                            colors.text_strong,
                        )),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(rgb(colors.text_muted))
                        .child(render_marquee_text(
                            SharedString::from(format!("{} — {}", snapshot.artist, snapshot.album)),
                            SharedString::from(format!(
                                "mini-square-artist-album-marquee-{}",
                                snapshot.path.display()
                            )),
                            320.0,
                            7.8,
                            colors.text_muted,
                        )),
                ),
        )
        .into_any_element()
}

/// Hover-revealed control overlay.
///
/// Sits absolutely positioned over the parent (the metadata column in
/// CompactBar mode, the artwork region in the squares). Opacity 0 by
/// default; `:hover` flips it to opacity 1 with a tinted scrim so the
/// underlying art/metadata visibly recedes while the controls are
/// active.
///
/// `compact = true` packs the controls more tightly to fit the
/// 360x100 bar.
#[allow(clippy::too_many_arguments)]
fn mini_overlay(
    is_playing: bool,
    volume: f32,
    mode_icon: &'static str,
    mode_active: bool,
    playback_progress: f32,
    playback_position: Duration,
    duration: SharedString,
    waveform: Arc<[f32]>,
    waveform_loading: bool,
    morph_active: bool,
    seekbar_fps: Option<f32>,
    visualizer_kind: VisualizerKind,
    analysis_frame: Option<tempo::audio_analyzer::AnalysisFrame>,
    seekbar_hover_intensity: f32,
    waveform_handle: gpui::ScrollHandle,
    band_smoothed: &mut [f32; tempo::audio_analyzer::BAND_COUNT],
    colors: ThemeColors,
    compact: bool,
    cx: &mut Context<PlayerEntity>,
) -> AnyElement {
    let volume_fill = PLAYER_VOLUME_BAR_W * volume;
    let elapsed = SharedString::from(super::super::format_duration(playback_position));

    // Outer wrapper: opacity 0 idle, opacity 1 on hover.
    //
    // - Compact bar: the overlay covers the metadata column, so it
    //   uses an opaque `colors.player` background; showing both the
    //   text underneath and the controls would be visually noisy.
    // - Square modes: the overlay sits over the album artwork and
    //   *must keep the art visible* per design feedback. We use a
    //   translucent dark scrim that dims the artwork just enough for
    //   the controls to read clearly without hiding the cover.
    let mut wrapper = div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .opacity(0.0)
        .hover(|this| this.opacity(1.0))
        .flex()
        .flex_col()
        .when(!compact, |this| this.gap_2())
        .when(compact, |this| this.gap_1())
        .items_center()
        .justify_center()
        .px_2()
        .py_1();
    if compact {
        wrapper = wrapper.bg(rgb(colors.player));
    } else {
        // ~55% black scrim. Dark enough that white-on-art controls
        // stay legible against bright covers, light enough that the
        // album art still shows through.
        wrapper = wrapper.bg(gpui::rgba(0x0000008c));
    }
    wrapper
        .child(
            // Top row: transport controls.
            transport_overlay(is_playing, mode_icon, mode_active, colors, cx),
        )
        .child(
            // Middle row: visualizer/seekbar (full waveform_seekbar
            // reused for visual parity with the full bar).
            div()
                .w_full()
                .h(if compact { px(28.0) } else { px(56.0) })
                .relative()
                .child(waveform_seekbar(
                    elapsed,
                    duration,
                    playback_progress,
                    waveform,
                    waveform_loading,
                    morph_active,
                    is_playing,
                    seekbar_fps,
                    visualizer_kind,
                    analysis_frame,
                    seekbar_hover_intensity,
                    band_smoothed,
                    colors,
                    waveform_handle,
                    cx,
                )),
        )
        .child(
            // Bottom row: volume + size-cycle + return-to-full.
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    // Volume cluster.
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("mini-volume-mute")
                                .cursor_pointer()
                                .active(|this| this.opacity(0.75))
                                .on_click(cx.listener(|player, _, _, cx| {
                                    player.toggle_mute(cx);
                                    cx.notify();
                                }))
                                .child(volume_speaker_icon(1, colors)),
                        )
                        .child(
                            div()
                                .id("mini-volume-bar")
                                .w(px(PLAYER_VOLUME_BAR_W))
                                .h(px(14.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|player, event: &MouseDownEvent, _w, cx| {
                                        player.begin_volume_drag(event, cx);
                                        cx.stop_propagation();
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    |player, event: &MouseMoveEvent, _w, cx| {
                                        if player.drag_volume(event, cx).is_some() {
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|player, _: &MouseUpEvent, _w, cx| {
                                        if player.finish_volume_drag(cx) {
                                            cx.stop_propagation();
                                        }
                                    }),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .h(px(3.0))
                                        .rounded_full()
                                        .bg(rgb(colors.text_faint))
                                        .child(
                                            div()
                                                .w(px(volume_fill))
                                                .h(px(3.0))
                                                .rounded_full()
                                                .bg(rgb(colors.text)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("mini-volume-max")
                                .cursor_pointer()
                                .active(|this| this.opacity(0.75))
                                .on_click(cx.listener(|player, _, _, cx| {
                                    player.set_max_volume(cx);
                                    cx.notify();
                                }))
                                .child(volume_speaker_icon(3, colors)),
                        ),
                )
                .child(
                    // Window controls cluster: size cycle + return to full.
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id("mini-size-cycle")
                                .cursor_pointer()
                                .px_1()
                                .py_1()
                                .rounded_sm()
                                .text_color(rgb(colors.text_muted))
                                .hover(move |this| {
                                    this.text_color(rgb(colors.text_strong))
                                        .bg(rgb(colors.button_hover))
                                })
                                .on_click(cx.listener(|_player, _, _, cx| {
                                    cx.emit(PlayerEvent::RequestCycleMiniSize);
                                }))
                                .child(size_cycle_icon(colors)),
                        )
                        .child(
                            div()
                                .id("mini-return-full")
                                .cursor_pointer()
                                .px_1()
                                .py_1()
                                .rounded_sm()
                                .text_color(rgb(colors.text_muted))
                                .hover(move |this| {
                                    this.text_color(rgb(colors.text_strong))
                                        .bg(rgb(colors.button_hover))
                                })
                                .on_click(cx.listener(|_player, _, _, cx| {
                                    cx.emit(PlayerEvent::RequestExitMini);
                                }))
                                .child(return_to_full_icon(colors)),
                        ),
                ),
        )
        .into_any_element()
}

/// Mini-toggle button rendered in the bottom-right of the now-playing
/// info column on the full player bar. Click emits
/// [`super::PlayerEvent::RequestEnterMini`].
///
/// Positioned with a negative `right` so the button slides into the
/// player bar's `gap_4` between the now-playing info column and the
/// seekbar — visually the button sits right against the seekbar's
/// left edge, well clear of the marquee text columns. The album row
/// reserves matching right-padding (see `render.rs`) so a long album
/// title can't marquee under the button.
pub(super) fn mini_toggle_button(
    colors: ThemeColors,
    cx: &mut Context<PlayerEntity>,
) -> AnyElement {
    div()
        .id("mini-player-toggle")
        .absolute()
        .bottom(px(0.0))
        .right(px(-14.0))
        .px_1()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .text_color(rgb(colors.text_faint))
        .hover(move |this| {
            this.text_color(rgb(colors.accent))
                .bg(rgb(colors.button_hover))
        })
        .on_click(cx.listener(|_player, _, _, cx| {
            cx.emit(PlayerEvent::RequestEnterMini);
        }))
        .child(mini_toggle_icon(colors))
        .into_any_element()
}

// ============================================================================
// Inline SVG glyphs.
//
// Each is a small 2D-line pictogram drawn in `text_muted`-tinted strokes.
// Built the same way as `volume_speaker_icon` in `render.rs`: produce
// SVG bytes, wrap in `Image::from_bytes`. Cheap (~16x16) and never
// theme-blocked because the stroke color is interpolated into the
// SVG source per call.
// ============================================================================

/// "Shrink to mini" glyph: a small square inside a larger square with
/// an arrow pointing into the smaller square. Reads as "compact this
/// down".
fn mini_toggle_icon(colors: ThemeColors) -> AnyElement {
    let color = format!("#{:06x}", colors.text_faint);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24">
            <rect x="3" y="3" width="18" height="14" rx="2" fill="none" stroke="{color}" stroke-width="1.6"/>
            <rect x="11" y="11" width="8" height="6" rx="1" fill="{color}"/>
        </svg>"#
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(18.0))
    .h(px(18.0))
    .into_any_element()
}

/// "Cycle size" glyph: three concentric brackets suggesting "next
/// size up / cycle". Used inside the mini overlay only.
fn size_cycle_icon(colors: ThemeColors) -> AnyElement {
    let color = format!("#{:06x}", colors.text_muted);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24">
            <rect x="9" y="9" width="6" height="6" fill="none" stroke="{color}" stroke-width="1.5"/>
            <rect x="6" y="6" width="12" height="12" fill="none" stroke="{color}" stroke-width="1.2" opacity="0.7"/>
            <rect x="3" y="3" width="18" height="18" fill="none" stroke="{color}" stroke-width="1.0" opacity="0.45"/>
        </svg>"#
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(16.0))
    .h(px(16.0))
    .into_any_element()
}

/// "Return to full" glyph: arrow expanding outward from a small box.
/// Used inside the mini overlay only.
fn return_to_full_icon(colors: ThemeColors) -> AnyElement {
    let color = format!("#{:06x}", colors.text_muted);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24">
            <path d="M4 14V20H10" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>
            <path d="M20 10V4H14" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>
            <path d="M4 20L10 14" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>
            <path d="M20 4L14 10" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>
        </svg>"#
    );
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(16.0))
    .h(px(16.0))
    .into_any_element()
}
