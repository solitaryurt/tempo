use super::*;

impl TempoApp {
    /// Render the standalone "Liked" page reachable from the sidebar.
    /// The liked page is a filtered view of the in-memory track list
    /// (`Track::liked == true`) rendered with the same row + cell code
    /// as the main library table so the user sees identical artwork,
    /// title styling, hover states, and column visibility.
    ///
    /// Liked indices are recomputed every render rather than cached on
    /// `TempoApp` because the count changes only on toggle, the user
    /// is unlikely to be on this page at the moment of the toggle, and
    /// the filter is a single linear pass over `tracks`.
    pub(super) fn render_liked_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let liked_indices = self.liked_track_indices();
        let item_count = liked_indices.len();
        let subtitle = if item_count == 1 {
            "1 liked track".to_string()
        } else {
            format!("{} liked tracks", Self::format_count_short(item_count))
        };

        div()
            .id("liked-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_simple_page_header("Liked", subtitle))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .child(
                div()
                    .id("liked-scroll")
                    .flex_1()
                    .min_h_0()
                    .child(self.render_liked_table(liked_indices, cx)),
            )
    }

    fn render_liked_table(
        &self,
        liked_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let scroll_handle = self.liked_scroll_handle.clone();
        let item_count = liked_indices.len();

        div()
            .flex()
            .flex_col()
            .size_full()
            // Right-click anywhere in the body opens the column menu,
            // matching the main table's behaviour so column-toggle UX
            // is consistent across pages.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.show_column_menu(event);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(self.render_liked_header(cx))
            .when(item_count == 0, |this| {
                this.child(div().p_5().text_color(rgb(colors.text_muted)).child(
                    "No liked tracks yet. Click a heart in the Liked column to add tracks here.",
                ))
            })
            .when(item_count > 0, |this| {
                let scrollbar =
                    self.render_browse_scrollbar(BrowseScrollbarTarget::Liked, item_count, cx);
                this.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(
                            uniform_list(
                                "liked-rows",
                                item_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    let visible = range.end.saturating_sub(range.start);
                                    let _build_span = perf::span(
                                        "liked.uniform_list.build",
                                        format!(
                                            "rows={} range={}..{}",
                                            visible, range.start, range.end
                                        ),
                                    );
                                    let is_playing = this.player.read(cx).is_playing();
                                    range
                                        .filter_map(|row_ix| {
                                            let track_ix = liked_indices.get(row_ix).copied()?;
                                            let track = this.tracks.get(track_ix)?;
                                            Some(
                                                this.render_track_row(
                                                    row_ix, track_ix, track, is_playing, false, cx,
                                                )
                                                .into_any_element(),
                                            )
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .track_scroll(scroll_handle),
                        )
                        .child(scrollbar),
                )
            })
    }

    /// Header row for the Liked page. Reuses the standard column
    /// header rendering so the resize handles, drag-to-reorder, and
    /// sort indicators all behave identically to the main library
    /// table.
    fn render_liked_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        div()
            .id("liked-header")
            .h(px(27.0))
            .flex_none()
            .px_4()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.app))
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(colors.text_muted))
            .flex()
            .items_center()
            .overflow_hidden()
            .children(
                self.visible_columns
                    .iter()
                    .copied()
                    .map(|column| self.liked_column_header(column, cx)),
            )
    }

    fn liked_column_header(&self, column: TableColumn, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let label = Self::column_label(column);
        let width = self.column_width(column);

        div()
            .id(SharedString::from(format!(
                "liked-column-{}",
                Self::column_key(column)
            )))
            .relative()
            .h_full()
            .w(px(width))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .text_color(rgb(colors.text_faint))
            .hover(move |this| this.text_color(rgb(colors.text)))
            .child(label)
            .on_drag(
                ColumnDrag::new(column, label),
                |drag: &ColumnDrag, position, _, cx| {
                    let preview = drag.clone().position(position);
                    cx.new(|_| preview)
                },
            )
            .on_drop(cx.listener(move |this, drag: &ColumnDrag, _window, cx| {
                this.move_table_column_before(drag.column, column);
                cx.notify();
            }))
            .child(self.liked_column_resizer(ColumnResizeTarget::Track(column), cx))
            .into_any_element()
    }

    fn liked_column_resizer(
        &self,
        target: ColumnResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        div()
            .id(SharedString::from(format!(
                "liked-column-resizer-{}",
                Self::resize_target_key(target)
            )))
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
                    this.begin_resize_target(target, event);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                if event.standard_click() && event.click_count() >= 2 {
                    this.autosize_resize_target(target);
                    cx.notify();
                }
                cx.stop_propagation();
            }))
    }

    /// Indices into `self.tracks` for tracks that are currently liked,
    /// preserving the natural track order so the Liked page reads as
    /// a stable filtered slice of the library.
    pub(super) fn liked_track_indices(&self) -> Vec<usize> {
        self.tracks
            .iter()
            .enumerate()
            .filter_map(|(ix, track)| if track.liked { Some(ix) } else { None })
            .collect()
    }
}
