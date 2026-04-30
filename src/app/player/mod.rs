//! Player module — owns the [`PlayerEntity`] and all rendering /
//! state-mutation code for the player bar (transport, waveform,
//! volume, output device picker).
//!
//! See [`entity`] for the architectural design notes. This file has
//! two purposes:
//!
//! 1. **Render the player bar** — transport, waveform seekbar,
//!    Now-Playing strip, volume, output picker. Rendering reads from
//!    `self.player.read(cx)` and binds clicks/hovers via
//!    `self.player.update(cx, |p, cx| p.foo(...))`. The render code
//!    still lives on `TempoApp` (not `PlayerEntity::render`) because
//!    it needs the active track's metadata from `self.tracks` to
//!    paint the title/artist/album marquees and the album tile.
//!
//!    Deferring the move of `render_player_bar` onto `PlayerEntity`
//!    itself avoids duplicating the `Track` lookup logic; once the
//!    `PlayerEntity` carries enough metadata (via
//!    [`PlayerEvent::PlayingTrackChanged`] payload extension or a
//!    push from `TempoApp` after `play_track`), this can move
//!    wholesale.
//!
//! 2. **TempoApp glue methods** — `play_track`, `play_finished_track`,
//!    `play_random_track`, `play_adjacent_track`, `queue_*`,
//!    `seek_playback_to`, etc. These do cross-region bookkeeping
//!    (play counts, history, queue, tabs) and then delegate the
//!    audio-side work to `PlayerEntity` via `self.player.update`.

use super::*;
use std::time::Instant;

mod entity;
mod render;

pub(super) use entity::{PlayerEntity, PlayerEvent, PlayingTrackSnapshot};

// ============================================================================
// TempoApp glue methods — orchestration that touches both player state
// (audio backend) and parent state (tracks, queue, history, tabs).
// ============================================================================

impl TempoApp {
    /// Single subscriber callback wired in `TempoApp::new`. Translates
    /// each [`PlayerEvent`] into the right cross-region update on
    /// `TempoApp`. Keeping this one method keeps the event vocabulary
    /// visible in one place; new variants get a new arm here.
    pub(super) fn handle_player_event(&mut self, event: &PlayerEvent, cx: &mut Context<Self>) {
        match event {
            PlayerEvent::PlayingTrackChanged { path } => {
                if let Some(path) = path
                    && let Some(&ix) = self.track_path_index.get(path)
                {
                    self.playing_track = ix;
                }
                cx.notify();
            }
            PlayerEvent::IsPlayingChanged(is_playing) => {
                // Table active-row transport icon (Ⅱ vs ▶) and the
                // play-count column need to repaint.
                perf::event(
                    "player.is_playing_changed",
                    format!("is_playing={is_playing}"),
                );
                cx.notify();
            }
            PlayerEvent::TrackFinished { finished_path } => {
                let is_finished_path = self
                    .player
                    .read(cx)
                    .playing_track_path()
                    .is_some_and(|current| current == finished_path.as_path());
                if !is_finished_path {
                    // The user already moved on (manual skip) — ignore
                    // the stale auto-advance.
                    return;
                }
                self.play_finished_track(cx);
            }
            PlayerEvent::NowPlayingLinkClicked { kind, path } => {
                let Some(&track_ix) = self.track_path_index.get(path) else {
                    // Track was removed from library while it kept
                    // playing — silently drop the click.
                    return;
                };
                match kind {
                    NowPlayingLink::Title => self.select_track_in_all_music(track_ix),
                    NowPlayingLink::Artist => self.open_artist_tab_for_track(track_ix),
                    NowPlayingLink::Album => self.open_album_tab_for_track(track_ix),
                }
                cx.notify();
            }
            PlayerEvent::StateMutated => {
                self.refresh_player_state_snapshot(cx);
                self.save_app_state();
            }
            PlayerEvent::RequestPlayPause => {
                self.toggle_playback(cx);
                cx.notify();
            }
            PlayerEvent::RequestPlayPrev => {
                self.play_adjacent_track(-1, cx);
                cx.notify();
            }
            PlayerEvent::RequestPlayNext => {
                self.play_adjacent_track(1, cx);
                cx.notify();
            }
            PlayerEvent::RequestPlayRandom => {
                self.play_random_track(cx);
                cx.notify();
            }
            PlayerEvent::RequestSeekFromWaveformClick { ratio } => {
                self.seek_from_waveform_click(*ratio, cx);
                cx.notify();
            }
            PlayerEvent::RequestSelectOutputDevice(name) => {
                self.select_output_device(name.clone(), cx);
                cx.notify();
            }
        }
    }

    /// Refresh the denormalized `volume_snapshot` and
    /// `output_device_snapshot` mirrors from authoritative
    /// [`PlayerEntity`] state. Called after every
    /// [`PlayerEvent::StateMutated`] and once more at shutdown via the
    /// `on_app_quit` hook so the final save is consistent.
    pub(super) fn refresh_player_state_snapshot(&mut self, cx: &gpui::App) {
        let player = self.player.read(cx);
        self.volume_snapshot = player.volume();
        self.output_device_snapshot = player.output_device().map(str::to_string);
    }

    pub(super) fn start_deferred_playback_init(&self, cx: &mut Context<Self>) {
        self.player.update(cx, |player, player_cx| {
            player.start_deferred_init(player_cx)
        });
    }

    pub(super) fn start_playback_tick(&self, cx: &mut Context<Self>) {
        self.player.update(cx, |player, player_cx| {
            player.start_playback_tick(player_cx)
        });
    }

    pub(super) fn remove_track_from_queue(&mut self, removed_ix: usize) {
        self.queue = self
            .queue
            .iter()
            .filter_map(|track_ix| {
                if *track_ix == removed_ix {
                    None
                } else if *track_ix > removed_ix {
                    Some(*track_ix - 1)
                } else {
                    Some(*track_ix)
                }
            })
            .collect();
    }

    pub(super) fn queue_track(&mut self, track_ix: usize) {
        self.queue_track_at_end(track_ix);
    }

    pub(super) fn queue_track_at_start(&mut self, track_ix: usize) {
        if track_ix >= self.tracks.len() {
            return;
        }

        self.queue.insert(0, track_ix);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    pub(super) fn queue_track_at_end(&mut self, track_ix: usize) {
        if track_ix >= self.tracks.len() {
            return;
        }

        self.queue.push(track_ix);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    pub(super) fn queue_album_from_track(&mut self, track_ix: usize, shuffled: bool) {
        let Some(album) = self.tracks.get(track_ix).map(|track| track.album.clone()) else {
            return;
        };

        let mut album_tracks = self
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(ix, track)| (track.album == album).then_some(ix))
            .collect::<Vec<_>>();

        if shuffled {
            let seed = Self::shuffle_seed();
            album_tracks.sort_by_key(|track_ix| {
                Self::shuffle_key(&self.tracks[*track_ix], *track_ix, seed)
            });
        }

        self.queue.extend(album_tracks);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    pub(super) fn play_genre(&mut self, genre_key: &str, shuffled: bool, cx: &mut Context<Self>) {
        let mut tracks = self.source_track_indices(TabSource::Genre(genre_key.to_string()));
        if tracks.is_empty() {
            return;
        }

        if shuffled {
            let seed = Self::shuffle_seed();
            tracks.sort_by_key(|track_ix| {
                Self::shuffle_key(&self.tracks[*track_ix], *track_ix, seed)
            });
        }

        let first = tracks[0];
        self.queue = tracks.into_iter().skip(1).collect();
        self.right_sidebar_collapsed = self.queue.is_empty();
        self.play_track(first, cx);
    }

    pub(super) fn shuffle_seed() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_default()
    }

    pub(super) fn shuffle_key(track: &Track, track_ix: usize, seed: u64) -> u64 {
        let mut hash = seed ^ ((track_ix as u64).wrapping_mul(0x9e3779b97f4a7c15));

        for part in [&track.title, &track.artist, &track.album] {
            for byte in part.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }

        hash ^ (hash >> 33)
    }

    pub(super) fn clamp_track_indices(&mut self, cx: &mut Context<Self>) {
        if self.tracks.is_empty() {
            for tab in &mut self.tabs {
                tab.selected_track = 0;
            }
            self.playing_track = 0;
            self.context_menu_track = None;
            // The previous monolithic implementation merely flipped
            // `is_playing = false` without stopping the audio
            // backend; calling `player.stop()` here also drains the
            // rodio sink, so a subsequent `restart_library_watcher`
            // doesn't leave half-decoded audio queued. This matches
            // user expectations ("library is empty, so should be
            // silent").
            self.player.update(cx, |player, cx| {
                player.stop(cx);
                player.set_playing_track(None);
            });
            return;
        }

        let last = self.tracks.len() - 1;
        self.playing_track = self.playing_track.min(last);

        for tab_ix in 0..self.tabs.len() {
            let selected_track = self.tabs[tab_ix].selected_track.min(last);
            let replacement_track = {
                let indices = self.track_indices_for_tab(tab_ix);
                (!indices.contains(&selected_track)).then(|| indices.first().copied().unwrap_or(0))
            };
            let tab = &mut self.tabs[tab_ix];
            tab.selected_track = replacement_track.unwrap_or(selected_track);
        }

        if self
            .context_menu_track
            .is_some_and(|track_ix| track_ix > last)
        {
            self.context_menu_track = None;
        }

        self.queue.retain(|track_ix| *track_ix <= last);

        // After clamping, refresh the player's render snapshot from
        // whichever track now occupies the `playing_track` slot. The
        // path may have changed if scan replaced the file; the
        // snapshot's `path` field is what the player uses to
        // disambiguate "still the same track" so this keeps the
        // player bar coherent with the table active row.
        let snapshot = player::PlayingTrackSnapshot::from_track(&self.tracks[self.playing_track]);
        self.player
            .update(cx, |player, _| player.set_playing_track(Some(snapshot)));
    }

    pub(super) fn play_track(&mut self, track_ix: usize, cx: &mut Context<Self>) {
        self.play_track_with_history(track_ix, true, cx);
    }

    pub(super) fn play_track_with_history(
        &mut self,
        track_ix: usize,
        record_history: bool,
        cx: &mut Context<Self>,
    ) {
        let start = Instant::now();
        let Some(track) = self.tracks.get(track_ix) else {
            return;
        };
        let track_path = track.path.clone();
        // Snapshot the render-relevant track fields *before* calling
        // into the player; cheap clones (SharedString, PathBuf, Arc).
        let snapshot = player::PlayingTrackSnapshot::from_track(track);

        self.playing_track = track_ix;
        self.set_active_selected_track(track_ix);
        self.context_menu_track = None;

        let result = self.player.update(cx, |player, cx| {
            player.set_playing_track(Some(snapshot));
            player.start_playback(track_path.clone(), cx)
        });

        if result.is_ok() {
            // Increment play count via catalog (authoritative across
            // restarts) with an in-memory fallback. Then mirror the
            // count onto the in-memory `Track` so the table column
            // reflects it without a query round-trip.
            let plays = perf::time(
                "player.increment_play_count",
                format!("path={}", track_path.display()),
                || {
                    self.catalog
                        .as_ref()
                        .and_then(|catalog| catalog.increment_play_count(&track_path).ok())
                        .unwrap_or_else(|| self.tracks[track_ix].plays.saturating_add(1))
                },
            );
            if let Some(track) = self.tracks.get_mut(track_ix) {
                track.plays = plays;
            }
            self.rebuild_genres();
            if record_history {
                self.record_playback_history(track_ix);
            }
        }
        perf::log_duration(
            "player.play_track",
            start.elapsed(),
            format!(
                "track_ix={track_ix} record_history={record_history} path={}",
                track_path.display()
            ),
        );
    }

    /// Smart play/pause: if currently playing, pause; if paused but
    /// the backend has a loaded track, resume; if the backend is
    /// empty, restart from `self.playing_track`. This lives on
    /// `TempoApp` because the "restart from index" case needs the
    /// `tracks` list.
    pub(super) fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.tracks.is_empty() {
            return;
        }

        let (is_playing, resumable) = {
            let player = self.player.read(cx);
            let is_playing = player.is_playing();
            (is_playing, !is_playing && player.has_playback())
        };

        if is_playing {
            self.player.update(cx, |player, cx| player.pause(cx));
            self.context_menu_track = None;
            return;
        }

        if resumable {
            let resumed = self.player.update(cx, |player, cx| player.resume(cx));
            if !resumed {
                // Backend was empty; fall through to a fresh
                // start_playback below.
                self.play_track(self.playing_track, cx);
            }
            self.context_menu_track = None;
            return;
        }

        // No backend yet (still initializing) — kick off start_playback
        // anyway so once the backend appears, the click takes effect.
        self.play_track(self.playing_track, cx);
        self.context_menu_track = None;
    }

    pub(super) fn select_output_device(&mut self, output_name: String, cx: &mut Context<Self>) {
        let was_playing_result = self.player.update(cx, |player, cx| {
            player.select_output_device(output_name, cx)
        });
        if let Ok(true) = was_playing_result {
            // Audio backend was reset; replay the same track from
            // scratch so the new device takes over without dropping
            // the user's place in the queue.
            self.play_track_with_history(self.playing_track, false, cx);
        }
    }

    /// Programmatic volume change — currently unused by UI (the volume
    /// bar drives the player directly), but exposed for future
    /// media-key / DBus integration.
    #[allow(dead_code)]
    pub(super) fn set_playback_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.player
            .update(cx, |player, cx| player.set_volume(volume, cx));
    }

    /// Volume drag fallback — catches mouse-move events that escape
    /// the volume bar's own `on_mouse_move` listener (e.g. when the
    /// user drags off the bar). Returns `true` if the drag was active
    /// so `TempoApp::render` can stop propagation. Volume tooltip is
    /// rendered locally inside the player bar; the parent doesn't
    /// have to coordinate.
    pub(super) fn drag_volume(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        self.player
            .update(cx, |player, cx| player.drag_volume(event, cx))
            .is_some()
    }

    /// Volume drag-end fallback — catches mouse-up that escapes the
    /// volume bar. Returns `true` if a drag was in progress.
    pub(super) fn finish_volume_drag(&mut self, cx: &mut Context<Self>) -> bool {
        self.player
            .update(cx, |player, cx| player.finish_volume_drag(cx))
    }

    pub(super) fn play_adjacent_track(&mut self, delta: isize, cx: &mut Context<Self>) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let position = indices
            .iter()
            .position(|ix| *ix == self.playing_track)
            .unwrap_or(0);
        let next = (position as isize + delta).clamp(0, indices.len().saturating_sub(1) as isize);
        self.play_track(indices[next as usize], cx);
    }

    /// Auto-advance: pick the next track per the active playback
    /// mode, or stop at end-of-list for `Straight`. Called from the
    /// `PlayerEvent::TrackFinished` arm of `handle_player_event`.
    pub(super) fn play_finished_track(&mut self, cx: &mut Context<Self>) {
        let mode = self.player.read(cx).playback_mode();
        match mode {
            PlaybackMode::Loop => self.play_track(self.playing_track, cx),
            PlaybackMode::Shuffle => self.play_random_track(cx),
            PlaybackMode::Straight => {
                if let Some(next) = self.next_track_after(self.playing_track) {
                    self.play_track(next, cx);
                } else {
                    // End-of-list: surface a status string but keep
                    // the now-playing strip in place so the user can
                    // still hit play again.
                    self.player.update(cx, |player, cx| {
                        player.stop(cx);
                        player.playback_status = "Playback finished".to_string();
                    });
                }
            }
        }
    }

    pub(super) fn next_track_after(&self, track_ix: usize) -> Option<usize> {
        let indices = self.current_track_indices();
        let position = indices.iter().position(|ix| *ix == track_ix)?;
        indices.get(position + 1).copied()
    }

    pub(super) fn play_random_track(&mut self, cx: &mut Context<Self>) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let seed = Self::shuffle_seed();
        let next = indices
            .iter()
            .copied()
            .filter(|track_ix| indices.len() == 1 || *track_ix != self.playing_track)
            .min_by_key(|track_ix| Self::shuffle_key(&self.tracks[*track_ix], *track_ix, seed))
            .unwrap_or(self.playing_track);
        self.play_track(next, cx);
    }

    /// Seek within the currently-playing track. If the audio backend
    /// is empty (e.g. after a stop), restarts playback from the
    /// current track first. Lives on `TempoApp` because the restart
    /// case needs the track index → path resolution.
    ///
    /// Currently exposed for media-key / scrubber integrations; the
    /// in-app waveform click goes through `seek_from_waveform_click`.
    #[allow(dead_code)]
    pub(super) fn seek_playback(&mut self, position: Duration, cx: &mut Context<Self>) {
        let needs_restart = self.player.update(cx, |player, cx| {
            // Check before calling seek so we don't burn the seek
            // request on an empty backend.
            if !player.has_playback() {
                player.playback_status = "Playback unavailable".to_string();
                return false;
            }
            !player.seek(position, cx)
        });
        if needs_restart {
            self.play_track_with_history(self.playing_track, false, cx);
            // After restart, retry the seek so the click feels
            // immediate.
            self.player
                .update(cx, |player, cx| _ = player.seek(position, cx));
        }
    }

    pub(super) fn seek_from_waveform_click(&mut self, ratio: f32, cx: &mut Context<Self>) {
        let Some(track) = self.tracks.get(self.playing_track) else {
            return;
        };
        let duration = track.duration_value;
        let outcome = self.player.update(cx, |player, cx| {
            player.seek_from_waveform_click(ratio, duration, cx)
        });
        if outcome.needs_restart {
            // Backend was empty (e.g. seek after natural end-of-track
            // stop). Restart playback for the current track and
            // re-issue the seek so the click feels immediate.
            self.play_track_with_history(self.playing_track, false, cx);
            self.player
                .update(cx, |player, cx| _ = player.seek(outcome.target, cx));
        }
    }
}

// ============================================================================
// Empty-library placeholder + Settings page hooks
//
// The player bar's main rendering moved to `PlayerEntity::render` (in
// `render.rs`). Two helpers stay on `TempoApp`:
//
// 1. `render_empty_player_bar` — when the library is empty there's no
//    `PlayingTrackSnapshot` to embed the entity with, and the
//    placeholder needs `is_scanning` + `visible_scan_status()` which
//    live on the parent. `TempoApp::render` calls this directly when
//    `self.tracks.is_empty()`.
//
// 2. `playback_status_dropdown` / `settings_output_device_menu` —
//    rendered on the Settings page (sibling of the player bar). The
//    Settings-anchored variant of the output picker has to live at
//    root for absolute positioning relative to the page; the player-
//    anchored variant lives inside `PlayerEntity::render` instead.
// ============================================================================

impl TempoApp {
    /// The empty placeholder that replaces the player bar when the
    /// library has no tracks. Reads `is_scanning` + `visible_scan_status`
    /// from `TempoApp` directly; `PlayerEntity` cannot render this
    /// because it deliberately doesn't know about scan state.
    pub(super) fn render_empty_player_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let _ = cx;
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .child(if self.is_scanning {
                                "Scanning library"
                            } else {
                                "Library scanner idle"
                            }),
                    )
                    .child(
                        div()
                            .text_color(rgb(colors.text_muted))
                            .child(self.visible_scan_status()),
                    ),
            )
            .into_any_element()
    }

    /// Output-device dropdown trigger. Used by the Settings page (and
    /// previously by the player bar before its rendering moved into
    /// `PlayerEntity::render`).
    pub(super) fn playback_status_dropdown(
        &self,
        source: OutputMenuSource,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let button_label = {
            let player = self.player.read(cx);
            match source {
                OutputMenuSource::Player => format!("{} ▾", player.playback_status_label()),
                OutputMenuSource::Settings => format!("{} ▾", player.current_output_label()),
            }
        };

        div()
            .id(SharedString::from(match source {
                OutputMenuSource::Player => "player-output-dropdown",
                OutputMenuSource::Settings => "settings-output-dropdown",
            }))
            .relative()
            .child(
                div()
                    .id(SharedString::from(match source {
                        OutputMenuSource::Player => "player-output-dropdown-button",
                        OutputMenuSource::Settings => "settings-output-dropdown-button",
                    }))
                    .cursor_pointer()
                    .rounded_sm()
                    .px_1()
                    .text_color(rgb(colors.text_muted))
                    .hover(move |this| this.text_color(rgb(colors.accent)).bg(rgb(colors.hover)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        let position = event.position();
                        this.player.update(cx, |player, cx| {
                            player.toggle_output_menu(source, position, cx);
                        });
                    }))
                    .child(button_label),
            )
    }

    /// Settings-anchored output device picker. Rendered as a child of
    /// the root `TempoApp::render` (gated on `output_menu_source ==
    /// Some(Settings)`) so it floats over the Settings page.
    pub(super) fn settings_output_device_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let position = self.player.read(cx).output_menu_position();
        let colors = *self.colors();
        let current_output = self.player.read(cx).current_output_label();
        menu_at(
            position,
            Corner::TopRight,
            point(px(24.0), px(18.0)),
            settings_output_device_panel(current_output, colors, cx),
        )
    }
}

fn settings_output_device_panel(
    current_output: String,
    colors: ThemeColors,
    cx: &mut Context<TempoApp>,
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
                        SharedString::from(format!("settings-output-device-{device_ix}")),
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_output_device(output_name.clone(), cx);
                        cx.notify();
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
