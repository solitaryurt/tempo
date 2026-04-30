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
mod mini;
mod render;
mod visualizers;

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
                // Keep the OS title bar in sync with the active
                // track while we're in mini mode (the user can't
                // see the in-app marquee from outside the window).
                if matches!(self.window_mode, WindowMode::Mini(_)) {
                    self.pending_window_title = Some(self.format_window_title());
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
            PlayerEvent::PlayThresholdReached { path } => {
                // The user has now listened to this track for at least
                // `PLAY_THRESHOLD_SECS`; commit the deferred play
                // (catalog `play_count`, in-memory mirror, optional
                // history entry). Sub-threshold skips never reach this
                // arm, so quickly hopping between tracks leaves no
                // trace.
                self.commit_play_for_path(path.as_path());
                cx.notify();
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
            PlayerEvent::RequestEnterMini => {
                self.enter_mini_mode(cx);
                cx.notify();
            }
            PlayerEvent::RequestExitMini => {
                self.exit_mini_mode(cx);
                cx.notify();
            }
            PlayerEvent::RequestCycleMiniSize => {
                self.cycle_mini_size(cx);
                cx.notify();
            }
        }
    }

    /// Switch from full-window layout to mini player. Defaults to
    /// `MiniSize::CompactBar`. The actual swap (open new window,
    /// close old) is deferred to the next render (which has
    /// `&mut Window` + `&mut App`); see `pending_window_swap` in
    /// `TempoApp::render`.
    ///
    /// TODO(always-on-top): when GPUI gains a runtime API to mark a
    /// window as always-on-top (or when a platform shim is added),
    /// flip it on here and clear it in `exit_mini_mode`. The mini
    /// player particularly benefits because it's small enough that
    /// other windows can easily cover it.
    pub(super) fn enter_mini_mode(&mut self, cx: &mut Context<Self>) {
        if matches!(self.window_mode, WindowMode::Mini(_)) {
            return;
        }
        let target = MiniSize::CompactBar;
        self.window_mode = WindowMode::Mini(target);
        self.pending_window_swap = Some(PendingWindowSwap {
            target_size: self.target_size_for(target),
            // Render-time computes the centered position so the new
            // window pops up where the user is looking.
            explicit_bounds: None,
        });
        // Title gets refreshed on the next track-change too, but
        // push it now so the OS shell sees something useful even if
        // playback never advances.
        self.pending_window_title = Some(self.format_window_title());
        self.player.update(cx, |player, player_cx| {
            player.set_window_mode(WindowMode::Mini(target), player_cx);
        });
    }

    /// Resolve the window size to open for a given mini variant.
    ///
    /// Both variants use their fixed default — `CompactBar` because
    /// there's nothing useful to resize on a horizontal strip, and
    /// `Square` because we intentionally don't persist the user's
    /// drag-resize across cycles (each entry into `Square` reopens
    /// at the 400x400 default and the user resizes from there).
    pub(super) fn target_size_for(&self, mini: MiniSize) -> Size<Pixels> {
        mini.default_window_size()
    }

    /// Restore the full-window layout. Reopens at the bounds
    /// captured the moment we entered mini mode, or at a centered
    /// default if none were captured.
    pub(super) fn exit_mini_mode(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.window_mode, WindowMode::Mini(_)) {
            return;
        }
        self.window_mode = WindowMode::Full;
        // `saved_full_bounds` was captured at the top of `Render`
        // the first frame we entered mini mode (we needed `&mut
        // Window` for `window.bounds()`, which event handlers don't
        // have).
        let restore_bounds = self.saved_full_bounds.take();
        self.pending_window_swap = Some(PendingWindowSwap {
            target_size: restore_bounds
                .map(|b| b.size)
                .unwrap_or_else(|| size(px(1280.0), px(820.0))),
            explicit_bounds: restore_bounds,
        });
        self.pending_window_title = Some("Tempo".to_string());
        self.player.update(cx, |player, player_cx| {
            player.set_window_mode(WindowMode::Full, player_cx);
        });
    }

    /// Rotate to the next mini size. No-op when already in full mode.
    ///
    /// Cycling uses the same window-recreate path as full↔mini
    /// transitions because in-place `window.resize()` is essentially
    /// advisory on Wayland (the call updates GPUI's internal buffer
    /// size but most compositors — including Hyprland — ignore the
    /// hint and keep the outer window at whatever size their layout
    /// rules dictated). Recreating the window with explicit
    /// `WindowBounds::Windowed(...)` is the only reliable way to
    /// actually shrink/grow the surface.
    ///
    /// The replacement window is opened with `WindowKind::Floating`
    /// (set in the swap-drain block in `TempoApp::render`) which
    /// asks the compositor to float the new window via
    /// `xdg_toplevel.set_parent`. Hyprland reads that as "transient
    /// → float by default", so users don't need a manual
    /// `windowrulev2 = float, class:tempo` to keep the mini player
    /// floating across size cycles.
    pub(super) fn cycle_mini_size(&mut self, cx: &mut Context<Self>) {
        let WindowMode::Mini(current) = self.window_mode else {
            return;
        };
        let next = current.next();
        self.window_mode = WindowMode::Mini(next);
        self.pending_window_swap = Some(PendingWindowSwap {
            target_size: self.target_size_for(next),
            // Keep the new window centered on the current window's
            // center — the swap-path render code computes this when
            // `explicit_bounds` is `None`.
            explicit_bounds: None,
        });
        self.player.update(cx, |player, player_cx| {
            player.set_window_mode(WindowMode::Mini(next), player_cx);
        });
    }

    /// Format the OS title bar text for the current track. Used
    /// while in mini mode so the system task switcher / taskbar
    /// shows what's playing without the user having to un-mini the
    /// window.
    pub(super) fn format_window_title(&self) -> String {
        let Some(track) = self.tracks.get(self.playing_track) else {
            return "Tempo".to_string();
        };
        // Em-dash separators read better than hyphens in title-bar
        // ellipsis paths.
        format!("{} — {} — {}", track.title, track.artist, track.album)
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
        self.seekbar_visualizer_snapshot = player.seekbar_visualizer();
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
        // Track every dropped queue position so we can adjust the
        // cursor; positions to the left of the cursor that were
        // dropped shift it left, and a drop *at* the cursor clears
        // it altogether (the cursor entry no longer exists).
        let cursor = self.queue_cursor;
        let mut new_cursor = cursor;
        let mut dropped_before_cursor: usize = 0;
        let new_queue: Vec<usize> = self
            .queue
            .iter()
            .enumerate()
            .filter_map(|(position, track_ix)| {
                if *track_ix == removed_ix {
                    if let Some(c) = cursor {
                        if position == c {
                            new_cursor = None;
                        } else if position < c {
                            dropped_before_cursor += 1;
                        }
                    }
                    None
                } else if *track_ix > removed_ix {
                    Some(*track_ix - 1)
                } else {
                    Some(*track_ix)
                }
            })
            .collect();
        self.queue = new_queue;
        if let Some(c) = new_cursor {
            self.queue_cursor = Some(c.saturating_sub(dropped_before_cursor));
        } else {
            self.queue_cursor = None;
        }
    }

    pub(super) fn queue_track(&mut self, track_ix: usize) {
        self.queue_track_at_end(track_ix);
    }

    /// Clear every entry from the Up Next queue and re-collapse the
    /// right sidebar (it auto-shows when items are added; mirroring
    /// that on clear keeps the reopen-arrow gating in `library_view`
    /// consistent). Bound to the `✕` button in the queue header.
    pub(super) fn clear_queue(&mut self, cx: &mut Context<Self>) {
        if self.queue.is_empty() {
            return;
        }
        self.queue.clear();
        self.queue_cursor = None;
        self.right_sidebar_collapsed = true;
        self.queue_context_menu = None;
        cx.notify();
    }

    /// Remove the entry at `queue_position`. Out-of-range calls are
    /// silently ignored so racing clicks can't panic the app. Adjusts
    /// the active-row cursor: removing the cursor entry drops it,
    /// removing an entry above shifts it left so it still points at
    /// the same logical row.
    pub(super) fn remove_queue_entry(&mut self, queue_position: usize, cx: &mut Context<Self>) {
        if queue_position >= self.queue.len() {
            return;
        }
        self.queue.remove(queue_position);
        if let Some(cursor) = self.queue_cursor {
            self.queue_cursor = if cursor == queue_position {
                None
            } else if cursor > queue_position {
                Some(cursor - 1)
            } else {
                Some(cursor)
            };
        }
        self.queue_context_menu = None;
        cx.notify();
    }

    /// Move the entry at `from` to `to` within the queue. `to` is
    /// interpreted in the *pre-removal* index space (i.e. the visible
    /// drop target index) so callers don't have to special-case the
    /// "drop after the source" math themselves. The cursor follows
    /// whichever entry it pointed at: if it pointed at `from`, it
    /// tracks the moved entry to its new position; otherwise it
    /// shifts to keep pointing at the same logical entry.
    pub(super) fn move_queue_entry(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from >= self.queue.len() || from == to {
            return;
        }
        let value = self.queue.remove(from);
        let adjusted = if from < to { to - 1 } else { to };
        let clamped = adjusted.min(self.queue.len());
        self.queue.insert(clamped, value);
        if let Some(cursor) = self.queue_cursor {
            self.queue_cursor = Some(if cursor == from {
                // Cursor entry itself was the one moved.
                clamped
            } else if from < cursor && cursor <= clamped {
                // Removed something before the cursor and reinserted
                // at-or-after it -- cursor effectively shifts left by 1.
                cursor - 1
            } else if clamped <= cursor && cursor < from {
                // Inserted before the cursor -- cursor shifts right by 1.
                cursor + 1
            } else {
                cursor
            });
        }
        cx.notify();
    }

    /// Insert `track_ix` at `position` in the queue. Used by drops
    /// from the main track table onto a queue row (insert above) or
    /// onto the bottom drop zone (append). Out-of-bounds `track_ix`
    /// is rejected; out-of-bounds `position` is clamped. Shifts the
    /// cursor right by 1 if the insert lands at or before it so it
    /// still points at the same logical entry.
    pub(super) fn insert_in_queue(
        &mut self,
        position: usize,
        track_ix: usize,
        cx: &mut Context<Self>,
    ) {
        if track_ix >= self.tracks.len() {
            return;
        }
        let position = position.min(self.queue.len());
        self.queue.insert(position, track_ix);
        if let Some(cursor) = self.queue_cursor
            && position <= cursor
        {
            self.queue_cursor = Some(cursor + 1);
        }
        self.right_sidebar_collapsed = false;
        cx.notify();
    }

    pub(super) fn queue_track_at_start(&mut self, track_ix: usize) {
        if track_ix >= self.tracks.len() {
            return;
        }

        self.queue.insert(0, track_ix);
        // Inserting at 0 shifts every queue entry right by one, so
        // the cursor (if set) needs to follow.
        if let Some(cursor) = self.queue_cursor {
            self.queue_cursor = Some(cursor + 1);
        }
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
            // Library went empty -- the queue is about to be (or has
            // been) cleared by the same library reload, so the cursor
            // can't point at anything meaningful anymore.
            self.queue_cursor = None;
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
        // Any non-queue-driven play invalidates the queue cursor: the
        // user is now playing something *outside* the Up Next list,
        // so the active-row indicator should disappear from the
        // queue. The two queue-driven paths (`play_queue_entry` and
        // queue auto-advance in `play_finished_track`) re-set the
        // cursor *after* this call returns.
        self.queue_cursor = None;
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
            // Recording the play (catalog `play_count`, in-memory
            // `Track.plays` mirror, JSON history append) is *deferred*
            // until the user has actually listened for at least
            // `PLAY_THRESHOLD_SECS`. The player entity's tick emits
            // `PlayerEvent::PlayThresholdReached { path }` once that
            // threshold is crossed, and `handle_player_event` calls
            // `commit_play_for_path` from there. Skipping a track
            // before the threshold therefore leaves no trace.
            //
            // `record_history == false` (the device-switch reload
            // path) is captured here so the upcoming threshold event
            // for this same path reuses the original intent rather
            // than double-counting after `select_output_device`
            // restarts playback.
            self.pending_play = Some(PendingPlay {
                path: track_path.clone(),
                record_history,
            });
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

    /// Commit a deferred play once the user has listened past
    /// [`PLAY_THRESHOLD_SECS`]. Increments the catalog `play_count`
    /// (authoritative across restarts), mirrors the new count onto
    /// the in-memory `Track` so the table column refreshes without a
    /// query round-trip, rebuilds the genres index (play counts feed
    /// "most-played" sorts), and appends a `PlaybackHistoryEntry`
    /// when `record_history` was set on the pending play.
    ///
    /// Called from the `PlayerEvent::PlayThresholdReached` arm of
    /// `handle_player_event`. The `path` arg is the path the player
    /// crossed 15 s on; we resolve to a track index here (rather than
    /// trusting `self.playing_track`) so a quick skip+restart can't
    /// commit a play against the wrong row.
    pub(super) fn commit_play_for_path(&mut self, path: &std::path::Path) {
        let Some(pending) = self.pending_play.take() else {
            return;
        };
        if pending.path != path {
            // Stale event for a track that was skipped before crossing
            // the threshold. Drop it; the new track's pending entry
            // (if any) is already in place.
            return;
        }
        let Some(&track_ix) = self.track_path_index.get(&pending.path) else {
            // Track was removed from the library between play and
            // commit (very rare — would need a rescan mid-playback).
            return;
        };

        let plays = perf::time(
            "player.increment_play_count",
            format!("path={}", pending.path.display()),
            || {
                self.catalog
                    .as_ref()
                    .and_then(|catalog| catalog.increment_play_count(&pending.path).ok())
                    .unwrap_or_else(|| {
                        self.tracks
                            .get(track_ix)
                            .map(|track| track.plays.saturating_add(1))
                            .unwrap_or(1)
                    })
            },
        );
        if let Some(track) = self.tracks.get_mut(track_ix) {
            track.plays = plays;
        }
        self.rebuild_genres();
        if pending.record_history {
            self.record_playback_history(track_ix);
        }
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
    ///
    /// The Up Next queue takes priority over the active table view in
    /// every mode except `Loop` (which always re-plays the current
    /// track). The queue is treated as a list with a *cursor* (see
    /// `self.queue_cursor`) rather than a destructive FIFO -- finishing
    /// a track advances the cursor by one but leaves entries in place
    /// so the user can scrub backward through the visible queue.
    pub(super) fn play_finished_track(&mut self, cx: &mut Context<Self>) {
        let mode = self.player.read(cx).playback_mode();
        if !matches!(mode, PlaybackMode::Loop)
            && let Some(next_position) = self.next_queue_position_valid()
        {
            self.play_queue_entry(next_position, cx);
            return;
        }
        match mode {
            PlaybackMode::Loop => self.play_track(self.playing_track, cx),
            PlaybackMode::Shuffle => self.play_random_track(cx),
            PlaybackMode::Straight => {
                if let Some(next) = self.next_track_after(self.playing_track) {
                    self.play_track(next, cx);
                } else {
                    // End-of-list: surface a status string but keep
                    // the now-playing strip in place so the user can
                    // still hit play again. Drop the queue cursor so
                    // the Up Next sidebar's active-row indicator
                    // disappears -- nothing is playing from the
                    // queue anymore.
                    self.queue_cursor = None;
                    self.player.update(cx, |player, cx| {
                        player.stop(cx);
                        player.playback_status = "Playback finished".to_string();
                    });
                }
            }
        }
    }

    /// Pick the next valid queue position to play, advancing past any
    /// entries whose stored index no longer points into `self.tracks`
    /// (a defensive measure against rescans that shifted indices).
    /// Returns `None` once we reach the end of the queue without
    /// finding a valid entry; the caller falls back to mode-based
    /// selection in that case.
    ///
    /// Starts from `queue_cursor + 1` if a cursor exists (auto-advance
    /// from queue), or from `0` if the cursor is `None` (fresh start
    /// into the queue).
    fn next_queue_position_valid(&self) -> Option<usize> {
        let mut pos = match self.queue_cursor {
            Some(cursor) => cursor + 1,
            None => 0,
        };
        while pos < self.queue.len() {
            if self.queue[pos] < self.tracks.len() {
                return Some(pos);
            }
            pos += 1;
        }
        None
    }

    /// Play the queue entry at `queue_position`, leaving every queue
    /// entry in place and updating the cursor so the Up Next sidebar
    /// indicator follows the active row. Used by manual click on a
    /// queue row, the queue context menu's "Play now", and the
    /// auto-advance path in `play_finished_track`.
    pub(super) fn play_queue_entry(&mut self, queue_position: usize, cx: &mut Context<Self>) {
        let Some(track_ix) = self.queue.get(queue_position).copied() else {
            return;
        };
        if track_ix >= self.tracks.len() {
            return;
        }
        // `play_track_with_history` clears `queue_cursor` as part of
        // its general "any non-queue-driven play invalidates the
        // cursor" rule. Re-set the cursor *after* the call so it
        // reflects the entry we just played.
        self.play_track_with_history(track_ix, true, cx);
        self.queue_cursor = Some(queue_position);
        self.queue_context_menu = None;
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
