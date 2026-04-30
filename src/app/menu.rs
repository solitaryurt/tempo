//! Floating menu/dropdown layout helpers.
//!
//! These are free functions (rather than methods on [`TempoApp`])
//! because the player bar — which lives in its own [`PlayerEntity`]
//! after the Phase 3 #16 split — also renders dropdowns (the output
//! device picker). Both entities call into these helpers without
//! borrowing each other.
//!
//! All helpers take [`ThemeColors`] explicitly (it's `Copy`, so this
//! is free) so the layout stays a pure function of inputs and a
//! caller without a `TempoApp` handle (e.g. tests, future
//! sub-entities) can produce identical menus.

use super::*;

pub(in crate::app) fn menu_at(
    position: Point<Pixels>,
    anchor: Corner,
    offset: Point<Pixels>,
    panel: impl IntoElement,
) -> gpui::Anchored {
    anchored()
        .position(position)
        .anchor(anchor)
        .offset(offset)
        .snap_to_window_with_margin(px(8.0))
        .child(panel)
}

pub(in crate::app) fn menu_panel(width: f32, colors: ThemeColors) -> gpui::Stateful<gpui::Div> {
    div()
        .id("menu-panel")
        .w(px(width))
        .rounded_md()
        .border_1()
        .border_color(rgb(colors.border_strong))
        .bg(rgb(colors.elevated))
        .shadow_lg()
        .overflow_hidden()
        // Swallow mouse-down inside the panel so app-level dismiss
        // handlers don't close the menu before its items receive the
        // click, and so clicks don't leak through to elements behind
        // the floating panel (e.g. the seekbar).
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Right, |_, _, cx| cx.stop_propagation())
}

pub(in crate::app) fn menu_header(
    title: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(rgb(colors.border))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(colors.text_strong))
        .overflow_hidden()
        .text_ellipsis()
        .child(title.into())
}

pub(in crate::app) fn menu_header_with_subtitle(
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Div {
    div()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(rgb(colors.border))
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(colors.text_strong))
                .child(title.into()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(colors.text_muted))
                .overflow_hidden()
                .text_ellipsis()
                .child(subtitle.into()),
        )
}

pub(in crate::app) fn menu_section_label(label: &'static str, colors: ThemeColors) -> gpui::Div {
    div()
        .mt_1()
        .px_3()
        .pt_2()
        .pb_1()
        .border_t_1()
        .border_color(rgb(colors.border))
        .text_xs()
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(rgb(colors.text_faint))
        .child(label)
}

pub(in crate::app) fn menu_item_base(
    id: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    let id = id.into();

    div()
        .id(id)
        .h(px(28.0))
        .px_3()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(rgb(colors.text))
        .hover(move |this| {
            this.bg(rgb(colors.button_hover))
                .text_color(rgb(colors.text_strong))
        })
}

pub(in crate::app) fn menu_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    colors: ThemeColors,
) -> gpui::Stateful<gpui::Div> {
    menu_item_base(id, colors).child(label.into())
}
