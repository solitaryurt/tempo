use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowOptions, actions, px,
    size,
};

mod app;

actions!(
    tempo,
    [
        PlaySelected,
        TogglePause,
        MoveSelectionUp,
        MoveSelectionDown,
        NewTab,
        CloseTab,
        CloseAllTabs,
        ReopenClosedTab,
        NextTab,
        PreviousTab,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectTab9,
        SelectTab10,
        FocusSearch,
        OpenSettings,
        PlayRandomTrack,
        NavigateBack,
        NavigateForward,
        ToggleMiniPlayer,
        CycleMiniPlayer,
        /// Hide the main window to the system tray. The window stays
        /// alive (audio keeps playing) and can be restored from the
        /// tray icon's left-click or "Show Tempo" menu item, the
        /// MPRIS `Raise` method, or the global `ShowWindow` hotkey.
        /// Bound to Ctrl+H.
        HideWindow
    ]
);

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);

        let window_handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    app_id: Some("tempo".into()),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| app::TempoApp::new(window, cx)),
            )
            .expect("failed to open Tempo window");

        // Tray-as-process-root lifecycle. The user's expectation is
        // that closing the main window leaves Tempo running so music
        // keeps playing; only the tray's "Quit Tempo" item actually
        // exits. We achieve this by intercepting the X-button click
        // via `on_window_should_close` (returning `false` cancels
        // the close on every backend GPUI ships) and minimizing
        // instead. Tray "Show window" later un-minimizes via
        // `Window::activate_window`. See gpui-0.2.2/src/window.rs:4329
        // for the API; gpui-0.2.2/src/platform/linux/wayland/window.rs:540
        // is the Wayland honor that makes this work without a fork.
        //
        // The same registration is repeated in the mini-window-swap
        // path inside `TempoApp::render` so that swapped windows
        // also intercept their X button.
        let close_handle = window_handle;
        window_handle
            .update(cx, |app, window, cx| {
                window.on_window_should_close(cx, move |window, cx| {
                    window.minimize_window();
                    // Hyprland ignores xdg_toplevel.set_minimized()
                    // for tiled windows, so the call above is a
                    // no-op there. Pair it with an explicit
                    // `hyprctl dispatch` that parks the window on
                    // a hidden special workspace. No-op on every
                    // other environment (the helper short-circuits
                    // when HYPRLAND_INSTANCE_SIGNATURE is unset).
                    #[cfg(all(unix, not(target_os = "macos")))]
                    tempo::hypr::hide_window();
                    close_handle
                        .update(cx, |app, _window, cx| {
                            app.on_window_close_intercepted(cx);
                        })
                        .ok();
                    false
                });
                // Stash the window handle on the app so the tray,
                // MPRIS Raise, and global hotkeys can route to a
                // single `focus_main_window` helper.
                app.set_main_window(window_handle);
            })
            .ok();

        cx.bind_keys([
            KeyBinding::new("enter", PlaySelected, None),
            KeyBinding::new("space", TogglePause, None),
            KeyBinding::new("left", MoveSelectionUp, None),
            KeyBinding::new("right", MoveSelectionDown, None),
            KeyBinding::new("ctrl-t", NewTab, None),
            KeyBinding::new("ctrl-w", CloseTab, None),
            KeyBinding::new("ctrl-shift-w", CloseAllTabs, None),
            KeyBinding::new("ctrl-shift-t", ReopenClosedTab, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
            KeyBinding::new("ctrl-1", SelectTab1, None),
            KeyBinding::new("ctrl-2", SelectTab2, None),
            KeyBinding::new("ctrl-3", SelectTab3, None),
            KeyBinding::new("ctrl-4", SelectTab4, None),
            KeyBinding::new("ctrl-5", SelectTab5, None),
            KeyBinding::new("ctrl-6", SelectTab6, None),
            KeyBinding::new("ctrl-7", SelectTab7, None),
            KeyBinding::new("ctrl-8", SelectTab8, None),
            KeyBinding::new("ctrl-9", SelectTab9, None),
            KeyBinding::new("ctrl-0", SelectTab10, None),
            KeyBinding::new("ctrl-f", FocusSearch, None),
            KeyBinding::new("ctrl-s", OpenSettings, None),
            KeyBinding::new("ctrl-r", PlayRandomTrack, None),
            KeyBinding::new("/", FocusSearch, None),
            KeyBinding::new("alt-left", NavigateBack, None),
            KeyBinding::new("alt-right", NavigateForward, None),
            KeyBinding::new("ctrl-m", ToggleMiniPlayer, None),
            KeyBinding::new("ctrl-shift-m", CycleMiniPlayer, None),
            KeyBinding::new("ctrl-h", HideWindow, None),
        ]);

        cx.activate(true);
    });
}
