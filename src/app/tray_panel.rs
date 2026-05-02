//! GPUI-side glue for the StatusNotifierItem tray icon.
//!
//! Mirrors `hotkeys_panel.rs`'s MPRIS half: boot a background
//! [`tempo::tray::TrayService`], drain its command channel on a
//! GPUI tick, and surface a couple of helpers so the rest of the
//! app can publish now-playing updates with one line each.
//!
//! Linux-only; the whole module is cfg-gated at the import site
//! in `src/app/mod.rs`. Windows / macOS native trays live in
//! follow-up PRs.
//!
//! ## Flow
//!
//! - `start_tray_service` is called once at startup. It tries to
//!   construct a `TrayService`; failures (no D-Bus, missing
//!   StatusNotifierWatcher) are logged and swallowed so a missing
//!   tray host doesn't break the rest of the app.
//! - On success it stashes the service handle on
//!   [`TempoApp::tray_service`] and spawns a `cx.spawn` future that
//!   wakes every 50ms and drains the command channel.
//! - Each command is translated into the existing player /
//!   lifecycle methods (`toggle_playback`, `play_adjacent_track`,
//!   `focus_main_window`, `cx.quit()`).
//! - In return, `tray_publish` is called from
//!   `src/app/player/mod.rs` next to each `mpris_publish` call so
//!   tray tooltip / overlay-icon stay in sync with playback state.

use std::time::Duration;

use gpui::Context;

use tempo::perf;
use tempo::tray::{TrayCommand, TrayService, TrayTrackMeta, TrayUpdate};

use super::TempoApp;

const TRAY_VOLUME_STEP: f32 = 0.05;

impl TempoApp {
    /// Construct the StatusNotifierItem service and start draining
    /// its command channel. No-op on failure (tray icon simply
    /// won't appear, every other surface keeps working).
    pub(super) fn start_tray_service(&mut self, cx: &mut Context<Self>) {
        let (service, command_rx) = match TrayService::new() {
            Ok(pair) => pair,
            Err(error) => {
                perf::event("tray.start_failed", format!("error={error:#}"));
                return;
            }
        };

        // Push initial state so the menu reads sensibly even before
        // the user hits play. We have a `playing_track` index into
        // `self.tracks`; build the same `TrayTrackMeta` we would
        // for a state change.
        let meta = self.tray_current_meta();
        service.push_update(TrayUpdate::NowPlaying(meta));
        service.push_update(TrayUpdate::PlayingState(false));
        service.push_update(TrayUpdate::WindowHidden(self.window_hidden));
        self.tray_service = Some(service);

        // Drain commands on a 50ms tick (same cadence as MPRIS).
        // Unbounded channel + `try_recv` keeps the GPUI thread
        // entirely non-blocking; a burst of clicks accumulates and
        // gets flushed in one update batch.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let mut pending: Vec<TrayCommand> = Vec::new();
                loop {
                    match command_rx.try_recv() {
                        Ok(cmd) => pending.push(cmd),
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => return,
                    }
                }
                if pending.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |app, cx| {
                        for cmd in pending {
                            app.dispatch_tray_command(cmd, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    /// Translate a tray command into the corresponding app
    /// operation. Mirrors `dispatch_mpris_command` and
    /// `dispatch_hotkey_action` so all three remote surfaces share
    /// the same dispatch table semantics.
    pub(super) fn dispatch_tray_command(&mut self, command: TrayCommand, cx: &mut Context<Self>) {
        perf::event("tray.dispatch", format!("cmd={:?}", command));
        match command {
            TrayCommand::LeftClick => {
                // Left-click is a show/hide toggle so users have a
                // single-click way to dismiss the window from the
                // tray without going through the right-click menu.
                // The right-click menu's "Show / Hide Tempo" item
                // (commands `ShowWindow` / `HideWindow` below) does
                // the same thing but always with the explicit
                // direction the label promises.
                if self.window_hidden {
                    self.focus_main_window(cx);
                } else {
                    self.hide_main_window(cx);
                }
            }
            TrayCommand::ShowWindow => {
                self.focus_main_window(cx);
            }
            TrayCommand::HideWindow => {
                self.hide_main_window(cx);
            }
            TrayCommand::MiddleClick | TrayCommand::PlayPause => {
                if !self.tracks.is_empty() {
                    self.toggle_playback(cx);
                }
            }
            TrayCommand::Prev => {
                if !self.tracks.is_empty() {
                    self.play_adjacent_track(-1, cx);
                }
            }
            TrayCommand::Next => {
                if !self.tracks.is_empty() {
                    self.play_adjacent_track(1, cx);
                }
            }
            TrayCommand::Random => {
                if !self.tracks.is_empty() {
                    self.play_random_track(cx);
                }
            }
            TrayCommand::Scroll { delta } => {
                // Freedesktop scroll convention: positive delta on
                // a vertical wheel = up = louder.
                let step = if delta > 0 {
                    TRAY_VOLUME_STEP
                } else if delta < 0 {
                    -TRAY_VOLUME_STEP
                } else {
                    0.0
                };
                if step != 0.0 {
                    let next = (self.volume_snapshot + step).clamp(0.0, 1.0);
                    self.set_playback_volume(next, cx);
                }
            }
            TrayCommand::Quit => {
                // The *only* path that actually exits Tempo. The
                // X-button on the main window minimizes via the
                // `on_window_should_close` interceptor in
                // `main.rs`; user-initiated process exit goes
                // through here so we run the registered
                // `on_app_quit` save handler cleanly.
                perf::event("tray.quit", "");
                cx.quit();
            }
        }
    }

    /// Push a state change to the tray so D-Bus consumers (the
    /// tray host's tooltip / menu / overlay-icon) re-render. No-op
    /// when the tray failed to start.
    pub(super) fn tray_publish(&self, update: TrayUpdate) {
        if let Some(svc) = self.tray_service.as_ref() {
            svc.push_update(update);
        }
    }

    /// Build the now-playing payload for the tray. Returns `None`
    /// when there's no active track so `TrayUpdate::NowPlaying(None)`
    /// can clear the menu's header rows back to "Tempo — Idle".
    pub(super) fn tray_current_meta(&self) -> Option<TrayTrackMeta> {
        let track = self.tracks.get(self.playing_track)?;
        Some(TrayTrackMeta {
            title: track.title.to_string(),
            artist: track.artist.to_string(),
            album: track.album.to_string(),
        })
    }
}
