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
        NavigateForward
    ]
);

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("tempo".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| app::TempoApp::new(window, cx)),
        )
        .expect("failed to open Tempo window");

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
        ]);

        cx.activate(true);
    });
}
