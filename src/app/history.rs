use super::*;
use chrono::{DateTime, Datelike as _, Local};

impl TempoApp {
    pub(super) fn record_playback_history(&mut self, track_ix: usize) {
        let Some(track) = self.tracks.get(track_ix) else {
            return;
        };
        let played_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();

        self.playback_history.push(PlaybackHistoryEntry {
            played_at_unix_secs,
            track_path: track.path.clone(),
            // History entries are serialized to state.json, so we keep
            // them as `String`. The `Track` fields became `SharedString`
            // for the render hot path; convert once here.
            title: track.title.to_string(),
            artist: track.artist.to_string(),
            album: track.album.to_string(),
            duration: track.duration.to_string(),
        });
        self.save_app_state();
    }

    pub(super) fn render_playback_history_page(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let ordered_indices = self.sorted_playback_history_indices();
        let item_count = ordered_indices.len();
        let subtitle = if item_count == 1 {
            "1 play recorded".to_string()
        } else {
            format!("{item_count} plays recorded")
        };

        div()
            .id("playback-history-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_simple_page_header("Playback History", subtitle))
            .child(self.render_tab_bar(cx))
            .child(
                div()
                    .id("playback-history-scroll")
                    .flex_1()
                    .min_h_0()
                    .child(self.render_playback_history_table(ordered_indices, cx)),
            )
    }

    fn render_playback_history_table(
        &self,
        ordered_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let scroll_handle = self.playback_history_scroll_handle.clone();
        let item_count = ordered_indices.len();

        div()
            .flex()
            .flex_col()
            .size_full()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.show_column_menu(event);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(self.render_playback_history_header(cx))
            .when(self.playback_history.is_empty(), |this| {
                this.child(
                    div().p_5().text_color(rgb(colors.text_muted)).child(
                        "No playback history yet. Tracks will appear here as you play them.",
                    ),
                )
            })
            .when(!self.playback_history.is_empty(), |this| {
                let scrollbar = self.render_browse_scrollbar(
                    BrowseScrollbarTarget::PlaybackHistory,
                    item_count,
                    cx,
                );
                this.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(
                            uniform_list(
                                "playback-history-rows",
                                item_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    let visible = range.end.saturating_sub(range.start);
                                    let _build_span = perf::span(
                                        "history.uniform_list.build",
                                        format!(
                                            "rows={} range={}..{}",
                                            visible, range.start, range.end
                                        ),
                                    );
                                    range
                                        .filter_map(|row_ix| {
                                            let history_ix =
                                                ordered_indices.get(row_ix).copied()?;
                                            let entry = this.playback_history.get(history_ix)?;
                                            Some(
                                                this.render_playback_history_row(
                                                    row_ix, history_ix, entry, cx,
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

    fn render_playback_history_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .id("playback-history-header")
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
            .child(self.history_played_at_header(cx))
            .children(
                self.visible_columns
                    .iter()
                    .copied()
                    .map(|column| self.history_column_header(column, cx)),
            )
    }

    fn history_played_at_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let target = ColumnResizeTarget::PlaybackHistoryPlayedAt;

        div()
            .id("history-column-played-at")
            .relative()
            .h_full()
            .w(px(self.resize_target_width(target)))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .text_color(rgb(colors.text))
            .child("PLAYED AT")
            .child("▼")
            .child(self.history_column_resizer(target, cx))
            .into_any_element()
    }

    fn history_column_header(&self, column: TableColumn, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let label = Self::column_label(column);
        let width = self.column_width(column);

        div()
            .id(SharedString::from(format!(
                "history-column-{}",
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
            .child(self.history_column_resizer(ColumnResizeTarget::Track(column), cx))
            .into_any_element()
    }

    fn history_column_resizer(
        &self,
        target: ColumnResizeTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .id(SharedString::from(format!(
                "history-column-resizer-{}",
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

    fn render_playback_history_row(
        &self,
        ix: usize,
        history_ix: usize,
        entry: &PlaybackHistoryEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let track_ix = self
            .tracks
            .iter()
            .position(|track| track.path == entry.track_path);
        let active = track_ix.is_some_and(|track_ix| track_ix == self.playing_track);
        let is_playing = self.player.read(cx).is_playing();
        let bg = if active {
            colors.playing
        } else if ix.is_multiple_of(2) {
            colors.row
        } else {
            colors.surface
        };

        div()
            .id(gpui::ElementId::NamedInteger(
                "playback-history-row".into(),
                ix as u64,
            ))
            .h(px(TABLE_ROW_H))
            .px_4()
            .flex()
            .items_center()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(colors.row_border))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .when_some(track_ix, |this, track_ix| {
                this.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    this.set_active_selected_track(track_ix);
                    this.context_menu_track = None;
                    this.column_menu_open = false;

                    if event.standard_click() && event.click_count() >= 2 {
                        this.play_track_from_picker(track_ix, cx);
                    }

                    cx.notify();
                }))
            })
            .child(
                self.history_played_at_cell(history_ix, entry.played_at_unix_secs, cx)
                    .into_any_element(),
            )
            .children(self.visible_columns.iter().copied().map(|column| {
                self.history_column_cell(column, track_ix, entry, active, is_playing, cx)
            }))
    }

    fn history_played_at_cell(
        &self,
        history_ix: usize,
        unix_secs: u64,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();
        let cell = div()
            .id(SharedString::from(format!(
                "history-played-at-{history_ix}"
            )))
            .w(px(self.playback_history_played_at_width))
            .flex_none()
            .overflow_hidden()
            .text_ellipsis()
            .text_color(rgb(colors.text_muted))
            .child(Self::history_played_at_label(unix_secs));

        self.with_tooltip(
            cell,
            SharedString::from(format!("history-played-at-tooltip-{history_ix}")),
            Self::history_played_at_absolute_label(unix_secs),
            cx,
        )
    }

    fn history_column_cell(
        &self,
        column: TableColumn,
        track_ix: Option<usize>,
        entry: &PlaybackHistoryEntry,
        active: bool,
        is_playing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some((track_ix, track)) =
            track_ix.and_then(|track_ix| self.tracks.get(track_ix).map(|track| (track_ix, track)))
        {
            return self.track_cell(column, track_ix, track, active, is_playing, false, cx);
        }

        self.history_snapshot_cell(column, entry, active)
    }

    fn history_snapshot_cell(
        &self,
        column: TableColumn,
        entry: &PlaybackHistoryEntry,
        active: bool,
    ) -> AnyElement {
        let colors = *self.colors();
        let width = self.column_width(column);

        match column {
            TableColumn::Index => self.cell("", width).into_any_element(),
            TableColumn::Artwork => div()
                .w(px(width))
                .flex()
                .items_center()
                .child(
                    div()
                        .w(px(22.0))
                        .h(px(22.0))
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(colors.border_strong))
                        .bg(rgb(colors.playing))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(rgb(colors.text_faint))
                        .child("♪"),
                )
                .into_any_element(),
            TableColumn::Title => div()
                .w(px(width))
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(if active {
                    colors.accent
                } else {
                    colors.text_strong
                }))
                .child(entry.title.clone())
                .into_any_element(),
            TableColumn::Artist => self.cell(entry.artist.clone(), width).into_any_element(),
            TableColumn::Album => self.cell(entry.album.clone(), width).into_any_element(),
            TableColumn::Duration => self.cell(entry.duration.clone(), width).into_any_element(),
            TableColumn::Genre
            | TableColumn::TrackNumber
            | TableColumn::Format
            | TableColumn::Bitrate
            | TableColumn::FileSize
            | TableColumn::Year
            | TableColumn::DateAdded
            | TableColumn::Plays
            | TableColumn::Liked => self.cell("", width).into_any_element(),
        }
    }

    pub(super) fn sorted_playback_history_indices(&self) -> Vec<usize> {
        let mut indices = (0..self.playback_history.len()).collect::<Vec<_>>();
        indices.sort_by(|left, right| {
            self.playback_history[*right]
                .played_at_unix_secs
                .cmp(&self.playback_history[*left].played_at_unix_secs)
                .then_with(|| right.cmp(left))
        });
        indices
    }

    pub(super) fn history_played_at_label(unix_secs: u64) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(unix_secs);
        if unix_secs <= now {
            let age = now - unix_secs;
            if age < 60 {
                return "just now".to_string();
            }
            if age < 3_600 {
                let minutes = age / 60;
                return format!("{minutes} min ago");
            }
            if age < 86_400 {
                let hours = age / 3_600;
                return if hours == 1 {
                    "1 hour ago".to_string()
                } else {
                    format!("{hours} hours ago")
                };
            }
        }

        let Some(local) = Self::history_local_datetime(unix_secs) else {
            return Self::date_time_label_from_unix(unix_secs);
        };
        let now_local = Local::now();
        if local.year() == now_local.year() {
            local.format("%b %-d, %-I:%M %p").to_string()
        } else {
            local.format("%Y-%m-%d").to_string()
        }
    }

    fn history_played_at_absolute_label(unix_secs: u64) -> String {
        Self::history_local_datetime(unix_secs)
            .map(|time| time.format("%Y-%m-%d %H:%M:%S %Z").to_string())
            .unwrap_or_else(|| Self::date_time_label_from_unix(unix_secs))
    }

    fn history_local_datetime(unix_secs: u64) -> Option<DateTime<Local>> {
        DateTime::from_timestamp(unix_secs as i64, 0).map(|utc| utc.with_timezone(&Local))
    }
}
