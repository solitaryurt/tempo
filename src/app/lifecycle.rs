//! Window/process lifecycle glue.
//!
//! Tempo's design has the system tray (not the main window) own the
//! process lifetime so closing the window leaves audio playing. The
//! pieces involved live across `main.rs`, the GPUI window event
//! plumbing, and the tray dispatcher; the helpers in this module are
//! the small handful of methods on [`TempoApp`] that all of those
//! sites call into so the behaviour stays consistent.
//!
//! - [`set_main_window`](TempoApp::set_main_window) — boot-time and
//!   mini-mode-swap-time recording of the current `WindowHandle` so
//!   the tray, MPRIS `Raise`, and the global-hotkey `ShowWindow`
//!   action all have a single source of truth.
//! - [`focus_main_window`](TempoApp::focus_main_window) — un-minimize
//!   + raise. Used by the three remote-surface code paths above.
//! - [`on_window_close_intercepted`](TempoApp::on_window_close_intercepted)
//!   — record the hidden state and fire a one-time `notify-send`
//!   toast the first time the user X-es out of the window so they
//!   know the app didn't actually quit. Persisted via the
//!   `seen_tray_minimize_toast` field on `AppState`.
//! - [`set_window_hidden`](TempoApp::set_window_hidden) — single
//!   point of truth for the visibility flag; pushes a tray update
//!   so the menu flips between "Show Tempo" and "Hide Tempo".

use std::time::Duration;

use gpui::{AppContext as _, Context, WindowHandle};
use tempo::perf;

use super::TempoApp;

impl TempoApp {
    /// Install the local single-instance server. Repeated launches
    /// connect to it, ask this process to focus the existing window,
    /// then exit before creating a second GPUI app.
    pub(crate) fn install_single_instance_server(
        &mut self,
        server: tempo::single_instance::SingleInstanceServer,
        cx: &mut Context<Self>,
    ) {
        self.single_instance_server = Some(server);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let should_focus = match this.update(cx, |app, _cx| {
                    let Some(server) = app.single_instance_server.as_ref() else {
                        return Ok(false);
                    };
                    server.try_accept_focus_request()
                }) {
                    Ok(Ok(should_focus)) => should_focus,
                    Ok(Err(error)) => {
                        perf::event("single_instance.accept", format!("err={error}"));
                        false
                    }
                    Err(_) => return,
                };
                if !should_focus {
                    continue;
                }
                if this
                    .update(cx, |app, cx| {
                        app.focus_main_window(cx);
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

    /// Stash the current top-level window handle on the app entity.
    /// Called from `main.rs` immediately after the initial
    /// `cx.open_window(...)` succeeds, and from the mini-mode window
    /// swap path in [`TempoApp::render`] after a fresh window has
    /// been mounted.
    pub(crate) fn set_main_window(&mut self, handle: WindowHandle<Self>) {
        self.main_window = Some(handle);
    }

    /// Show / raise the main window. Used by:
    ///
    /// - the global-hotkey [`HotkeyAction::ShowWindow`] dispatcher in
    ///   `hotkeys_panel.rs`,
    /// - the MPRIS `Raise` method (also `hotkeys_panel.rs`),
    /// - the tray icon's "Show window" menu entry (`tray_panel.rs`),
    /// - the tray icon's left-click activation.
    ///
    /// Behaviour:
    ///
    /// - If we have a `WindowHandle`, route a `Window::activate_window()`
    ///   into it. On Wayland this issues an `xdg_activation_v1` token,
    ///   which Hyprland / KDE / GNOME treat as "raise + focus" and
    ///   which un-minimizes a previously minimized window.
    /// - If we don't have a handle yet (vanishingly rare — we set it
    ///   during `TempoApp::new`'s caller), no-op and log so a future
    ///   regression is debuggable from the perf log.
    ///
    /// ## Why `cx.update_window` instead of `handle.update`?
    ///
    /// All call sites of this method run from inside
    /// `Entity<TempoApp>::update(...)` (the tray/MPRIS/hotkey
    /// dispatchers). `WindowHandle::update` re-leases the root view —
    /// which *is* `TempoApp` — and panics with "cannot update
    /// TempoApp while it is already being updated". Going through
    /// `cx.update_window` borrows the `Window` and `App` only and
    /// leaves the typed entity alone, so the existing lease stays
    /// valid.
    pub(super) fn focus_main_window(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.main_window else {
            perf::event("lifecycle.focus_main_window", "missing_handle");
            return;
        };
        // On Hyprland the window may live on a special workspace
        // (parked there by [`hide_main_window`]); pull it back to
        // the visible workspace before asking GPUI to focus. On
        // every other compositor this is a no-op.
        #[cfg(all(unix, not(target_os = "macos")))]
        tempo::hypr::show_window(match self.window_restore_mode {
            super::WindowRestoreMode::BringHere => tempo::hypr::RestoreMode::BringHere,
            super::WindowRestoreMode::GoToWindow => tempo::hypr::RestoreMode::GoToWindow,
        });
        let result = cx.update_window(handle.into(), |_root, window, _app| {
            // `activate_window()` covers both "raise" and the
            // "un-minimize" transition on every backend GPUI ships.
            // See gpui-0.2.2/src/window.rs:4112.
            window.activate_window();
        });
        if let Err(error) = result {
            perf::event("lifecycle.focus_main_window", format!("err={error:#}"));
            return;
        }
        self.set_window_hidden(false);
    }

    /// Hide / minimize the main window. Used by:
    ///
    /// - the `Ctrl+H` keybinding (action: `HideWindow`),
    /// - the tray icon's "Hide Tempo" menu item,
    /// - the tray icon's left-click toggle (when the window is
    ///   currently visible).
    ///
    /// Same `cx.update_window` rationale as
    /// [`focus_main_window`](Self::focus_main_window): the call sites
    /// run from inside an `Entity<TempoApp>::update`, so going through
    /// `WindowHandle::update` would re-lease the typed entity and
    /// panic. `cx.update_window` borrows only `Window` + `App`.
    ///
    /// Fires the same one-time minimize-to-tray toast the X-button
    /// interceptor uses, so the user sees the explainer regardless
    /// of which path they took.
    pub(super) fn hide_main_window(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.main_window else {
            perf::event("lifecycle.hide_main_window", "missing_handle");
            return;
        };
        // Standard path: `xdg_toplevel.set_minimized()`. KDE/GNOME
        // / X11 honor it; tiling Wayland compositors (Hyprland,
        // sway, river) generally ignore it for tiled windows.
        let result = cx.update_window(handle.into(), |_root, window, _app| {
            window.minimize_window();
        });
        if let Err(error) = result {
            perf::event("lifecycle.hide_main_window", format!("err={error:#}"));
            return;
        }
        // Hyprland-specific fallback: park the window on a hidden
        // special workspace so it actually disappears. No-op on
        // every other environment.
        #[cfg(all(unix, not(target_os = "macos")))]
        tempo::hypr::hide_window();
        self.set_window_hidden(true);
        self.fire_minimize_toast_once();
        let _ = cx;
    }

    /// Single point of truth for the `window_hidden` flag. Updates
    /// the field and pushes the new state to the tray so its menu
    /// flips between "Show Tempo" and "Hide Tempo".
    ///
    /// Called from:
    /// - [`hide_main_window`](Self::hide_main_window) → `true`,
    /// - [`focus_main_window`](Self::focus_main_window) → `false`,
    /// - the `on_window_should_close` X-button interceptor in
    ///   `main.rs` → `true` (via the `notify_minimize_to_tray_once`
    ///   path; see the call chain there),
    /// - the `observe_window_activation` hook installed in
    ///   `TempoApp::new` → `false` when the compositor restores us
    ///   without going through our explicit show paths (taskbar
    ///   click, alt-tab, etc.).
    pub(super) fn set_window_hidden(&mut self, hidden: bool) {
        if self.window_hidden == hidden {
            return;
        }
        self.window_hidden = hidden;
        #[cfg(all(unix, not(target_os = "macos")))]
        self.tray_publish(tempo::tray::TrayUpdate::WindowHidden(hidden));
    }

    /// Combined hook for the `on_window_should_close` X-button
    /// interceptor in `main.rs`. The platform window has already
    /// minimized itself by the time this runs; we just record the
    /// hidden state (so the tray menu flips to "Show Tempo") and
    /// fire the one-time toast.
    pub fn on_window_close_intercepted(&mut self, _cx: &mut Context<Self>) {
        self.set_window_hidden(true);
        self.fire_minimize_toast_once();
    }

    /// Show a one-time "Tempo continues in the tray" toast the first
    /// time the user clicks the X button on the main window. Called
    /// from [`on_window_close_intercepted`](Self::on_window_close_intercepted)
    /// and [`hide_main_window`](Self::hide_main_window). Subsequent
    /// invocations no-op.
    ///
    /// Implementation: shells out to `notify-send` (the freedesktop
    /// notification daemon CLI). It's available by default on KDE,
    /// GNOME, Mate, and any environment that has libnotify-bin. If
    /// it's missing we silently fall through; the toast is purely
    /// informational and the app continues to behave correctly.
    pub(super) fn fire_minimize_toast_once(&mut self) {
        if self.seen_tray_minimize_toast {
            return;
        }
        self.seen_tray_minimize_toast = true;
        self.save_app_state();

        // Run the toast in a detached thread so we don't block the
        // GPUI main thread on `notify-send`'s D-Bus round-trip
        // (fronting libnotify, which then talks to the notification
        // daemon over `org.freedesktop.Notifications`). The thread
        // outlives the GPUI tick easily — the binary is `bash`-
        // sized; failures are silent.
        let _ = std::thread::Builder::new()
            .name("tempo-tray-toast".into())
            .spawn(|| {
                let result = std::process::Command::new("notify-send")
                    .arg("--app-name=Tempo")
                    .arg("--icon=multimedia-player")
                    .arg("Tempo continues in the tray")
                    .arg("Right-click the tray icon to quit.")
                    .status();
                if let Err(error) = result {
                    perf::event(
                        "lifecycle.minimize_toast",
                        format!("notify_send_failed err={error}"),
                    );
                }
            });
    }
}
