use super::*;

impl TempoApp {
    pub(super) fn menu_at(
        &self,
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

    pub(super) fn menu_panel(&self, width: f32) -> gpui::Div {
        let colors = *self.colors();

        div()
            .w(px(width))
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .bg(rgb(colors.elevated))
            .shadow_lg()
            .overflow_hidden()
    }

    pub(super) fn menu_header(&self, title: impl Into<SharedString>) -> gpui::Div {
        let colors = *self.colors();

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

    pub(super) fn menu_header_with_subtitle(
        &self,
        title: impl Into<SharedString>,
        subtitle: impl Into<SharedString>,
    ) -> gpui::Div {
        let colors = *self.colors();

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

    pub(super) fn menu_section_label(&self, label: &'static str) -> gpui::Div {
        let colors = *self.colors();

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

    pub(super) fn menu_item_base(&self, id: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();
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

    pub(super) fn menu_item(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        self.menu_item_base(id).child(label.into())
    }
}
