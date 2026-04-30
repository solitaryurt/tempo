//! [`PlayerEntity`]'s `Render` impl + the player-bar layout helpers it
//! uses.
//!
//! Splitting out into its own file because the bar's layout is the
//! single largest piece of code in the player module (~500 LOC of UI
//! that's well-isolated from state-mutation logic). Keeping
//! `entity.rs` focused on state + commands and `mod.rs` focused on
//! `TempoApp` orchestration glue makes the boundaries readable at a
//! glance.
//!
//! ## Communication out of the render path
//!
//! Click handlers fall into three groups:
//!
//! 1. **Local mutations** (volume drag, mute, output menu toggle,
//!    Now-Playing hover, mode cycle, max-volume) — call methods on
//!    `&mut PlayerEntity` directly inside `cx.listener`.
//! 2. **Cross-region requests** (transport prev/next/play-pause/random,
//!    waveform seek, tab navigation from Now-Playing labels, output
//!    device selection) — emit a [`PlayerEvent::Request*`] /
//!    `NowPlayingLinkClicked` variant; the parent's
//!    `handle_player_event` does the rest.
//! 3. **No state change, just notify** — modifier-key changes for the
//!    alt overlay use `cx.notify()` directly on the player.

use super::*;
use entity::PlayingTrackSnapshot;

impl Render for PlayerEntity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _frame_span = perf::span(
            "render.player_frame",
            format!(
                "is_playing={} loading={}",
                self.is_playing,
                self.playing_track
                    .as_ref()
                    .is_some_and(|s| self.waveform_loading.contains(&s.path))
            ),
        );

        let colors = self.theme_colors;
        // The parent doesn't embed us when `tracks.is_empty()`, so a
        // missing snapshot here is unexpected — render a defensive
        // empty bar (matching the parent's empty placeholder) rather
        // than panicking.
        let Some(snapshot) = self.playing_track.clone() else {
            return empty_player_bar(colors);
        };

        let path = snapshot.path.clone();
        let (waveform_source, waveform_loading) = self.cached_waveform_for_path(&path, cx);
        // Drive the per-column morph: when the cache hands us a
        // different `Arc` than last frame (track changed, shimmer
        // → real peaks, etc.), this lerps from the heights we
        // *painted* last frame toward the new source over
        // `WAVEFORM_MORPH_DURATION`. While `morph_active` is true,
        // the bars row needs `with_animation` to keep repainting.
        let (waveform, morph_active) =
            self.resolve_waveform_heights(waveform_source, waveform_loading);
        let playback_position = self.playback_position().min(snapshot.duration_value);
        let is_playing = self.is_playing;
        let now_playing_info_hovered = self.now_playing_info_hovered;
        let hovered_link = self.hovered_now_playing_link;
        let playback_status_label = self.playback_status_label();
        let volume = self.volume;
        let output_menu_open_in_player = self.output_menu_source == Some(OutputMenuSource::Player);
        let mode_icon = match self.playback_mode {
            PlaybackMode::Straight => "→",
            PlaybackMode::Loop => "↻",
            PlaybackMode::Shuffle => "⤨",
        };
        let mode_active = self.playback_mode != PlaybackMode::Straight;

        let playback_progress = if snapshot.duration_value.is_zero() {
            0.0
        } else {
            (playback_position.as_secs_f32() / snapshot.duration_value.as_secs_f32())
                .clamp(0.0, 1.0)
        };
        let now_playing_active_color = colors.accent;
        let show_alternate_now_playing_info = now_playing_info_hovered && window.modifiers().alt;
        let year_label = if snapshot.year.eq_ignore_ascii_case("unknown year") {
            "Unknown Year".to_string()
        } else {
            snapshot.year.to_string()
        };
        let alternate_status = format!("{} | {}", year_label, playback_status_label);
        let title_color = if hovered_link == Some(NowPlayingLink::Title) {
            now_playing_active_color
        } else {
            colors.text_strong
        };
        let artist_color = if hovered_link == Some(NowPlayingLink::Artist) {
            now_playing_active_color
        } else {
            colors.text_muted
        };
        let album_color = if hovered_link == Some(NowPlayingLink::Album) {
            now_playing_active_color
        } else {
            colors.text_faint
        };
        let volume_fill = PLAYER_VOLUME_BAR_W * volume;
        // For animation IDs that need to stay stable per track but
        // change across tracks. The path's hash is sufficient — the
        // user-visible distinguisher is "different track, different
        // marquee scroll position".
        let snap_for_marquee = snapshot.clone();

        // The render snapshot is consumed by closures; clone the
        // bits each closure needs.
        let snap_for_album_link = snapshot.clone();
        let snap_for_title_link = snapshot.clone();
        let snap_for_artist_link = snapshot.clone();
        let snap_for_album_text = snapshot.clone();

        let bar = div()
            .id("player-bar")
            .relative()
            .h(px(86.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_t_1()
            .border_color(rgb(colors.button_hover))
            .bg(rgb(colors.player))
            .on_modifiers_changed(cx.listener(|player, event: &ModifiersChangedEvent, _, cx| {
                if player.set_alt_pressed(event.modifiers.alt) {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("now-playing-album-link")
                    .cursor_pointer()
                    .child(artwork::album_tile_with_hover_border(
                        &snap_for_marquee.as_track_view(),
                        54.0,
                        Some(now_playing_active_color),
                        colors,
                    ))
                    .on_click(cx.listener(move |_player, _, _, cx| {
                        cx.emit(PlayerEvent::NowPlayingLinkClicked {
                            kind: NowPlayingLink::Album,
                            path: snap_for_album_link.path.clone(),
                        });
                    })),
            )
            .child(
                div()
                    .id("now-playing-info")
                    .w(px(220.0))
                    .flex_none()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(2.0))
                    .on_hover(cx.listener(|player, hovered: &bool, window, cx| {
                        let alt = window.modifiers().alt;
                        player.set_now_playing_info_hovered(*hovered, alt);
                        cx.notify();
                    }))
                    .child(if show_alternate_now_playing_info {
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_strong))
                                    .child(render_marquee_text(
                                        snapshot.codec.clone(),
                                        SharedString::from(format!(
                                            "now-playing-codec-marquee-{}",
                                            snapshot.path.display()
                                        )),
                                        220.0,
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
                                        SharedString::from(bitrate_label(snapshot.bitrate)),
                                        SharedString::from(format!(
                                            "now-playing-bitrate-marquee-{}",
                                            snapshot.path.display()
                                        )),
                                        220.0,
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
                                        SharedString::from(alternate_status),
                                        SharedString::from(format!(
                                            "now-playing-status-marquee-{}",
                                            snapshot.path.display()
                                        )),
                                        220.0,
                                        7.8,
                                        colors.text_faint,
                                    )),
                            )
                    } else {
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .id("now-playing-title-link")
                                    .w_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(title_color))
                                    .cursor_pointer()
                                    .on_hover(cx.listener(|player, hovered: &bool, _, cx| {
                                        if *hovered {
                                            player.set_hovered_now_playing_link(Some(
                                                NowPlayingLink::Title,
                                            ));
                                        } else if player.hovered_now_playing_link()
                                            == Some(NowPlayingLink::Title)
                                        {
                                            player.set_hovered_now_playing_link(None);
                                        }
                                        cx.notify();
                                    }))
                                    .on_click(cx.listener(move |_player, _, _, cx| {
                                        cx.emit(PlayerEvent::NowPlayingLinkClicked {
                                            kind: NowPlayingLink::Title,
                                            path: snap_for_title_link.path.clone(),
                                        });
                                    }))
                                    .child(render_marquee_text(
                                        snapshot.title.clone(),
                                        SharedString::from(format!(
                                            "now-playing-title-marquee-{}",
                                            snapshot.path.display()
                                        )),
                                        220.0,
                                        8.6,
                                        title_color,
                                    )),
                            )
                            .child(
                                div()
                                    .id("now-playing-artist-link")
                                    .w_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_color(rgb(artist_color))
                                    .cursor_pointer()
                                    .on_hover(cx.listener(|player, hovered: &bool, _, cx| {
                                        if *hovered {
                                            player.set_hovered_now_playing_link(Some(
                                                NowPlayingLink::Artist,
                                            ));
                                        } else if player.hovered_now_playing_link()
                                            == Some(NowPlayingLink::Artist)
                                        {
                                            player.set_hovered_now_playing_link(None);
                                        }
                                        cx.notify();
                                    }))
                                    .on_click(cx.listener(move |_player, _, _, cx| {
                                        cx.emit(PlayerEvent::NowPlayingLinkClicked {
                                            kind: NowPlayingLink::Artist,
                                            path: snap_for_artist_link.path.clone(),
                                        });
                                    }))
                                    .child(render_marquee_text(
                                        snapshot.artist.clone(),
                                        SharedString::from(format!(
                                            "now-playing-artist-marquee-{}",
                                            snapshot.path.display()
                                        )),
                                        220.0,
                                        7.8,
                                        artist_color,
                                    )),
                            )
                            .child(
                                div()
                                    .id("now-playing-album-text-link")
                                    .w_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_color(rgb(album_color))
                                    .cursor_pointer()
                                    .on_hover(cx.listener(|player, hovered: &bool, _, cx| {
                                        if *hovered {
                                            player.set_hovered_now_playing_link(Some(
                                                NowPlayingLink::Album,
                                            ));
                                        } else if player.hovered_now_playing_link()
                                            == Some(NowPlayingLink::Album)
                                        {
                                            player.set_hovered_now_playing_link(None);
                                        }
                                        cx.notify();
                                    }))
                                    .on_click(cx.listener(move |_player, _, _, cx| {
                                        cx.emit(PlayerEvent::NowPlayingLinkClicked {
                                            kind: NowPlayingLink::Album,
                                            path: snap_for_album_text.path.clone(),
                                        });
                                    }))
                                    .child(render_marquee_text(
                                        snapshot.album.clone(),
                                        SharedString::from(format!(
                                            "now-playing-album-marquee-{}",
                                            snapshot.path.display()
                                        )),
                                        220.0,
                                        7.8,
                                        album_color,
                                    )),
                            )
                    }),
            )
            .child(div().flex_1().h_full().relative().child(waveform_seekbar(
                SharedString::from(format_duration(playback_position)),
                snapshot.duration.clone(),
                playback_progress,
                waveform,
                waveform_loading,
                morph_active,
                colors,
                self.waveform_seekbar_scroll_handle.clone(),
                cx,
            )))
            .child(
                div()
                    .w(px(170.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_color(rgb(colors.text_muted))
                    .child(transport_overlay(
                        is_playing,
                        mode_icon,
                        mode_active,
                        colors,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .id("volume-mute")
                                    .cursor_pointer()
                                    .active(|this| this.opacity(0.75))
                                    .on_click(cx.listener(|player, _, _, cx| {
                                        player.toggle_mute(cx);
                                        cx.notify();
                                    }))
                                    .child(volume_speaker_icon(1, colors)),
                            )
                            .child({
                                let volume_handle = self.volume_bar_scroll_handle.clone();
                                div()
                                    .id("volume-bar")
                                    .w(px(PLAYER_VOLUME_BAR_W))
                                    .h(px(18.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .track_scroll(&volume_handle)
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
                                    )
                            })
                            .child(
                                div()
                                    .id("volume-max")
                                    .cursor_pointer()
                                    .active(|this| this.opacity(0.75))
                                    .on_click(cx.listener(|player, _, _, cx| {
                                        player.set_max_volume(cx);
                                        cx.notify();
                                    }))
                                    .child(volume_speaker_icon(3, colors)),
                            ),
                    ),
            )
            .when(output_menu_open_in_player, |this| {
                this.child(player_output_device_menu(
                    self.output_menu_position,
                    self.current_output_label(),
                    colors,
                    cx,
                ))
            });

        // Local volume tooltip rendered inside the player bar (rather
        // than via `TempoApp::tooltip` which lives at root) so volume
        // drags don't reach across entity boundaries 60 times a
        // second.
        let bar = if self.volume_dragging {
            bar.child(volume_tooltip_overlay(
                self.volume_tooltip_label(),
                volume_fill,
                colors,
            ))
        } else {
            bar
        };

        bar.into_any_element()
    }
}

// ============================================================================
// Free helpers — leaf layout building blocks. None take `&self`
// because the renderable inputs are already snapshotted by the time
// they're called.
// ============================================================================

/// Empty placeholder shown in `PlayerEntity::render` only as a
/// defensive fallback when the parent embeds the player without a
/// snapshot. The user-facing empty state lives on `TempoApp::render`
/// (it needs `is_scanning` + scan status).
fn empty_player_bar(colors: ThemeColors) -> AnyElement {
    div()
        .h(px(86.0))
        .flex_none()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .border_t_1()
        .border_color(rgb(colors.button_hover))
        .bg(rgb(colors.player))
        .child(
            div()
                .w(px(54.0))
                .h(px(54.0))
                .rounded_sm()
                .border_1()
                .border_color(rgb(colors.border_strong))
                .bg(rgb(colors.playing))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(colors.text_faint))
                .child("♪"),
        )
        .into_any_element()
}

/// Marquee scroll for overflowing text. Stateless, free function.
pub(super) fn render_marquee_text(
    text: SharedString,
    animation_id: impl Into<SharedString>,
    available_width: f32,
    average_char_width: f32,
    color: u32,
) -> AnyElement {
    let text_width = (text.chars().count() as f32 * average_char_width).max(1.0);

    if text_width <= available_width {
        return div()
            .w_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(rgb(color))
            .child(text)
            .into_any_element();
    }

    let gap = 44.0;
    let scroll_distance = text_width + gap;
    let duration = Duration::from_millis(((scroll_distance / 18.0).max(7.0) * 1000.0) as u64);

    div()
        .w_full()
        .overflow_hidden()
        .text_color(rgb(color))
        .child(
            div()
                .flex()
                .flex_none()
                .whitespace_nowrap()
                .child(div().w(px(text_width)).flex_none().child(text.clone()))
                .child(div().w(px(gap)).flex_none())
                .child(div().w(px(text_width)).flex_none().child(text))
                .with_animation(
                    animation_id.into(),
                    Animation::new(duration).repeat(),
                    move |this, delta| this.ml(px(-scroll_distance * delta)),
                ),
        )
        .into_any_element()
}

pub(super) fn bitrate_label(bitrate: Option<u32>) -> String {
    bitrate
        .map(|bitrate| format!("{bitrate} kbps"))
        .unwrap_or_else(|| "unknown bitrate".to_string())
}

pub(super) fn volume_speaker_icon(waves: usize, colors: ThemeColors) -> AnyElement {
    let color = format!("#{:06x}", colors.text_muted);
    let mut wave_paths = String::new();

    if waves >= 1 {
        wave_paths.push_str(&format!(
            r#"<path d="M14.5 9.4C15.2 10.1 15.6 11 15.6 12C15.6 13 15.2 13.9 14.5 14.6" fill="none" stroke="{color}" stroke-width="1.8" stroke-linecap="round"/>"#
        ));
    }

    if waves >= 2 {
        wave_paths.push_str(&format!(
            r#"<path d="M17 7.2C18.2 8.5 18.9 10.2 18.9 12C18.9 13.8 18.2 15.5 17 16.8" fill="none" stroke="{color}" stroke-width="1.8" stroke-linecap="round"/>"#
        ));
    }

    if waves >= 3 {
        wave_paths.push_str(&format!(
            r#"<path d="M19.4 5C21 7 22 9.4 22 12C22 14.6 21 17 19.4 19" fill="none" stroke="{color}" stroke-width="1.8" stroke-linecap="round"/>"#
        ));
    }

    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M3 9V15H7L12 19V5L7 9H3Z" fill="{color}"/>{wave_paths}</svg>"#
    );

    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(18.0))
    .h(px(18.0))
    .into_any_element()
}

/// Waveform seekbar with progressive playhead, "Loading waveform"
/// pill (when applicable), and elapsed/duration labels. Click emits
/// [`PlayerEvent::RequestSeekFromWaveformClick`] — the parent does
/// the backend-empty restart logic that needs `tracks`.
#[allow(clippy::too_many_arguments)]
fn waveform_seekbar(
    elapsed: SharedString,
    duration: SharedString,
    progress: f32,
    waveform: Arc<[f32]>,
    loading: bool,
    morph_active: bool,
    colors: ThemeColors,
    waveform_handle: gpui::ScrollHandle,
    cx: &mut Context<PlayerEntity>,
) -> impl IntoElement + use<> {
    // Downsample for narrow seekbars. Each bar takes ≥1px plus a 1px
    // gap, and the bars row has 8px (`.px_2()`) padding on each side.
    // If the painted width can't fit all `waveform.len()` bars, we
    // bin the peaks into a smaller buffer so the row stops
    // overflowing past its container — and so the click-ratio
    // (computed against the seekbar's bounds) maps cleanly onto a
    // visible bar.
    //
    // Using the *previous* frame's bounds is good enough: on the
    // very first paint after a track load `bounds.size.width` is
    // zero (`max_bars_for_width` returns 0 → keep full data), and
    // every subsequent paint reads the most recent painted width,
    // which means a resize is corrected on the next frame.
    //
    // Only the full-resolution waveform is cached on `PlayerEntity`;
    // the downsample is recomputed per render (≤360 floats → cheap)
    // so we don't have to invalidate per-width caches.
    let bars_width = f32::from(waveform_handle.bounds().size.width);
    let bars: Arc<[f32]> = match max_bars_for_width(bars_width) {
        Some(max) if max < waveform.len() => Arc::from(downsample_peaks(&waveform, max)),
        _ => Arc::clone(&waveform),
    };
    let progress_segments = (bars.len() as f32 * progress).round() as usize;
    let bars_for_iter = Arc::clone(&bars);

    let bars_row = div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .px_2()
        .flex()
        .items_center()
        .gap(px(1.0))
        // `Arc<[f32]>::iter()` borrows the slice — no clone of the
        // underlying buffer per frame, unlike the prior `Vec<f32>`
        // which was cloned per render.
        .children(
            bars_for_iter
                .iter()
                .copied()
                .enumerate()
                .map(move |(ix, height)| {
                    waveform_bar(ix, height, progress_segments, loading, colors)
                }),
        );

    // Wrap the bars row in `with_animation` whenever something is
    // moving — either the loading shimmer (heights regenerated each
    // frame from a wall-clock phase inside `cached_waveform`) or
    // the per-column morph (heights lerped each frame inside
    // `resolve_waveform_heights`). The animation's only job is to
    // force a fresh paint at the window's refresh rate; the actual
    // visual change is computed in the entity, not in this closure.
    //
    // Phase 3 #17: this replaces the lone direct
    // `request_animation_frame` call in the codebase. Future
    // animation needs in this entity should follow the same
    // `with_animation` idiom.
    //
    // The animation IDs are distinct so that switching between
    // "loading" and "morphing" mid-animation doesn't reset the
    // wall-clock used by the shimmer. The looped 1500ms / 400ms
    // periods are arbitrary — the closure is a no-op so the period
    // only affects how often the animation system re-evaluates
    // (and therefore repaints).
    let bars_row: AnyElement = if loading {
        bars_row
            .with_animation(
                "waveform-loading-shimmer",
                Animation::new(Duration::from_millis(1500)).repeat(),
                |this, _delta| this,
            )
            .into_any_element()
    } else if morph_active {
        bars_row
            .with_animation(
                "waveform-morph",
                Animation::new(WAVEFORM_MORPH_DURATION),
                |this, _delta| this,
            )
            .into_any_element()
    } else {
        bars_row.into_any_element()
    };

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
        .bg(rgb(colors.waveform_bg))
        .border_1()
        .border_color(rgb(colors.waveform_border))
        // `track_scroll` records the seekbar's painted bounds on
        // every paint, so the click handler below can compute a
        // ratio against the *actual* rendered width. Earlier code
        // derived the seekbar's left/right from `viewport_size()`
        // and the fixed player-bar layout constants, which gave
        // stale results between a window resize and the next paint.
        .track_scroll(&waveform_handle)
        .on_click(cx.listener(|player, event: &ClickEvent, _window, cx| {
            if event.standard_click() {
                let bounds = player.waveform_seekbar_scroll_handle.bounds();
                let width = f32::from(bounds.size.width);
                if width <= 0.0 {
                    return;
                }
                let click_x = f32::from(event.position().x);
                let ratio = ((click_x - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
                cx.emit(PlayerEvent::RequestSeekFromWaveformClick { ratio });
            }
        }))
        .child(
            div()
                .absolute()
                .top(px(42.0))
                .left_0()
                .right_0()
                .h(px(1.0))
                .bg(rgb(colors.waveform_line)),
        )
        .child(bars_row)
        .when(loading, |this| {
            this.child(
                div()
                    .absolute()
                    .top_2()
                    .left_3()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(colors.waveform_bg))
                    .text_xs()
                    .text_color(rgb(colors.waveform_played_peak))
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
                .bg(rgb(colors.waveform_bg))
                .text_xs()
                .text_color(rgb(colors.text_faint))
                .child(elapsed),
        )
        .child(
            div()
                .absolute()
                .bottom_2()
                .right_3()
                .px_1()
                .rounded_sm()
                .bg(rgb(colors.waveform_bg))
                .text_xs()
                .text_color(rgb(colors.text_faint))
                .child(duration),
        )
}

/// Return the maximum number of bars that fit in a seekbar of the
/// given painted width, accounting for the 8px (`.px_2()`) padding
/// on each side of the bars row and the 1px gap between bars.
/// Returns `None` when the width is non-positive (e.g. before the
/// first paint), signalling the caller to keep the full waveform.
fn max_bars_for_width(width: f32) -> Option<usize> {
    // px_2 = 8px each side
    const HORIZONTAL_PADDING: f32 = 16.0;
    // 1px bar + 1px gap, and one fewer gap than bars
    const PER_BAR: f32 = 2.0;

    let usable = width - HORIZONTAL_PADDING + 1.0; // +1 cancels the missing trailing gap
    if usable <= 0.0 {
        return None;
    }
    let max = (usable / PER_BAR).floor() as usize;
    if max == 0 { None } else { Some(max) }
}

/// Bin `src` into `target_len` peaks, keeping the maximum value of
/// each bin. Used to shrink the cached waveform for narrow seekbars
/// without losing the loud transients that make the bar legible.
/// Caller guarantees `0 < target_len < src.len()`.
fn downsample_peaks(src: &[f32], target_len: usize) -> Vec<f32> {
    debug_assert!(target_len > 0 && target_len < src.len());
    let src_len = src.len();
    let mut out = Vec::with_capacity(target_len);
    for ix in 0..target_len {
        // Float-bucket boundaries (avoids the off-by-one that
        // integer division `src_len * ix / target_len` introduces
        // when `target_len` doesn't divide `src_len`).
        let start = ((ix * src_len) as f32 / target_len as f32).floor() as usize;
        let end = (((ix + 1) * src_len) as f32 / target_len as f32).ceil() as usize;
        let end = end.min(src_len).max(start + 1);
        let peak = src[start..end].iter().copied().fold(0.0_f32, f32::max);
        out.push(peak);
    }
    out
}

fn waveform_bar(
    ix: usize,
    height: f32,
    progress_segments: usize,
    loading: bool,
    colors: ThemeColors,
) -> impl IntoElement {
    let played = ix < progress_segments;
    let playhead = ix == progress_segments;
    let peak = height > 44.0;
    let color = if loading && peak {
        colors.waveform_played
    } else if loading {
        colors.waveform_idle_peak
    } else if playhead {
        colors.waveform_playhead
    } else if played && peak {
        colors.waveform_played_peak
    } else if played {
        colors.waveform_played
    } else if peak {
        colors.waveform_idle_peak
    } else {
        colors.waveform_idle
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

/// Transport controls strip: mode-cycle, prev, play/pause, next,
/// random. All clicks emit `PlayerEvent::Request*` variants — the
/// parent handles the cross-region work (resolving the next track
/// from the active tab's index list, smart pause/resume/restart,
/// etc.).
fn transport_overlay(
    is_playing: bool,
    mode_icon: &'static str,
    mode_active: bool,
    colors: ThemeColors,
    cx: &mut Context<PlayerEntity>,
) -> impl IntoElement + use<> {
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(colors.app))
        .border_1()
        .border_color(rgb(colors.waveform_border))
        .child(
            transport_button(mode_icon, false, mode_active, colors).on_click(cx.listener(
                |player, _, _, cx| {
                    player.cycle_playback_mode(cx);
                    cx.notify();
                },
            )),
        )
        .child(
            transport_button("◀", false, false, colors).on_click(cx.listener(
                |_player, _, _, cx| {
                    cx.emit(PlayerEvent::RequestPlayPrev);
                },
            )),
        )
        .child(
            transport_button(if is_playing { "Ⅱ" } else { "▶" }, true, false, colors).on_click(
                cx.listener(|_player, _, _, cx| {
                    cx.emit(PlayerEvent::RequestPlayPause);
                }),
            ),
        )
        .child(
            transport_button("▶", false, false, colors).on_click(cx.listener(
                |_player, _, _, cx| {
                    cx.emit(PlayerEvent::RequestPlayNext);
                },
            )),
        )
        .child(
            transport_button("↻", false, false, colors).on_click(cx.listener(
                |_player, _, _, cx| {
                    cx.emit(PlayerEvent::RequestPlayRandom);
                },
            )),
        )
}

fn transport_button(
    label: &'static str,
    primary: bool,
    active: bool,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let size = if primary { 28.0 } else { 22.0 };
    let hover_size = if primary { 32.0 } else { 26.0 };
    let bg = if primary {
        colors.transport_primary_bg
    } else if active {
        colors.text_strong
    } else {
        colors.player
    };
    let fg = if primary {
        colors.transport_primary_fg
    } else if active {
        colors.app
    } else {
        colors.text_muted
    };

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
                .bg(rgb(colors.text_strong))
                .text_color(rgb(colors.app))
        })
        .child(label)
}

/// Floating dropdown anchored above the player-bar's status pill,
/// listing available output devices. Selecting an item emits
/// [`PlayerEvent::RequestSelectOutputDevice`] which the parent
/// translates into a backend swap + optional playback restart.
fn player_output_device_menu(
    position: Point<Pixels>,
    current_output: String,
    colors: ThemeColors,
    cx: &mut Context<PlayerEntity>,
) -> impl IntoElement + use<> {
    menu_at(
        position,
        Corner::BottomLeft,
        point(px(0.0), px(-8.0)),
        output_device_menu_panel(current_output, colors, cx),
    )
}

fn output_device_menu_panel(
    current_output: String,
    colors: ThemeColors,
    cx: &mut Context<PlayerEntity>,
) -> impl IntoElement + use<> {
    let devices = perf::time(
        "player.output_devices_for_menu",
        "",
        PlaybackController::output_devices,
    );

    menu_panel(260.0, colors)
        .child(menu_header_with_subtitle(
            "Audio Output",
            current_output.clone(),
            colors,
        ))
        .when(devices.is_empty(), |this| {
            this.child(
                div()
                    .px_3()
                    .py_2()
                    .text_color(rgb(colors.text_muted))
                    .child("No output devices found"),
            )
        })
        .children(
            devices
                .into_iter()
                .enumerate()
                .map(move |(device_ix, device)| {
                    let selected = device.name == current_output;
                    let label = if device.is_default {
                        format!("{} (default)", device.name)
                    } else {
                        device.name.clone()
                    };
                    let output_name = device.name;

                    menu_item_base(
                        SharedString::from(format!("output-device-{device_ix}")),
                        colors,
                    )
                    .h(px(30.0))
                    .justify_between()
                    .text_color(rgb(if selected {
                        colors.accent_soft
                    } else {
                        colors.text
                    }))
                    .hover(move |this| {
                        this.bg(rgb(colors.button_hover))
                            .text_color(rgb(colors.text_strong))
                    })
                    .on_click(cx.listener(move |_player, _, _, cx| {
                        cx.emit(PlayerEvent::RequestSelectOutputDevice(output_name.clone()));
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(label),
                    )
                    .child(if selected { "✓" } else { "" })
                }),
        )
}

/// Volume tooltip rendered inside the player bar (instead of via
/// `TempoApp::tooltip`) so 60Hz drag updates don't bounce events
/// across entity boundaries. Anchored just above the volume bar via
/// absolute positioning relative to the player bar's bottom-right.
fn volume_tooltip_overlay(
    label: SharedString,
    volume_fill: f32,
    colors: ThemeColors,
) -> impl IntoElement {
    // Position the tooltip near the volume bar (which sits in the
    // 170px controls strip on the right). The exact horizontal
    // offset is approximate; CSS `right` puts it near the volume
    // controls.
    let _ = volume_fill;
    div()
        .absolute()
        .bottom(px(64.0))
        .right(px(20.0))
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(colors.elevated))
        .border_1()
        .border_color(rgb(colors.border_strong))
        .text_xs()
        .text_color(rgb(colors.text_strong))
        .child(label)
}

// ============================================================================
// PlayingTrackSnapshot helpers — bridge to existing helpers that
// expect a `&Track`.
// ============================================================================

impl PlayingTrackSnapshot {
    /// View this snapshot as a temporary `Track`-shaped value for
    /// passing to helpers like `artwork::album_tile_with_hover_border`
    /// that take `&Track`. Only the fields the helpers read are
    /// populated; everything else is filled with cheap defaults.
    pub(super) fn as_track_view(&self) -> Track {
        Track {
            artist_id: None,
            album_id: None,
            path: self.path.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            genre: SharedString::default(),
            track_number: None,
            year: self.year.clone(),
            date_added: SystemTime::UNIX_EPOCH,
            duration: self.duration.clone(),
            duration_value: self.duration_value,
            codec: self.codec.clone(),
            bitrate: self.bitrate,
            file_size: 0,
            plays: 0,
            liked: false,
            artwork: self.artwork.clone(),
            album_initials: self.album_initials.clone(),
            album_color: self.album_color,
            searchable_lower: String::new(),
        }
    }
}

// ============================================================================
// PlayerEntity helper — waveform cache lookup keyed by path.
// ============================================================================

impl PlayerEntity {
    /// Path-keyed wrapper around `cached_waveform`. The render path
    /// has the active path in scope (from the snapshot), but the
    /// existing cache API takes a `WaveformSource` because that's
    /// what the decoder needs. This builder constructs a minimal
    /// source from the snapshot.
    pub(super) fn cached_waveform_for_path(
        &mut self,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> (Arc<[f32]>, bool) {
        let Some(snapshot) = self.playing_track.as_ref() else {
            // Shouldn't be called without a snapshot, but fall back
            // gracefully.
            return (
                Arc::<[f32]>::from(entity::generate_loading_waveform(
                    entity::waveform_loading_phase(),
                )),
                false,
            );
        };
        debug_assert_eq!(snapshot.path, path);
        let source = WaveformSource {
            path: path.to_path_buf(),
            title: snapshot.title.clone(),
            artist: snapshot.artist.clone(),
            album: snapshot.album.clone(),
            duration: snapshot.duration.clone(),
            duration_value: snapshot.duration_value,
        };
        self.cached_waveform(&source, cx)
    }
}
