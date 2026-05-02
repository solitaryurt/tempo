//! Settings → Hotkeys panel and global-hotkey wiring.
//!
//! Two pieces live here:
//!
//! 1. **Boot** for the evdev-backed [`HotkeyService`] (read combos
//!    from `AppState`, spawn watcher threads) and the MPRIS D-Bus
//!    server. A pair of GPUI tasks drains each service's event
//!    channel and dispatches into [`TempoApp`].
//! 2. **Render** the Settings → Hotkeys panel: per-action rows with
//!    "Record" / "Clear" buttons, plus a top-of-panel banner that
//!    surfaces the evdev permission state (so a user not in the
//!    `input` group sees the exact fix command).
//!
//! Volume and seek deltas are intentionally hardcoded for v1
//! (5% / 10s).

use std::time::Duration;

use gpui::{
    AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div, prelude::*, px, rgb,
};

use tempo::hotkeys::{HotkeyAction, HotkeyConfig, HotkeyEvent, InitStatus, KeyCombo};
use tempo::mpris::{MprisCommand, MprisPlaybackStatus, MprisTrackMeta, MprisUpdate};
use tempo::perf;

use super::TempoApp;

const VOLUME_STEP: f32 = 0.05;
const SEEK_STEP: Duration = Duration::from_secs(10);

impl TempoApp {
    /// Boot the evdev hotkey watcher with the user's saved bindings.
    /// Failures are non-fatal: missing permissions / no keyboard
    /// devices just means global hotkeys are inert (the in-window
    /// `Ctrl+Space`-style GPUI keybindings still work).
    pub(super) fn start_global_hotkeys(
        &mut self,
        saved_bindings: std::collections::HashMap<String, KeyCombo>,
        cx: &mut Context<Self>,
    ) {
        // Translate the on-disk shape (`{ "play_pause": KeyCombo }`)
        // into the runtime shape the service expects. Keys we don't
        // recognise are dropped silently; this lets us rename a
        // storage key in a future version without leaving stale
        // bindings live.
        let mut bindings = std::collections::HashMap::new();
        for (key, combo) in saved_bindings {
            if let Some(action) = HotkeyAction::from_storage_key(&key) {
                bindings.insert(action, combo);
            } else {
                perf::event("hotkeys.unknown_storage_key", format!("key={key}"));
            }
        }
        let initial_config = HotkeyConfig::from_map(bindings);

        let (service, event_rx) = match tempo::hotkeys::HotkeyService::new(initial_config) {
            Ok(pair) => pair,
            Err(error) => {
                perf::event("hotkeys.init_failed", format!("error={error:#}"));
                return;
            }
        };

        match &service.init_status {
            InitStatus::Ok {
                keyboard_count,
                unreadable,
            } => perf::event(
                "hotkeys.init_ok",
                format!("keyboards={keyboard_count} unreadable={unreadable}"),
            ),
            InitStatus::PermissionDenied(msg) => {
                perf::event("hotkeys.permission_denied", msg.clone())
            }
            InitStatus::NoInputDir(msg) => perf::event("hotkeys.no_input_dir", msg.clone()),
            InitStatus::NoKeyboardsFound => perf::event("hotkeys.no_keyboards", ""),
        }

        self.hotkey_service = Some(service);

        // Drain the event channel on the GPUI thread. 50ms tick
        // mirrors the cadence the library/event loops use; keeps
        // hotkey latency under one frame for typical 60Hz UIs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let mut pending: Vec<HotkeyEvent> = Vec::new();
                loop {
                    match event_rx.try_recv() {
                        Ok(event) => pending.push(event),
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => return,
                    }
                }
                if pending.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |app, cx| {
                        for event in pending {
                            match event {
                                HotkeyEvent::Activated(action) => {
                                    app.dispatch_hotkey_action(action, cx);
                                }
                                HotkeyEvent::Recorded { action, combo } => {
                                    app.finish_recording_hotkey(action, combo, cx);
                                }
                            }
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

    /// Boot the MPRIS D-Bus server. The bus name suffix uses the
    /// process pid so multiple Tempo instances (rare but possible
    /// during dev) don't collide on the same name.
    pub(super) fn start_mpris_server(&mut self, cx: &mut Context<Self>) {
        let bus_suffix = format!("Tempo.instance{}", std::process::id());
        let (service, command_rx) = match tempo::mpris::MprisService::new(bus_suffix) {
            Ok(pair) => pair,
            Err(error) => {
                perf::event("mpris.start_failed", format!("error={error:#}"));
                return;
            }
        };
        self.mpris_service = Some(service);

        // Publish initial state (track metadata + volume + paused).
        // Without this the taskbar widget renders blank until the
        // user first hits play.
        let meta = self.mpris_current_metadata();
        self.mpris_publish(MprisUpdate::Metadata(meta));
        self.mpris_publish(MprisUpdate::Volume(self.volume_snapshot as f64));
        self.mpris_publish(MprisUpdate::PlaybackStatus(MprisPlaybackStatus::Stopped));

        // Drain MPRIS commands on the GPUI thread. 50ms tick.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let mut pending: Vec<MprisCommand> = Vec::new();
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
                            app.dispatch_mpris_command(cmd, cx);
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

    /// Translate a global-hotkey activation into the corresponding
    /// player / app operation.
    pub(super) fn dispatch_hotkey_action(&mut self, action: HotkeyAction, cx: &mut Context<Self>) {
        // Empty-library guard. Some actions (Show window, Toggle
        // mute) still make sense without tracks loaded.
        if self.tracks.is_empty()
            && !matches!(action, HotkeyAction::ShowWindow | HotkeyAction::ToggleMute)
        {
            return;
        }
        perf::event("hotkeys.dispatch", format!("action={:?}", action));
        match action {
            HotkeyAction::PlayPause => self.toggle_playback(cx),
            HotkeyAction::NextTrack => self.play_adjacent_track(1, cx),
            HotkeyAction::PrevTrack => self.play_adjacent_track(-1, cx),
            HotkeyAction::Stop => {
                self.player.update(cx, |player, cx| player.stop(cx));
            }
            HotkeyAction::VolumeUp => {
                let next = (self.volume_snapshot + VOLUME_STEP).clamp(0.0, 1.0);
                self.set_playback_volume(next, cx);
            }
            HotkeyAction::VolumeDown => {
                let next = (self.volume_snapshot - VOLUME_STEP).clamp(0.0, 1.0);
                self.set_playback_volume(next, cx);
            }
            HotkeyAction::ToggleMute => {
                self.player.update(cx, |player, cx| player.toggle_mute(cx));
            }
            HotkeyAction::PlayRandom => self.play_random_track(cx),
            HotkeyAction::SeekForward => {
                let current = self.player.read(cx).playback_position();
                self.seek_playback(current.saturating_add(SEEK_STEP), cx);
            }
            HotkeyAction::SeekBackward => {
                let current = self.player.read(cx).playback_position();
                self.seek_playback(current.saturating_sub(SEEK_STEP), cx);
            }
            HotkeyAction::CyclePlaybackMode => {
                self.player
                    .update(cx, |player, cx| player.cycle_playback_mode(cx));
            }
            HotkeyAction::ShowWindow => {
                // Routed through the shared `focus_main_window` helper
                // so global hotkeys, MPRIS Raise, and the tray's
                // "Show window" item all use the same un-minimize path.
                self.focus_main_window(cx);
            }
        }
    }

    /// Translate an MPRIS D-Bus method call into a player op.
    /// Mirrors `dispatch_hotkey_action` but originates from desktop
    /// integrations (media keys, GNOME widget, KDE Connect).
    pub(super) fn dispatch_mpris_command(&mut self, command: MprisCommand, cx: &mut Context<Self>) {
        perf::event("mpris.dispatch", format!("cmd={:?}", command));
        match command {
            MprisCommand::PlayPause | MprisCommand::Play | MprisCommand::Pause => {
                if !self.tracks.is_empty() {
                    self.toggle_playback(cx);
                }
            }
            MprisCommand::Stop => {
                self.player.update(cx, |player, cx| player.stop(cx));
            }
            MprisCommand::Next => {
                if !self.tracks.is_empty() {
                    self.play_adjacent_track(1, cx);
                }
            }
            MprisCommand::Previous => {
                if !self.tracks.is_empty() {
                    self.play_adjacent_track(-1, cx);
                }
            }
            MprisCommand::SeekRelative(micros) => {
                if self.tracks.is_empty() {
                    return;
                }
                let current = self.player.read(cx).playback_position();
                let target = if micros >= 0 {
                    current.saturating_add(Duration::from_micros(micros as u64))
                } else {
                    current.saturating_sub(Duration::from_micros((-micros) as u64))
                };
                self.seek_playback(target, cx);
            }
            MprisCommand::SetPosition(micros) => {
                if self.tracks.is_empty() || micros < 0 {
                    return;
                }
                self.seek_playback(Duration::from_micros(micros as u64), cx);
            }
            MprisCommand::SetVolume(v) => {
                let clamped = (v as f32).clamp(0.0, 1.0);
                self.set_playback_volume(clamped, cx);
            }
            MprisCommand::Raise => {
                self.focus_main_window(cx);
            }
        }
    }

    /// Push a state change to the MPRIS server so D-Bus consumers
    /// (taskbar widgets, etc) see it. No-op when MPRIS isn't running.
    pub(super) fn mpris_publish(&self, update: MprisUpdate) {
        if let Some(svc) = self.mpris_service.as_ref() {
            svc.push_update(update);
        }
    }

    /// Build a now-playing metadata payload from the current
    /// `playing_track`. Returns `None` when nothing is loaded so the
    /// caller can clear MPRIS metadata.
    pub(super) fn mpris_current_metadata(&self) -> Option<MprisTrackMeta> {
        let track = self.tracks.get(self.playing_track)?;
        Some(MprisTrackMeta {
            title: track.title.to_string(),
            artist: track.artist.to_string(),
            album: track.album.to_string(),
            length_us: track.duration_value.as_micros() as i64,
            art_url: super::art_url_for_track(track),
            trackid: track.path.to_string_lossy().into_owned(),
        })
    }

    /// User pressed a combo while we were recording. Save the
    /// binding, persist app state, and clear recording UI.
    fn finish_recording_hotkey(
        &mut self,
        action: HotkeyAction,
        combo: KeyCombo,
        _cx: &mut Context<Self>,
    ) {
        // Reject duplicates: if another action is already bound to
        // this exact combo, drop the existing binding so a single
        // physical combo only ever fires one action.
        if let Some(svc) = self.hotkey_service.as_ref() {
            let snap = svc.snapshot();
            let conflict = snap
                .iter()
                .find(|(other, c)| **other != action && **c == combo)
                .map(|(other, _)| *other);
            if let Some(other) = conflict {
                perf::event(
                    "hotkeys.recording_replaced_existing",
                    format!(
                        "stolen_from={} new_owner={} combo={}",
                        other.storage_key(),
                        action.storage_key(),
                        combo.display()
                    ),
                );
                svc.set_binding(other, None);
            }
            svc.set_binding(action, Some(combo.clone()));
        }
        perf::event(
            "hotkeys.recorded",
            format!("action={} combo={}", action.storage_key(), combo.display()),
        );
        self.recording_action = None;
        self.save_app_state();
    }

    /// Click handler for the per-row "Record" button.
    pub(super) fn begin_recording_hotkey(&mut self, action: HotkeyAction) {
        if let Some(svc) = self.hotkey_service.as_ref() {
            svc.begin_recording(action);
        }
        self.recording_action = Some(action);
    }

    /// Click handler for the per-row "Clear" button.
    pub(super) fn clear_hotkey(&mut self, action: HotkeyAction) {
        if let Some(svc) = self.hotkey_service.as_ref() {
            svc.set_binding(action, None);
        }
        // If we were recording this same action, cancel recording too.
        if self.recording_action == Some(action) {
            if let Some(svc) = self.hotkey_service.as_ref() {
                svc.cancel_recording();
            }
            self.recording_action = None;
        }
        self.save_app_state();
    }

    /// Cancel an in-flight recording (escape / click outside).
    pub(super) fn cancel_recording_hotkey(&mut self) {
        if let Some(svc) = self.hotkey_service.as_ref() {
            svc.cancel_recording();
        }
        self.recording_action = None;
    }

    /// Render the Settings → Hotkeys panel.
    pub(super) fn render_hotkey_settings(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        // Decide what banner to show at the top. Order matters:
        // permission errors first (most actionable), then "no
        // keyboards" (rare, mostly headless), then the success
        // banner.
        let init_status = self
            .hotkey_service
            .as_ref()
            .map(|svc| svc.init_status.clone());

        let banner_message: Option<(String, bool)> = match init_status.as_ref() {
            None => Some((
                "Global hotkey watcher failed to start. See terminal output for details."
                    .to_string(),
                true,
            )),
            Some(InitStatus::PermissionDenied(msg)) => Some((msg.clone(), true)),
            Some(InitStatus::NoInputDir(msg)) => Some((msg.clone(), true)),
            Some(InitStatus::NoKeyboardsFound) => Some((
                "No keyboard devices detected under /dev/input. Plug in a keyboard \
                 and restart Tempo."
                    .to_string(),
                true,
            )),
            Some(InitStatus::Ok {
                keyboard_count,
                unreadable: _,
            }) => Some((
                format!(
                    "Listening on {keyboard_count} keyboard device{s}. Click Record on \
                     any row, then press the combo you want.",
                    s = if *keyboard_count == 1 { "" } else { "s" }
                ),
                false,
            )),
        };

        div()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.surface))
            .overflow_hidden()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .bg(rgb(colors.elevated))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Global Hotkeys"),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(colors.border))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when_some(banner_message, |this, (text, is_error)| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(if is_error {
                                    rgb(colors.accent)
                                } else {
                                    rgb(colors.text_muted)
                                })
                                .child(SharedString::from(text)),
                        )
                    })
                    .children(
                        HotkeyAction::ALL
                            .into_iter()
                            .map(|action| self.render_hotkey_row(action, cx)),
                    )
                    .child(self.render_mpris_status_line()),
            )
    }

    fn render_hotkey_row(
        &self,
        action: HotkeyAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let storage_key = action.storage_key();

        // Look up the current binding via the service (authoritative
        // source of truth). Reading from the snapshot is cheap (small
        // HashMap clone); we only render twelve rows.
        let current: Option<KeyCombo> = self
            .hotkey_service
            .as_ref()
            .and_then(|svc| svc.snapshot().get(&action).cloned());

        let is_recording = self.recording_action == Some(action);

        // Trigger label: either the bound combo, the recording
        // animation text, or "Not bound".
        let trigger_label: SharedString = if is_recording {
            SharedString::from("Press combo… (Esc cancels)")
        } else if let Some(combo) = current.as_ref() {
            SharedString::from(combo.display())
        } else {
            SharedString::from("Not bound")
        };

        let row_id = SharedString::from(format!("hotkey-row-{storage_key}"));
        let record_id = SharedString::from(format!("hotkey-rec-{storage_key}"));
        let clear_id = SharedString::from(format!("hotkey-clr-{storage_key}"));

        div()
            .id(row_id)
            .min_h(px(34.0))
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgb(colors.row))
            .border_1()
            .border_color(rgb(colors.border))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_color(rgb(colors.text_strong))
                            .child(action.label()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if is_recording {
                                rgb(colors.accent)
                            } else if current.is_some() {
                                rgb(colors.text_strong)
                            } else {
                                rgb(colors.text_muted)
                            })
                            .child(trigger_label),
                    ),
            )
            .child(
                div()
                    .id(record_id)
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text_muted))
                    .hover(|this| {
                        this.bg(rgb(colors.button_hover))
                            .text_color(rgb(colors.text_strong))
                    })
                    .active(|this| this.opacity(0.82))
                    .child(if is_recording { "Cancel" } else { "Record" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.recording_action == Some(action) {
                            this.cancel_recording_hotkey();
                        } else {
                            this.begin_recording_hotkey(action);
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(clear_id)
                    .cursor_pointer()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text_muted))
                    .hover(|this| {
                        this.bg(rgb(colors.button_hover))
                            .text_color(rgb(colors.text_strong))
                    })
                    .active(|this| this.opacity(0.82))
                    .child("Clear")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.clear_hotkey(action);
                        cx.notify();
                    })),
            )
    }

    fn render_mpris_status_line(&self) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let connected = self.mpris_service.is_some();
        div()
            .pt_2()
            .text_xs()
            .text_color(rgb(colors.text_muted))
            .child(if connected {
                "MPRIS active: standard media keys (XF86AudioPlay/Next/Prev) work \
                 system-wide via D-Bus, no setup needed."
            } else {
                "MPRIS unavailable: no D-Bus session found, or another Tempo \
                 instance owns the bus name. Standard media keys won't route to \
                 this app."
            })
    }
}

/// Top-level entry point used by `super::settings::render_settings`.
pub(super) fn render_hotkey_section(app: &TempoApp, cx: &mut Context<TempoApp>) -> AnyElement {
    app.render_hotkey_settings(cx).into_any_element()
}
