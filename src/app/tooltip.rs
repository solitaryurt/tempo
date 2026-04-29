use super::*;

impl TempoApp {
    pub(crate) fn with_tooltip(
        &self,
        element: gpui::Stateful<gpui::Div>,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        self.with_tooltip_delay(element, id, label, Duration::ZERO, cx)
    }

    pub(crate) fn with_tooltip_delay(
        &self,
        element: gpui::Stateful<gpui::Div>,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        delay: Duration,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();
        let label = label.into();

        element.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
            if *hovered {
                this.schedule_tooltip(
                    id.clone(),
                    label.clone(),
                    delay,
                    window.mouse_position(),
                    cx,
                );
            } else {
                this.hide_tooltip(&id, cx);
            }
        }))
    }

    fn schedule_tooltip(
        &mut self,
        id: SharedString,
        label: SharedString,
        delay: Duration,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.tooltip_generation = self.tooltip_generation.wrapping_add(1);
        let generation = self.tooltip_generation;
        self.hovered_tooltip_id = Some(id.clone());

        if delay.is_zero() {
            self.tooltip = Some(Tooltip {
                id,
                label,
                position,
            });
            cx.notify();
            return;
        }

        self.tooltip = None;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;

            let Ok(()) = this.update(cx, |app, cx| {
                if app.tooltip_generation != generation
                    || app.hovered_tooltip_id.as_ref() != Some(&id)
                {
                    return;
                }

                app.tooltip = Some(Tooltip {
                    id,
                    label,
                    position,
                });
                cx.notify();
            }) else {
                return;
            };
        })
        .detach();
    }

    fn hide_tooltip(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if self.hovered_tooltip_id.as_ref() != Some(id) {
            return;
        }

        self.tooltip_generation = self.tooltip_generation.wrapping_add(1);
        self.hovered_tooltip_id = None;
        if self
            .tooltip
            .as_ref()
            .is_some_and(|tooltip| &tooltip.id == id)
        {
            self.tooltip = None;
            cx.notify();
        }
    }

    pub(super) fn render_tooltip(&self, tooltip: &Tooltip) -> impl IntoElement + use<> {
        let colors = *self.colors();

        anchored()
            .position(tooltip.position)
            .offset(point(px(12.0), px(18.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .id(SharedString::from(format!("tooltip-{}", tooltip.id)))
                    .max_w(px(260.0))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.border_strong))
                    .bg(rgb(colors.elevated))
                    .shadow_lg()
                    .text_xs()
                    .text_color(rgb(colors.text_strong))
                    .child(tooltip.label.clone()),
            )
    }
}
