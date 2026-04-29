use super::*;

impl TempoApp {
    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.page {
            Page::Library => div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(self.render_library(window, cx))
                .child(self.render_queue(cx))
                .into_any_element(),
            Page::Artists => self.render_artists_page(window, cx).into_any_element(),
            Page::Albums => self.render_albums_page(window, cx).into_any_element(),
            Page::PlaybackHistory => self.render_playback_history_page(cx).into_any_element(),
            Page::ScanErrors => self.render_scan_errors_page(cx).into_any_element(),
            Page::Settings => self.render_settings(cx).into_any_element(),
        }
    }

    pub(super) fn render_library(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .relative()
            .bg(rgb(colors.surface))
            .child(self.render_library_header(window, cx))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .when_some(self.render_detail_hero(cx), |this, hero| this.child(hero))
            .child(self.render_table(cx))
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .h(px(34.0))
            .flex_none()
            .px_3()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.app))
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
                    .border_color(rgb(colors.waveform_border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text_muted))
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
        let colors = *self.colors();
        let bg = if active {
            colors.elevated
        } else {
            colors.panel_alt
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let border = if active {
            colors.border_strong
        } else {
            colors.border
        };

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
                this.child(
                    div()
                        .text_xs()
                        .text_color(rgb(colors.accent))
                        .child("search"),
                )
            })
            .when(self.can_close_tab(ix), |this| {
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
                        .text_color(rgb(colors.text_faint))
                        .hover(move |this| {
                            this.bg(rgb(colors.button_hover))
                                .text_color(rgb(colors.text_strong))
                        })
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
                    TabSource::Library | TabSource::Artist(_) | TabSource::Album(_) => None,
                },
                |this, playlist_ix| {
                    this.on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                        this.add_track_to_playlist(drag.track_ix, playlist_ix);
                        cx.notify();
                    }))
                },
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_tab(ix);
                cx.notify();
            }))
    }

    pub(super) fn render_search_box(
        &self,
        window: &Window,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let query = self.search_input.text();
        let is_focused = self.search_focus_handle.is_focused(window);
        let children = self.render_search_text_children(placeholder, is_focused);

        div()
            .id("library-search")
            .w(px(180.0))
            .h(px(26.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.waveform_border))
            .bg(rgb(colors.button))
            .px_3()
            .flex()
            .items_center()
            .overflow_hidden()
            .text_xs()
            .cursor_pointer()
            .text_color(rgb(if query.is_empty() {
                colors.text_faint
            } else {
                colors.text
            }))
            .track_focus(&self.search_focus_handle)
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.search_focus_handle);
                this.search_input.move_to_end();
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_search_key_down(event, cx);
            }))
            .children(children)
    }

    fn render_search_text_children(
        &self,
        placeholder: &'static str,
        is_focused: bool,
    ) -> Vec<AnyElement> {
        let colors = *self.colors();
        let query = self.search_input.text();
        let mut children = vec![div().flex_none().child("⌕  ").into_any_element()];

        if query.is_empty() {
            if is_focused {
                children.push(self.render_search_cursor().into_any_element());
            } else {
                children.push(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(placeholder.to_string())
                        .into_any_element(),
                );
            }
            return children;
        }

        if !is_focused {
            children.push(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(query.to_string())
                    .into_any_element(),
            );
            return children;
        }

        if let Some(selection) = self.search_input.selection_range() {
            if selection.start > 0 {
                children.push(
                    div()
                        .flex_none()
                        .child(query[..selection.start].to_string())
                        .into_any_element(),
                );
            }
            children.push(
                div()
                    .flex_none()
                    .rounded_sm()
                    .bg(rgb(colors.selected))
                    .text_color(rgb(colors.text_strong))
                    .child(query[selection.clone()].to_string())
                    .into_any_element(),
            );
            if selection.end < query.len() {
                children.push(
                    div()
                        .flex_none()
                        .child(query[selection.end..].to_string())
                        .into_any_element(),
                );
            }
        } else {
            let cursor = self.search_input.cursor().min(query.len());
            if cursor > 0 {
                children.push(
                    div()
                        .flex_none()
                        .child(query[..cursor].to_string())
                        .into_any_element(),
                );
            }
            children.push(self.render_search_cursor().into_any_element());
            if cursor < query.len() {
                children.push(
                    div()
                        .flex_none()
                        .child(query[cursor..].to_string())
                        .into_any_element(),
                );
            }
        }

        children
    }

    fn render_search_cursor(&self) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .flex_none()
            .ml(px(1.0))
            .w(px(1.0))
            .h(px(14.0))
            .bg(rgb(colors.text))
            .with_animation(
                "search-cursor",
                Animation::new(Duration::from_millis(1000)).repeat(),
                |this, delta| this.opacity(if delta < 0.5 { 1.0 } else { 0.0 }),
            )
    }

    pub(super) fn render_library_header(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .h(px(54.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_b_1()
            .border_color(rgb(colors.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(self.left_sidebar_collapsed, |this| {
                        this.child(self.sidebar_button("›", "open-left-sidebar").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.left_sidebar_collapsed = false;
                                cx.notify();
                            }),
                        ))
                    })
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .child(self.tab_title(self.active_tab())),
                    ),
            )
            .child(self.render_scan_status(cx))
            .child(div().flex_1())
            .child(self.render_search_box(window, "Search library", cx))
            .child(
                self.with_tooltip(
                    self.sidebar_button("⚙", "open-settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_page(Page::Settings);
                            cx.notify();
                        })),
                    "open-settings-tooltip",
                    "Settings",
                    cx,
                ),
            )
            .when(
                self.right_sidebar_collapsed && !self.queue.is_empty(),
                |this| {
                    this.child(self.sidebar_button("‹", "open-right-sidebar").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.right_sidebar_collapsed = false;
                            cx.notify();
                        }),
                    ))
                },
            )
    }

    pub(super) fn render_scan_status(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .text_xs()
            .text_color(rgb(if self.is_scanning {
                colors.accent
            } else {
                colors.text_faint
            }))
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
                        .text_color(rgb(colors.accent))
                        .hover(move |this| {
                            this.bg(rgb(colors.button_hover))
                                .text_color(rgb(colors.accent_soft))
                        })
                        .child(label)
                        .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                            this.open_page(Page::ScanErrors);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
            })
    }

    pub(super) fn render_scan_errors_page(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let item_count = self.scan_errors.len();
        let subtitle = if self.scan_errors.is_empty() {
            "No scan errors".to_string()
        } else {
            format!(
                "{} {} from the current scan",
                self.scan_errors.len(),
                if self.scan_errors.len() == 1 {
                    "error"
                } else {
                    "errors"
                }
            )
        };

        div()
            .id("scan-errors-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_simple_page_header("Scan Errors", subtitle))
            .child(
                div()
                    .id("scan-errors-scroll")
                    .flex_1()
                    .min_h_0()
                    .child(self.render_scan_errors_table(item_count, cx)),
            )
    }

    pub(super) fn render_simple_page_header(
        &self,
        title: &'static str,
        subtitle: String,
    ) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .h(px(74.0))
            .flex_none()
            .px_4()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.app))
            .flex()
            .flex_col()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .child(title),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_muted))
                    .child(subtitle),
            )
    }

    fn render_scan_errors_table(
        &self,
        item_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let scroll_handle = self.scan_errors_scroll_handle.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.render_resizable_table_header(
                27.0,
                &[
                    ColumnResizeTarget::ScanError(ScanErrorColumn::Index),
                    ColumnResizeTarget::ScanError(ScanErrorColumn::Path),
                    ColumnResizeTarget::ScanError(ScanErrorColumn::Error),
                ],
                cx,
            ))
            .when(self.scan_errors.is_empty(), |this| {
                this.child(
                    div()
                        .p_5()
                        .text_color(rgb(colors.text_muted))
                        .child("No scan errors for the current scan."),
                )
            })
            .when(!self.scan_errors.is_empty(), |this| {
                this.child(
                    uniform_list(
                        "scan-errors-rows",
                        item_count,
                        cx.processor(move |this, range: Range<usize>, _window, _cx| {
                            let item_count = this.scan_errors.len();

                            range
                                .filter_map(|row_ix| {
                                    let error_ix = item_count.checked_sub(row_ix + 1)?;
                                    let error = this.scan_errors.get(error_ix)?;
                                    Some(
                                        this.render_scan_error_row(row_ix, error)
                                            .into_any_element(),
                                    )
                                })
                                .collect()
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .track_scroll(scroll_handle),
                )
            })
    }

    fn render_scan_error_row(&self, ix: usize, error: &IndexingError) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .min_h(px(TABLE_ROW_H))
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .bg(rgb(if ix.is_multiple_of(2) {
                colors.row
            } else {
                colors.surface
            }))
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .w(px(self.scan_error_column_width(ScanErrorColumn::Index)))
                    .flex_none()
                    .text_color(rgb(colors.text_faint))
                    .child((ix + 1).to_string()),
            )
            .child(
                div()
                    .w(px(self.scan_error_column_width(ScanErrorColumn::Path)))
                    .flex_none()
                    .text_color(rgb(colors.text_strong))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(error.path.display().to_string()),
            )
            .child(
                div()
                    .w(px(self.scan_error_column_width(ScanErrorColumn::Error)))
                    .min_w_0()
                    .text_color(rgb(colors.text_muted))
                    .child(error.message.clone()),
            )
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
