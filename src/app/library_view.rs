use super::*;

impl TempoApp {
    pub(super) fn render_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Library => div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(self.render_library(cx))
                .child(self.render_queue(cx))
                .into_any_element(),
            Page::Settings => self.render_settings(cx).into_any_element(),
        }
    }

    pub(super) fn render_library(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(0x131419))
            .child(self.render_library_header(cx))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .child(self.render_table(cx))
            .when(
                self.show_scan_errors && !self.scan_errors.is_empty(),
                |this| this.child(self.render_scan_errors_popover(cx)),
            )
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .h(px(34.0))
            .flex_none()
            .px_3()
            .border_b_1()
            .border_color(rgb(0x24252b))
            .bg(rgb(0x111216))
            .flex()
            .items_end()
            .gap_1()
            .children(
                self.tabs
                    .iter()
                    .enumerate()
                    .map(|(ix, tab)| self.render_tab(ix, tab, cx)),
            )
            .child(
                div()
                    .id("new-tab-button")
                    .mb(px(5.0))
                    .w(px(24.0))
                    .h(px(22.0))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x30323a))
                    .bg(rgb(0x18191f))
                    .text_color(rgb(0x9a9ea8))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_library_tab();
                        window.focus(&this.search_focus_handle);
                        cx.notify();
                    })),
            )
    }

    pub(super) fn render_tab(
        &self,
        ix: usize,
        tab: &BrowseTab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = ix == self.active_tab;
        let bg = if active { 0x1b1c22 } else { 0x15161a };
        let fg = if active { 0xf0f0f4 } else { 0x9a9ea8 };
        let border = if active { 0x3a3d45 } else { 0x24252b };

        div()
            .id(SharedString::from(format!("browse-tab-{ix}")))
            .max_w(px(210.0))
            .h(px(28.0))
            .px_3()
            .rounded_t_md()
            .border_1()
            .border_b_0()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .active(|this| this.opacity(0.82))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.tab_title(tab)),
            )
            .when(!tab.search_query.trim().is_empty(), |this| {
                this.child(div().text_xs().text_color(rgb(0xeeb17d)).child("search"))
            })
            .when(self.tabs.len() > 1, |this| {
                this.child(
                    div()
                        .id(SharedString::from(format!("close-tab-{ix}")))
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded_sm()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(0x777b84))
                        .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf0f0f4)))
                        .child("x")
                        .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                            this.close_tab(ix);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
            .when_some(
                match tab.source {
                    TabSource::Playlist(playlist_ix) => Some(playlist_ix),
                    TabSource::Library => None,
                },
                |this, playlist_ix| {
                    this.on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                        this.add_track_to_playlist(drag.track_ix, playlist_ix);
                        cx.notify();
                    }))
                },
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.active_tab = ix;
                this.open_page(Page::Library);
                cx.notify();
            }))
    }

    pub(super) fn render_library_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(54.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(rgb(0x24252b))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(self.left_sidebar_collapsed, |this| {
                        this.child(Self::sidebar_button("›", "open-left-sidebar").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.left_sidebar_collapsed = false;
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xf0f0f4))
                            .child(self.tab_title(self.active_tab())),
                    ),
            )
            .child(self.render_scan_status(cx))
            .child(div().flex_1())
            .child(
                div()
                    .id("library-search")
                    .w(px(180.0))
                    .h(px(26.0))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x30323a))
                    .bg(rgb(0x18191f))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_xs()
                    .text_color(rgb(if self.active_search_query().is_empty() {
                        0x737781
                    } else {
                        0xd8d8dd
                    }))
                    .track_focus(&self.search_focus_handle)
                    .on_click(cx.listener(|this, _, window, _cx| {
                        window.focus(&this.search_focus_handle);
                    }))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                        this.handle_search_key_down(event, cx);
                    }))
                    .child(if self.active_search_query().is_empty() {
                        "⌕  Search library".into()
                    } else {
                        format!("⌕  {}", self.active_search_query())
                    }),
            )
            .child(
                Self::sidebar_button("⚙", "open-settings").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.open_page(Page::Settings);
                        cx.notify();
                    },
                )),
            )
            .when(
                self.right_sidebar_collapsed && !self.queue.is_empty(),
                |this| {
                    this.child(Self::sidebar_button("‹", "open-right-sidebar").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.right_sidebar_collapsed = false;
                            cx.notify();
                        }),
                    ))
                },
            )
    }

    pub(super) fn render_scan_status(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .text_xs()
            .text_color(rgb(if self.is_scanning { 0xeeb17d } else { 0x676b74 }))
            .flex()
            .items_center()
            .gap_1()
            .child(self.visible_scan_status_without_errors())
            .when(self.scan_progress.errors > 0, |this| {
                let label = format!(
                    "{} {}",
                    self.scan_progress.errors,
                    if self.scan_progress.errors == 1 {
                        "error"
                    } else {
                        "errors"
                    }
                );

                this.child(
                    div()
                        .id("scan-errors-toggle")
                        .rounded_sm()
                        .px_1()
                        .cursor_pointer()
                        .text_color(rgb(0xeeb17d))
                        .hover(|this| this.bg(rgb(0x282a30)).text_color(rgb(0xf2c693)))
                        .child(label)
                        .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.show_scan_errors = !this.show_scan_errors;
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
    }

    pub(super) fn render_scan_errors_popover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let errors = self
            .scan_errors
            .iter()
            .rev()
            .take(10)
            .map(|error| {
                div()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x282a30))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(rgb(0xf0f0f4))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(error.path.display().to_string()),
                    )
                    .child(div().text_color(rgb(0xa1a5af)).child(error.message.clone()))
            })
            .collect::<Vec<_>>();
        let hidden_count = self.scan_errors.len().saturating_sub(errors.len());

        div()
            .absolute()
            .top(px(46.0))
            .right(px(16.0))
            .w(px(420.0))
            .max_h(px(360.0))
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343741))
            .bg(rgb(0x1b1c22))
            .shadow_lg()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xf0f0f4))
                            .child("Scan errors"),
                    )
                    .child(div().flex_1())
                    .child(
                        Self::sidebar_button("x", "close-scan-errors").on_click(cx.listener(
                            |this, _event: &ClickEvent, _window, cx| {
                                this.show_scan_errors = false;
                                cx.stop_propagation();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x8a8e97))
                    .child("Most recent errors are shown first."),
            )
            .children(errors)
            .when(hidden_count > 0, |this| {
                this.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(rgb(0x8a8e97))
                        .child(format!("+{hidden_count} older errors")),
                )
            })
    }

    pub(super) fn format_library_size(tracks: &[Track]) -> String {
        let bytes = tracks.iter().map(|track| track.file_size).sum::<u64>();
        if bytes >= 1_000_000_000 {
            format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
        } else if bytes >= 1_000_000 {
            format!("{:.1} MB", bytes as f64 / 1_000_000.0)
        } else {
            format!("{} KB", bytes / 1_000)
        }
    }
}
