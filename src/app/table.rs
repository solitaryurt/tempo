use super::*;

impl TempoApp {
    pub(super) fn column_width(&self, column: TableColumn) -> f32 {
        match column {
            TableColumn::Index => self.column_widths.index,
            TableColumn::Artwork => self.column_widths.artwork,
            TableColumn::Title => self.column_widths.title,
            TableColumn::Album => self.column_widths.album,
            TableColumn::Format => self.column_widths.format,
            TableColumn::Plays => self.column_widths.plays,
            TableColumn::Duration => self.column_widths.duration,
            TableColumn::Loved => self.column_widths.loved,
        }
    }

    pub(super) fn set_column_width(&mut self, column: TableColumn, width: f32) {
        let width = width.max(Self::min_column_width(column));
        match column {
            TableColumn::Index => self.column_widths.index = width,
            TableColumn::Artwork => self.column_widths.artwork = width,
            TableColumn::Title => self.column_widths.title = width,
            TableColumn::Album => self.column_widths.album = width,
            TableColumn::Format => self.column_widths.format = width,
            TableColumn::Plays => self.column_widths.plays = width,
            TableColumn::Duration => self.column_widths.duration = width,
            TableColumn::Loved => self.column_widths.loved = width,
        }
    }

    pub(super) fn min_column_width(column: TableColumn) -> f32 {
        match column {
            TableColumn::Index | TableColumn::Artwork | TableColumn::Loved => 24.0,
            TableColumn::Format => 44.0,
            TableColumn::Plays | TableColumn::Duration => 52.0,
            TableColumn::Title | TableColumn::Album => 96.0,
        }
    }

    pub(super) fn begin_column_resize(&mut self, column: TableColumn, event: &MouseDownEvent) {
        self.column_resize = Some(ColumnResize {
            column,
            start_x: f32::from(event.position.x),
            start_width: self.column_width(column),
        });
        self.context_menu_track = None;
    }

    pub(super) fn resize_column_from_mouse(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(resize) = self.column_resize else {
            return false;
        };

        let delta = f32::from(event.position.x) - resize.start_x;
        self.set_column_width(resize.column, resize.start_width + delta);
        true
    }

    pub(super) fn finish_column_resize(&mut self) -> bool {
        self.column_resize.take().is_some()
    }

    pub(super) fn handle_table_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page != Page::Library || !self.focus_handle.is_focused(window) {
            return;
        }

        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return;
        }

        let handled = match event.keystroke.key.as_str() {
            "pageup" => {
                self.move_selection_by_page(-1);
                true
            }
            "pagedown" => {
                self.move_selection_by_page(1);
                true
            }
            "home" => {
                self.select_table_edge(false);
                true
            }
            "end" => {
                self.select_table_edge(true);
                true
            }
            _ => Self::album_jump_key(event)
                .map(|jump_key| self.scroll_to_album_initial(jump_key))
                .unwrap_or(false),
        };

        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        self.move_selection_by_rows(delta, ScrollStrategy::Center);
    }

    pub(super) fn move_selection_by_page(&mut self, direction: isize) {
        let rows = self.table_page_row_count() as isize;
        self.move_selection_by_rows(direction * rows, ScrollStrategy::Center);
    }

    pub(super) fn move_selection_by_rows(&mut self, delta: isize, strategy: ScrollStrategy) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let selected_track = self.active_selected_track();
        let Some(position) = indices.iter().position(|ix| *ix == selected_track) else {
            return;
        };
        let next = (position as isize + delta).clamp(0, indices.len().saturating_sub(1) as isize);
        self.select_table_row(next as usize, strategy);
    }

    pub(super) fn select_table_edge(&mut self, end: bool) {
        let Some(last_row) = self.current_track_indices().len().checked_sub(1) else {
            return;
        };

        if end {
            self.select_table_row(last_row, ScrollStrategy::Bottom);
        } else {
            self.select_table_row(0, ScrollStrategy::Top);
        }
    }

    pub(super) fn select_table_row(&mut self, row_ix: usize, strategy: ScrollStrategy) {
        let Some(track_ix) = self.current_track_indices().get(row_ix).copied() else {
            return;
        };

        self.set_active_selected_track(track_ix);
        self.active_tab()
            .table_scroll_handle
            .scroll_to_item(row_ix, strategy);
        self.context_menu_track = None;
    }

    pub(super) fn table_page_row_count(&self) -> usize {
        self.table_scrollbar_metrics()
            .map(|metrics| (metrics.track_height / TABLE_ROW_H).floor() as usize)
            .unwrap_or(12)
            .saturating_sub(1)
            .max(1)
    }

    pub(super) fn album_jump_key(event: &KeyDownEvent) -> Option<char> {
        let key_char = event.keystroke.key_char.as_deref()?;
        let mut chars = key_char.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }

        ch.is_ascii_alphanumeric().then(|| ch.to_ascii_uppercase())
    }

    pub(super) fn scroll_to_album_initial(&mut self, jump_key: char) -> bool {
        let Some(row_ix) = self.current_track_indices().iter().position(|track_ix| {
            self.tracks
                .get(*track_ix)
                .is_some_and(|track| Self::album_initial_matches(&track.album, jump_key))
        }) else {
            return false;
        };

        self.select_table_row(row_ix, ScrollStrategy::Top);
        true
    }

    pub(super) fn album_initial_matches(album: &str, jump_key: char) -> bool {
        album
            .chars()
            .find(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_uppercase() == jump_key)
            .unwrap_or(false)
    }

    pub(super) fn table_scrollbar_metrics(&self) -> Option<TableScrollbarMetrics> {
        let handle = self.active_tab().table_scroll_handle.clone();
        let (base_handle, measured) = {
            let state = handle.0.borrow();
            (state.base_handle.clone(), state.last_item_size.is_some())
        };

        if !measured {
            return None;
        }

        let bounds = base_handle.bounds();
        let viewport_height = f32::from(bounds.size.height);
        if viewport_height <= 0.0 {
            return None;
        }

        let max_scroll = f32::from(base_handle.max_offset().height).max(0.0);
        let content_height = viewport_height + max_scroll;
        let track_height = (viewport_height - TABLE_SCROLLBAR_MARGIN * 2.0).max(1.0);
        let thumb_height = if content_height <= 0.0 {
            track_height
        } else {
            ((viewport_height / content_height) * track_height)
                .max(TABLE_SCROLLBAR_MIN_THUMB_H)
                .min(track_height)
        };
        let thumb_travel = (track_height - thumb_height).max(0.0);
        let scroll_top = (-f32::from(base_handle.offset().y)).clamp(0.0, max_scroll);
        let thumb_top = if max_scroll > 0.0 && thumb_travel > 0.0 {
            (scroll_top / max_scroll) * thumb_travel
        } else {
            0.0
        };

        Some(TableScrollbarMetrics {
            track_top: f32::from(bounds.origin.y) + TABLE_SCROLLBAR_MARGIN,
            track_height,
            thumb_top,
            thumb_height,
            max_scroll,
            scroll_top,
        })
    }

    pub(super) fn begin_table_scrollbar_drag(&mut self, event: &MouseDownEvent) -> bool {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let local_y = f32::from(event.position.y) - metrics.track_top;
        let thumb_bottom = metrics.thumb_top + metrics.thumb_height;
        let thumb_offset = if (metrics.thumb_top..=thumb_bottom).contains(&local_y) {
            local_y - metrics.thumb_top
        } else {
            metrics.thumb_height / 2.0
        };

        self.table_scrollbar_drag = Some(TableScrollbarDrag { thumb_offset });
        self.scroll_table_to_scrollbar_y(event.position.y, thumb_offset);
        true
    }

    pub(super) fn drag_table_scrollbar(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(drag) = self.table_scrollbar_drag else {
            return false;
        };

        if !event.dragging() {
            self.table_scrollbar_drag = None;
            return false;
        }

        self.scroll_table_to_scrollbar_y(event.position.y, drag.thumb_offset)
    }

    pub(super) fn finish_table_scrollbar_drag(&mut self) -> bool {
        self.table_scrollbar_drag.take().is_some()
    }

    pub(super) fn finish_table_drag_interactions(&mut self) -> bool {
        let scrolled = self.finish_table_scrollbar_drag();
        let resized = self.finish_column_resize();
        scrolled || resized
    }

    pub(super) fn mark_table_scrolling(&mut self, cx: &mut Context<Self>) {
        self.table_scroll_generation = self.table_scroll_generation.wrapping_add(1);

        if self.table_is_scrolling {
            return;
        }

        self.table_is_scrolling = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            loop {
                let Ok(generation) = this.update(cx, |app, _cx| app.table_scroll_generation) else {
                    return;
                };

                cx.background_executor()
                    .timer(TABLE_SCROLL_IDLE_DELAY)
                    .await;

                let Ok(should_stop) = this.update(cx, |app, cx| {
                    if app.table_scroll_generation == generation && app.table_is_scrolling {
                        app.table_is_scrolling = false;
                        cx.notify();
                        true
                    } else {
                        false
                    }
                }) else {
                    return;
                };

                if should_stop {
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn scroll_table_to_scrollbar_y(
        &mut self,
        mouse_y: Pixels,
        thumb_offset: f32,
    ) -> bool {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let thumb_travel = (metrics.track_height - metrics.thumb_height).max(1.0);
        let thumb_top =
            (f32::from(mouse_y) - metrics.track_top - thumb_offset).clamp(0.0, thumb_travel);
        let ratio = thumb_top / thumb_travel;
        self.scroll_table_to_ratio(ratio)
    }

    pub(super) fn scroll_table_to_ratio(&mut self, ratio: f32) -> bool {
        let handle = self.active_tab().table_scroll_handle.clone();
        let base_handle = handle.0.borrow().base_handle.clone();
        let max_scroll = f32::from(base_handle.max_offset().height).max(0.0);
        if max_scroll <= 0.0 {
            return false;
        }

        let target_y = px(-(ratio.clamp(0.0, 1.0) * max_scroll));
        let current = base_handle.offset();
        if (f32::from(current.y) - f32::from(target_y)).abs() < 0.5 {
            return false;
        }

        base_handle.set_offset(point(current.x, target_y));
        true
    }

    pub(super) fn current_scrollbar_label(&self, metrics: TableScrollbarMetrics) -> Option<String> {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return None;
        }

        let ratio = if metrics.max_scroll > 0.0 {
            metrics.scroll_top / metrics.max_scroll
        } else {
            0.0
        };
        let row_ix =
            (ratio.clamp(0.0, 1.0) * indices.len().saturating_sub(1) as f32).round() as usize;
        let track_ix = *indices.get(row_ix)?;
        Some(self.scrollbar_marker_label(track_ix, self.active_tab().sort_column))
    }

    pub(super) fn fast_scroll_top_row(&self) -> (usize, f32) {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return (0, 0.0);
        };

        let scroll_top = metrics.scroll_top.max(0.0);
        let row = (scroll_top / TABLE_ROW_H).floor() as usize;
        let offset = -(scroll_top % TABLE_ROW_H);
        (row, offset)
    }

    pub(super) fn render_table(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let item_count = self.current_track_indices().len();
        let active_search_query = self.active_search_query().to_string();
        let has_no_search_results = item_count == 0
            && self.active_source_track_count() > 0
            && !active_search_query.trim().is_empty();
        let table_scroll_handle = self.active_tab().table_scroll_handle.clone();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| {
                    window.focus(&this.focus_handle);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let scrolled = this.drag_table_scrollbar(event);
                let resized = !scrolled && this.resize_column_from_mouse(event);
                if scrolled || resized {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_drag_interactions() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_drag_interactions() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(self.render_table_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when(self.table_scrollbar_drag.is_some(), |this| {
                        this.child(self.render_fast_scroll_rows())
                    })
                    .when(self.table_scrollbar_drag.is_none(), |this| {
                        this.child(
                            uniform_list(
                                "track-table-rows",
                                item_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    range
                                        .enumerate()
                                        .filter_map(|(visible_row_ix, row_ix)| {
                                            let track_ix = this
                                                .current_track_indices()
                                                .get(row_ix)
                                                .copied()?;
                                            Some(
                                                this.render_track_row(
                                                    visible_row_ix,
                                                    track_ix,
                                                    &this.tracks[track_ix],
                                                    this.table_is_scrolling,
                                                    cx,
                                                )
                                                .into_any_element(),
                                            )
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .on_scroll_wheel(cx.listener(
                                |this, _event: &ScrollWheelEvent, _window, cx| {
                                    this.mark_table_scrolling(cx);
                                },
                            ))
                            .track_scroll(table_scroll_handle),
                        )
                    })
                    .child(self.render_table_scrollbar(item_count, cx)),
            )
            .when(self.tracks.is_empty(), |this| {
                let colors = *self.colors();
                this.child(
                    div()
                        .absolute()
                        .top(px(104.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(colors.text_strong))
                                .child("No indexed audio yet"),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors.text_muted))
                                .child(self.library_status.clone()),
                        ),
                )
            })
            .when(has_no_search_results, |this| {
                let colors = *self.colors();
                this.child(
                    div()
                        .absolute()
                        .top(px(104.0))
                        .left(px(24.0))
                        .right(px(24.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.surface))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(colors.text_strong))
                                .child("No matching tracks"),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors.text_muted))
                                .child(format!("No tracks match \"{}\".", active_search_query)),
                        ),
                )
            })
            .when_some(
                self.context_menu_track
                    .filter(|track_ix| *track_ix < self.tracks.len()),
                |this, track_ix| this.child(self.render_context_menu(track_ix, cx)),
            )
    }

    pub(super) fn render_fast_scroll_rows(&self) -> AnyElement {
        let Some(metrics) = self.table_scrollbar_metrics() else {
            return div().size_full().into_any_element();
        };

        let indices = self.current_track_indices();
        if indices.is_empty() {
            return div().size_full().into_any_element();
        }

        let viewport_height = metrics.track_height + TABLE_SCROLLBAR_MARGIN * 2.0;
        let visible_rows = (viewport_height / TABLE_ROW_H).ceil() as usize + 2;
        let (top_row, row_offset) = self.fast_scroll_top_row();
        let start_row = top_row.saturating_sub(FAST_SCROLL_OVERSCAN_ROWS);
        let rows_to_render = visible_rows + FAST_SCROLL_OVERSCAN_ROWS * 2;
        let first_row_top = row_offset - ((top_row - start_row) as f32 * TABLE_ROW_H);
        let rows = (0..rows_to_render)
            .filter_map(|visible_ix| {
                let row_ix = start_row + visible_ix;
                let track_ix = *indices.get(row_ix)?;
                let top = first_row_top + visible_ix as f32 * TABLE_ROW_H;
                Some(self.render_fast_track_row(top, track_ix, &self.tracks[track_ix]))
            })
            .collect::<Vec<_>>();

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .overflow_hidden()
            .bg(rgb(self.colors().surface))
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_fast_track_row(
        &self,
        top: f32,
        track_ix: usize,
        track: &Track,
    ) -> AnyElement {
        let active = track_ix == self.playing_track;
        let selected = track_ix == self.active_selected_track();
        let colors = *self.colors();
        let bg = if selected {
            colors.selected
        } else if active {
            colors.playing
        } else {
            colors.row
        };
        let title_color = if active {
            colors.accent
        } else {
            colors.text_strong
        };

        div()
            .absolute()
            .top(px(top))
            .left_0()
            .right_0()
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .bg(rgb(bg))
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Index)))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(if active {
                        if self.is_playing { "Ⅱ" } else { "▶" }.into()
                    } else {
                        format!("{:02}", track_ix + 1)
                    }),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Artwork)))
                    .flex()
                    .items_center()
                    .child(self.album_tile_placeholder(track, 22.0)),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Title)))
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(title_color))
                    .child(track.title.clone()),
            )
            .child(self.cell(track.album.clone(), self.column_width(TableColumn::Album)))
            .child(self.cell(track.codec.clone(), self.column_width(TableColumn::Format)))
            .child(self.cell(track.plays.clone(), self.column_width(TableColumn::Plays)))
            .child(self.cell(
                track.duration.clone(),
                self.column_width(TableColumn::Duration),
            ))
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Loved)))
                    .text_color(rgb(colors.love))
                    .child(if track.loved { "♥" } else { "" }),
            )
            .into_any_element()
    }

    pub(super) fn render_table_scrollbar(
        &self,
        item_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if item_count == 0 {
            return div().into_any_element();
        }
        let colors = *self.colors();

        let metrics = self.table_scrollbar_metrics();
        let thumb_top = metrics.map_or(0.0, |metrics| metrics.thumb_top);
        let thumb_height =
            metrics.map_or(TABLE_SCROLLBAR_MIN_THUMB_H, |metrics| metrics.thumb_height);
        let track_height = metrics.map_or(0.0, |metrics| metrics.track_height);
        let scrollable = metrics.is_some_and(|metrics| metrics.max_scroll > 0.0);
        let current_label = metrics.and_then(|metrics| self.current_scrollbar_label(metrics));
        let max_markers = if track_height > 0.0 {
            ((track_height / 16.0).floor() as usize).clamp(2, TABLE_SCROLLBAR_MAX_MARKERS)
        } else {
            0
        };
        let marker_stride = self
            .active_tab()
            .scrollbar_markers
            .len()
            .saturating_add(max_markers.saturating_sub(1))
            / max_markers.max(1);
        let marker_stride = marker_stride.max(1);
        let markers = self
            .active_tab()
            .scrollbar_markers
            .iter()
            .enumerate()
            .filter_map(|(ix, marker)| {
                if ix % marker_stride != 0 {
                    return None;
                }

                let top = TABLE_SCROLLBAR_MARGIN + marker.ratio.clamp(0.0, 1.0) * track_height;
                Some(
                    div()
                        .absolute()
                        .top(px((top - 7.0).max(TABLE_SCROLLBAR_MARGIN)))
                        .right(px(14.0))
                        .w(px(30.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_xs()
                        .text_color(rgb(colors.text_faint))
                        .child(marker.label.clone())
                        .into_any_element(),
                )
            })
            .collect::<Vec<_>>();
        let current_label_top = (TABLE_SCROLLBAR_MARGIN + thumb_top + thumb_height / 2.0 - 9.0)
            .clamp(
                TABLE_SCROLLBAR_MARGIN,
                (track_height - 18.0).max(TABLE_SCROLLBAR_MARGIN),
            );
        let hints_opacity = if self.table_scrollbar_drag.is_some() {
            1.0
        } else {
            0.0
        };

        div()
            .id("table-scrollbar")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(TABLE_SCROLLBAR_W))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if this.begin_table_scrollbar_drag(event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.drag_table_scrollbar(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_table_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(TABLE_SCROLLBAR_W))
                    .opacity(hints_opacity)
                    .hover(|this| this.opacity(1.0))
                    .children(markers)
                    .when(scrollable, |this| {
                        this.when_some(current_label, |this, label| {
                            this.child(
                                div()
                                    .absolute()
                                    .top(px(current_label_top))
                                    .right(px(15.0))
                                    .max_w(px(36.0))
                                    .h(px(18.0))
                                    .px_1()
                                    .rounded_sm()
                                    .bg(rgb(colors.playing))
                                    .border_1()
                                    .border_color(rgb(colors.border_strong))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(rgb(colors.text))
                                    .overflow_hidden()
                                    .child(label),
                            )
                        })
                    }),
            )
            .child(
                div()
                    .absolute()
                    .top(px(TABLE_SCROLLBAR_MARGIN))
                    .right(px(4.0))
                    .bottom(px(TABLE_SCROLLBAR_MARGIN))
                    .w(px(TABLE_SCROLLBAR_TRACK_W))
                    .rounded_full()
                    .bg(rgb(colors.elevated))
                    .opacity(if scrollable { 0.95 } else { 0.45 })
                    .child(
                        div()
                            .absolute()
                            .top(px(thumb_top))
                            .left(px(1.0))
                            .right(px(1.0))
                            .h(px(thumb_height))
                            .rounded_full()
                            .bg(rgb(if self.table_scrollbar_drag.is_some() {
                                colors.text
                            } else {
                                colors.text_faint
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_table_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .h(px(27.0))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(colors.border))
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(colors.text_faint))
            .child(self.header_cell("#", TableColumn::Index, Some(SortColumn::Index), cx))
            .child(self.header_cell("", TableColumn::Artwork, None, cx))
            .child(self.header_cell("TITLE", TableColumn::Title, Some(SortColumn::Title), cx))
            .child(self.header_cell("ALBUM", TableColumn::Album, Some(SortColumn::Album), cx))
            .child(self.header_cell("FMT", TableColumn::Format, Some(SortColumn::Format), cx))
            .child(self.header_cell("PLAYS", TableColumn::Plays, Some(SortColumn::Plays), cx))
            .child(self.header_cell(
                "TIME",
                TableColumn::Duration,
                Some(SortColumn::Duration),
                cx,
            ))
            .child(self.header_cell("", TableColumn::Loved, None, cx))
    }

    pub(super) fn header_cell(
        &self,
        label: &'static str,
        column: TableColumn,
        sort_column: Option<SortColumn>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let width = self.column_width(column);
        let tab = self.active_tab();
        let active = sort_column.is_some_and(|column| tab.sort_column == column);
        let colors = *self.colors();
        let icon = match tab.sort_direction {
            SortDirection::Ascending => "▲",
            SortDirection::Descending => "▼",
        };
        let id = match column {
            TableColumn::Index => "column-index",
            TableColumn::Artwork => "column-artwork",
            TableColumn::Title => "column-title",
            TableColumn::Album => "column-album",
            TableColumn::Format => "column-format",
            TableColumn::Plays => "column-plays",
            TableColumn::Duration => "column-duration",
            TableColumn::Loved => "column-loved",
        };

        div()
            .id(id)
            .relative()
            .h_full()
            .w(px(width))
            .flex()
            .items_center()
            .gap_1()
            .text_color(rgb(if active {
                colors.text
            } else {
                colors.text_faint
            }))
            .when(sort_column.is_some(), |this| {
                this.cursor_pointer()
                    .hover(move |this| this.text_color(rgb(colors.text)))
            })
            .child(label)
            .when(active, |this| this.child(icon))
            .when_some(sort_column, |this, sort_column| {
                this.on_click(cx.listener(move |this, _, _, cx| {
                    let tab = this.active_tab_mut();
                    if tab.sort_column == sort_column {
                        tab.sort_direction = match tab.sort_direction {
                            SortDirection::Ascending => SortDirection::Descending,
                            SortDirection::Descending => SortDirection::Ascending,
                        };
                    } else {
                        tab.sort_column = sort_column;
                        tab.sort_direction = SortDirection::Ascending;
                    }

                    this.invalidate_track_indices();
                    cx.notify();
                }))
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(6.0))
                    .cursor(CursorStyle::ResizeColumn)
                    .hover(move |this| this.bg(rgb(colors.border_strong)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.begin_column_resize(column, event);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
    }

    pub(super) fn render_track_row(
        &self,
        row_ix: usize,
        track_ix: usize,
        track: &Track,
        lightweight: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = track_ix == self.playing_track;
        let selected = track_ix == self.active_selected_track();
        let colors = *self.colors();
        let bg = if selected {
            colors.selected
        } else if active {
            colors.playing
        } else {
            colors.row
        };
        let title_color = if active {
            colors.accent
        } else {
            colors.text_strong
        };

        div()
            .id(SharedString::from(format!("track-row-{track_ix}")))
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .bg(rgb(bg))
            .when(!lightweight, |this| {
                this.cursor_pointer()
                    .hover(move |this| this.bg(rgb(colors.hover)))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        window.focus(&this.focus_handle);
                        this.set_active_selected_track(track_ix);
                        this.context_menu_track = None;

                        if event.standard_click() && event.modifiers().control {
                            this.queue_track(track_ix);
                            cx.notify();
                            return;
                        }

                        if event.standard_click() && event.click_count() >= 2 {
                            this.play_track(track_ix);
                        }

                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _event: &MouseDownEvent, window, cx| {
                            window.focus(&this.focus_handle);
                            this.set_active_selected_track(track_ix);
                            this.context_menu_track = Some(track_ix);
                            this.context_menu_row = row_ix;
                            cx.notify();
                        }),
                    )
                    .on_drag(
                        TrackDrag::new(track_ix, track),
                        |drag: &TrackDrag, position, _, cx| {
                            let preview = drag.clone().position(position);
                            cx.new(|_| preview)
                        },
                    )
            })
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Index)))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(if active {
                        if self.is_playing { "Ⅱ" } else { "▶" }.into()
                    } else {
                        format!("{:02}", track_ix + 1)
                    }),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Artwork)))
                    .flex()
                    .items_center()
                    .child(if lightweight {
                        self.album_tile_placeholder(track, 22.0)
                    } else {
                        self.album_tile(track, 22.0)
                    }),
            )
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Title)))
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(title_color))
                    .child(track.title.clone()),
            )
            .child(self.cell(track.album.clone(), self.column_width(TableColumn::Album)))
            .child(self.cell(track.codec.clone(), self.column_width(TableColumn::Format)))
            .child(self.cell(track.plays.clone(), self.column_width(TableColumn::Plays)))
            .child(self.cell(
                track.duration.clone(),
                self.column_width(TableColumn::Duration),
            ))
            .child(
                div()
                    .w(px(self.column_width(TableColumn::Loved)))
                    .text_color(rgb(colors.love))
                    .child(if track.loved { "♥" } else { "" }),
            )
    }

    pub(super) fn cell(&self, content: impl Into<SharedString>, width: f32) -> impl IntoElement {
        div()
            .w(px(width))
            .overflow_hidden()
            .text_ellipsis()
            .text_color(rgb(self.colors().text_muted))
            .child(content.into())
    }

    pub(super) fn render_context_menu(
        &self,
        track_ix: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let track = &self.tracks[track_ix];
        let top = 27.0 + ((self.context_menu_row as f32 + 1.0) * TABLE_ROW_H).min(560.0);
        let colors = *self.colors();

        div()
            .absolute()
            .top(px(top))
            .left(px(76.0))
            .w(px(190.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .bg(rgb(colors.elevated))
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(track.title.clone()),
            )
            .child(
                self.context_menu_item("Play from start")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if track_ix < this.tracks.len() {
                            this.play_track(track_ix);
                            cx.notify();
                        }
                    })),
            )
            .child(self.context_menu_item("Add to queue").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.queue_track(track_ix);
                    cx.notify();
                },
            )))
            .child(self.context_menu_item("Queue Album").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.queue_album_from_track(track_ix, false);
                    cx.notify();
                },
            )))
            .child(
                self.context_menu_item("Queue Album Shuffled")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.queue_album_from_track(track_ix, true);
                        cx.notify();
                    })),
            )
            .when(!self.playlists.is_empty(), |this| {
                this.child(
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
                        .child("ADD TO PLAYLIST"),
                )
                .children(
                    self.playlists
                        .iter()
                        .enumerate()
                        .map(|(playlist_ix, playlist)| {
                            self.context_menu_item_dynamic(format!("Add to {}", playlist.name))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.add_track_to_playlist(track_ix, playlist_ix);
                                    cx.notify();
                                }))
                        }),
                )
            })
            .child(self.context_menu_item("Go to album"))
            .child(self.context_menu_item("Show file"))
    }

    pub(super) fn context_menu_item(&self, label: &'static str) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();

        div()
            .id(SharedString::from(format!("context-menu-{label}")))
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
            .child(label)
    }

    pub(super) fn context_menu_item_dynamic(&self, label: String) -> gpui::Stateful<gpui::Div> {
        let id = SharedString::from(format!("context-menu-{label}"));
        let colors = *self.colors();

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
            .child(label)
    }
}
