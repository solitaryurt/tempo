use super::*;

#[derive(Clone, Copy)]
struct BrowseGridMetrics {
    columns: usize,
    card_width: f32,
}

impl TempoApp {
    pub(super) fn render_detail_hero(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.active_tab().source {
            TabSource::Artist(artist_id) => self.render_artist_detail_hero(artist_id, cx),
            TabSource::Album(album_id) => self.render_album_detail_hero(album_id, cx),
            TabSource::Library | TabSource::Playlist(_) => None,
        }
    }

    fn render_artist_detail_hero(
        &self,
        artist_id: i64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let artist = self.artist_by_id(artist_id)?;
        if artist.photo_path.is_none() || artist.bio.is_none() {
            self.queue_artist_metadata_demand(artist.artist_id);
        }
        let colors = *self.colors();
        let albums = self.albums_for_artist(artist.artist_id);

        Some(
            div()
                .id(SharedString::from(format!("artist-hero-{artist_id}")))
                .flex_none()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.elevated))
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .gap_4()
                        .items_center()
                        .child(self.hero_image(
                            SharedString::from(format!("artist-hero-image-{artist_id}")),
                            artist.photo_path.as_ref(),
                            artist.initials.clone(),
                            artist.color,
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(colors.accent))
                                        .child("ARTIST"),
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(colors.text_strong))
                                        .child(artist.name.clone()),
                                )
                                .child(div().text_color(rgb(colors.text_muted)).child(format!(
                                    "{} albums  ·  {} tracks",
                                    artist.album_count, artist.track_count
                                )))
                                .child(div().text_color(rgb(colors.text)).child(
                                    artist.bio.clone().unwrap_or_else(|| {
                                        format!(
                                            "{} is represented by {} local albums in your library.",
                                            artist.name, artist.album_count
                                        )
                                    }),
                                )),
                        ),
                )
                .when(!albums.is_empty(), |this| {
                    this.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_faint))
                                    .child("ALBUMS"),
                            )
                            .child(self.render_artist_album_strip(&albums, cx)),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_album_detail_hero(
        &self,
        album_id: i64,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let album = self.album_by_id(album_id)?;
        if album.artwork_path.is_none() {
            self.queue_album_cover_demand(album.album_id);
        }
        let colors = *self.colors();
        let artist_bio = self
            .artist_by_id(album.artist_id)
            .and_then(|artist| artist.bio.clone());
        let description = artist_bio.unwrap_or_else(|| {
            album
                .year
                .as_ref()
                .map(|year| {
                    format!(
                        "A {year} local album by {}, collected in your library with {} tracks.",
                        album.artist, album.track_count
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "A local album by {}, collected in your library with {} tracks.",
                        album.artist, album.track_count
                    )
                })
        });

        Some(
            div()
                .id(SharedString::from(format!("album-hero-{album_id}")))
                .flex_none()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.elevated))
                .flex()
                .gap_4()
                .items_center()
                .child(self.hero_image(
                    SharedString::from(format!("album-hero-image-{album_id}")),
                    album.artwork_path.as_ref(),
                    album.initials.clone(),
                    album.color,
                ))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.accent))
                                .child("ALBUM"),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(colors.text_strong))
                                .child(album.title.clone()),
                        )
                        .child(div().text_color(rgb(colors.text_muted)).child(format!(
                                "{}  ·  {}  ·  {} tracks",
                                album.artist,
                                album.year.clone().unwrap_or_else(|| "Unknown year".to_string()),
                                album.track_count
                            )))
                        .child(div().text_color(rgb(colors.text)).child(description)),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_artists_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let artist_indices = self.artist_indices_for_search_query(&self.browse_search_query);
        let is_searching = !self.browse_search_query.trim().is_empty();
        let subtitle = if is_searching {
            format!(
                "{} of {} artists  ·  {} local albums",
                artist_indices.len(),
                self.artists.len(),
                self.albums.len()
            )
        } else {
            format!(
                "{} artists  ·  {} local albums",
                self.artists.len(),
                self.albums.len()
            )
        };
        let grid = self.browse_grid_metrics(window);

        div()
            .id("artists-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_header(
                window,
                "Artists",
                subtitle,
                self.artist_view_mode,
                cx,
            ))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .child(match self.artist_view_mode {
                BrowseViewMode::Grid => self.render_artist_grid(grid, artist_indices, cx),
                BrowseViewMode::Table => self.render_artist_table(artist_indices, cx),
            })
    }

    pub(super) fn render_albums_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let album_indices = self.album_indices_for_search_query(&self.browse_search_query);
        let is_searching = !self.browse_search_query.trim().is_empty();
        let subtitle = if is_searching {
            format!(
                "{} of {} albums  ·  {} tracks",
                album_indices.len(),
                self.albums.len(),
                self.tracks.len()
            )
        } else {
            format!(
                "{} albums  ·  {} tracks",
                self.albums.len(),
                self.tracks.len()
            )
        };
        let grid = self.browse_grid_metrics(window);

        div()
            .id("albums-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_browse_header(window, "Albums", subtitle, self.album_view_mode, cx))
            .when(self.tabs.len() > 1, |this| {
                this.child(self.render_tab_bar(cx))
            })
            .child(match self.album_view_mode {
                BrowseViewMode::Grid => self.render_album_grid(grid, album_indices, cx),
                BrowseViewMode::Table => self.render_album_table(album_indices, cx),
            })
    }

    fn render_artist_grid(
        &self,
        grid: BrowseGridMetrics,
        artist_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_grid(
            BrowseScrollbarTarget::ArtistsGrid,
            "artists-grid-scroll",
            "artist-grid-rows",
            if is_searching {
                "No matching artists"
            } else {
                "No artists yet"
            },
            if is_searching {
                "No artists match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by artist."
            },
            artist_indices,
            grid,
            Self::render_artist_grid_row,
            cx,
        )
    }

    fn render_album_grid(
        &self,
        grid: BrowseGridMetrics,
        album_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_grid(
            BrowseScrollbarTarget::AlbumsGrid,
            "albums-grid-scroll",
            "album-grid-rows",
            if is_searching {
                "No matching albums"
            } else {
                "No albums yet"
            },
            if is_searching {
                "No albums match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by album."
            },
            album_indices,
            grid,
            Self::render_album_grid_row,
            cx,
        )
    }

    fn browse_grid_metrics(&self, window: &Window) -> BrowseGridMetrics {
        let sidebar_width = if self.left_sidebar_collapsed {
            0.0
        } else {
            LEFT_SIDEBAR_W
        };
        let width = f32::from(window.viewport_size().width);
        let available = (width - sidebar_width - BROWSE_GRID_PAD_X).max(BROWSE_GRID_CARD_W);
        let columns = ((available + BROWSE_GRID_GAP) / (BROWSE_GRID_CARD_W + BROWSE_GRID_GAP))
            .floor()
            .max(1.0) as usize;
        let total_gap = BROWSE_GRID_GAP * columns.saturating_sub(1) as f32;
        let card_width = ((available - total_gap) / columns as f32).max(BROWSE_GRID_CARD_W);

        BrowseGridMetrics {
            columns,
            card_width,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_browse_grid(
        &self,
        target: BrowseScrollbarTarget,
        id: &'static str,
        list_id: &'static str,
        empty_title: &'static str,
        empty_body: &'static str,
        item_indices: Vec<usize>,
        grid: BrowseGridMetrics,
        render_row: fn(&Self, usize, BrowseGridMetrics, &[usize], &mut Context<Self>) -> AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item_count = item_indices.len();
        if item_count == 0 {
            return div()
                .id(id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .child(self.render_empty_grid_message(empty_title, empty_body))
                .into_any_element();
        }

        let item_indices = Arc::new(item_indices);
        let columns = grid.columns;
        let row_count = item_count.div_ceil(columns);
        let scroll_handle = self.browse_scroll_handle(target);
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .p_4()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, _cx| {
                    window.focus(&this.focus_handle);
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if this.drag_browse_scrollbar(target, event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_browse_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_browse_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                uniform_list(
                    list_id,
                    row_count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        let visible = range.end.saturating_sub(range.start);
                        let _build_span = perf::span(
                            "browse.uniform_list.build",
                            format!(
                                "target={} rows={} range={}..{} columns={}",
                                Self::browse_scrollbar_target_id(target),
                                visible,
                                range.start,
                                range.end,
                                columns
                            ),
                        );
                        let item_indices = item_indices.clone();
                        range
                            .map(|row_ix| render_row(this, row_ix, grid, &item_indices, cx))
                            .collect()
                    }),
                )
                .size_full()
                .track_scroll(scroll_handle),
            )
            .child(self.render_browse_scrollbar(target, row_count, cx))
            .into_any_element()
    }

    fn render_artist_grid_row(
        &self,
        row_ix: usize,
        grid: BrowseGridMetrics,
        artist_indices: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = grid.columns;
        let start = row_ix * columns;
        let end = (start + columns).min(artist_indices.len());

        div()
            .id(SharedString::from(format!("artist-grid-row-{row_ix}")))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                artist_indices[start..end]
                    .iter()
                    .filter_map(|artist_ix| self.artists.get(*artist_ix))
                    .map(|artist| self.render_artist_card(artist, grid.card_width, cx)),
            )
            .into_any_element()
    }

    fn render_album_grid_row(
        &self,
        row_ix: usize,
        grid: BrowseGridMetrics,
        album_indices: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = grid.columns;
        let start = row_ix * columns;
        let end = (start + columns).min(album_indices.len());

        div()
            .id(SharedString::from(format!("album-grid-row-{row_ix}")))
            .flex()
            .gap_4()
            .pb_4()
            .children(
                album_indices[start..end]
                    .iter()
                    .filter_map(|album_ix| self.albums.get(*album_ix))
                    .map(|album| self.render_album_card(album, grid.card_width, cx)),
            )
            .into_any_element()
    }

    fn render_artist_table(
        &self,
        artist_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_table(
            BrowseScrollbarTarget::ArtistsTable,
            "artists-table",
            "artist-table-rows",
            if is_searching {
                "No matching artists"
            } else {
                "No artists yet"
            },
            if is_searching {
                "No artists match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by artist."
            },
            artist_indices,
            &[
                ColumnResizeTarget::Artist(ArtistTableColumn::Artwork),
                ColumnResizeTarget::Artist(ArtistTableColumn::Artist),
                ColumnResizeTarget::Artist(ArtistTableColumn::Albums),
                ColumnResizeTarget::Artist(ArtistTableColumn::Tracks),
            ],
            Self::render_artist_row,
            cx,
        )
    }

    fn render_album_table(&self, album_indices: Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let is_searching = !self.browse_search_query.trim().is_empty();
        self.render_browse_table(
            BrowseScrollbarTarget::AlbumsTable,
            "albums-table",
            "album-table-rows",
            if is_searching {
                "No matching albums"
            } else {
                "No albums yet"
            },
            if is_searching {
                "No albums match the current search."
            } else {
                "Add a music folder and Tempo will group indexed tracks by album."
            },
            album_indices,
            &[
                ColumnResizeTarget::Album(AlbumTableColumn::Artwork),
                ColumnResizeTarget::Album(AlbumTableColumn::Album),
                ColumnResizeTarget::Album(AlbumTableColumn::Artist),
                ColumnResizeTarget::Album(AlbumTableColumn::Year),
                ColumnResizeTarget::Album(AlbumTableColumn::Tracks),
            ],
            Self::render_album_row,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_browse_table(
        &self,
        target: BrowseScrollbarTarget,
        id: &'static str,
        list_id: &'static str,
        empty_title: &'static str,
        empty_body: &'static str,
        row_indices: Vec<usize>,
        columns: &'static [ColumnResizeTarget],
        render_row: fn(&Self, usize, usize, &mut Context<Self>) -> Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let row_count = row_indices.len();
        let row_indices = Arc::new(row_indices);
        let scroll_handle = self.browse_scroll_handle(target);

        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(colors.border))
            .child(self.render_resizable_table_header(34.0, columns, cx))
            .when(row_count == 0, |this| {
                this.child(
                    div()
                        .p_4()
                        .child(self.render_empty_grid_message(empty_title, empty_body)),
                )
            })
            .when(row_count > 0, |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .overflow_hidden()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event: &MouseDownEvent, window, _cx| {
                                window.focus(&this.focus_handle);
                            }),
                        )
                        .on_mouse_move(cx.listener(
                            move |this, event: &MouseMoveEvent, _window, cx| {
                                if this.drag_browse_scrollbar(target, event) {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            },
                        ))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                if this.finish_browse_scrollbar_drag() {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                                if this.finish_browse_scrollbar_drag() {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }),
                        )
                        .child(
                            uniform_list(
                                list_id,
                                row_count,
                                cx.processor(move |this, range: Range<usize>, _window, cx| {
                                    let visible = range.end.saturating_sub(range.start);
                                    let _build_span = perf::span(
                                        "browse.uniform_list.build",
                                        format!(
                                            "target={} rows={} range={}..{} mode=table",
                                            Self::browse_scrollbar_target_id(target),
                                            visible,
                                            range.start,
                                            range.end
                                        ),
                                    );
                                    let row_indices = row_indices.clone();
                                    range
                                        .filter_map(|row_ix| {
                                            let item_ix = *row_indices.get(row_ix)?;
                                            render_row(this, row_ix, item_ix, cx)
                                        })
                                        .collect()
                                }),
                            )
                            .size_full()
                            .track_scroll(scroll_handle),
                        )
                        .child(self.render_browse_scrollbar(target, row_count, cx)),
                )
            })
            .into_any_element()
    }

    pub(super) fn browse_scroll_handle(
        &self,
        target: BrowseScrollbarTarget,
    ) -> UniformListScrollHandle {
        match target {
            BrowseScrollbarTarget::ArtistsGrid => self.artist_grid_scroll_handle.clone(),
            BrowseScrollbarTarget::ArtistsTable => self.artist_table_scroll_handle.clone(),
            BrowseScrollbarTarget::AlbumsGrid => self.album_grid_scroll_handle.clone(),
            BrowseScrollbarTarget::AlbumsTable => self.album_table_scroll_handle.clone(),
            BrowseScrollbarTarget::PlaybackHistory => self.playback_history_scroll_handle.clone(),
        }
    }

    fn browse_scrollbar_base_handle(&self, target: BrowseScrollbarTarget) -> gpui::ScrollHandle {
        self.browse_scroll_handle(target)
            .0
            .borrow()
            .base_handle
            .clone()
    }

    fn browse_scrollbar_metrics(
        &self,
        target: BrowseScrollbarTarget,
    ) -> Option<TableScrollbarMetrics> {
        let handle = self.browse_scroll_handle(target);
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

    fn begin_browse_scrollbar_drag(
        &mut self,
        target: BrowseScrollbarTarget,
        event: &MouseDownEvent,
    ) -> bool {
        let Some(metrics) = self.browse_scrollbar_metrics(target) else {
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
        let start_offset = self.browse_scrollbar_base_handle(target).offset();

        self.browse_scrollbar_drag = Some(BrowseScrollbarDrag {
            target,
            thumb_offset,
            start_offset,
        });
        self.scroll_browse_to_scrollbar_y(target, event.position.y, thumb_offset);
        true
    }

    fn drag_browse_scrollbar(
        &mut self,
        target: BrowseScrollbarTarget,
        event: &MouseMoveEvent,
    ) -> bool {
        let Some(drag) = self.browse_scrollbar_drag else {
            return false;
        };

        if drag.target != target {
            return false;
        }

        if !event.dragging() {
            self.browse_scrollbar_drag = None;
            return false;
        }

        self.scroll_browse_to_scrollbar_y(target, event.position.y, drag.thumb_offset)
    }

    pub(super) fn finish_browse_scrollbar_drag(&mut self) -> bool {
        self.browse_scrollbar_drag.take().is_some()
    }

    pub(super) fn cancel_browse_scrollbar_drag(&mut self) -> bool {
        let Some(drag) = self.browse_scrollbar_drag.take() else {
            return false;
        };

        self.browse_scrollbar_base_handle(drag.target)
            .set_offset(drag.start_offset);
        true
    }

    fn scroll_browse_to_scrollbar_y(
        &mut self,
        target: BrowseScrollbarTarget,
        mouse_y: Pixels,
        thumb_offset: f32,
    ) -> bool {
        let Some(metrics) = self.browse_scrollbar_metrics(target) else {
            return false;
        };

        if metrics.max_scroll <= 0.0 {
            return false;
        }

        let thumb_travel = (metrics.track_height - metrics.thumb_height).max(1.0);
        let thumb_top =
            (f32::from(mouse_y) - metrics.track_top - thumb_offset).clamp(0.0, thumb_travel);
        let ratio = thumb_top / thumb_travel;
        self.scroll_browse_to_ratio(target, ratio)
    }

    fn scroll_browse_to_ratio(&mut self, target: BrowseScrollbarTarget, ratio: f32) -> bool {
        let base_handle = self.browse_scrollbar_base_handle(target);
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

    pub(super) fn render_browse_scrollbar(
        &self,
        target: BrowseScrollbarTarget,
        item_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if item_count == 0 {
            return div().into_any_element();
        }

        let metrics = self.browse_scrollbar_metrics(target);
        let markers = self.browse_scrollbar_markers(target);
        let current_label = metrics
            .and_then(|metrics| self.current_browse_scrollbar_label(target, metrics, &markers));
        let is_dragging = self
            .browse_scrollbar_drag
            .is_some_and(|drag| drag.target == target);

        div()
            .id(SharedString::from(format!(
                "browse-scrollbar-{}",
                Self::browse_scrollbar_target_id(target)
            )))
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(TABLE_SCROLLBAR_W))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle);
                    if this.begin_browse_scrollbar_drag(target, event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if this.drag_browse_scrollbar(target, event) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_browse_scrollbar_drag() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(self.render_marker_scrollbar_inner(
                metrics,
                &markers,
                current_label,
                is_dragging,
            ))
            .into_any_element()
    }

    pub(super) fn browse_scrollbar_target_id(target: BrowseScrollbarTarget) -> &'static str {
        match target {
            BrowseScrollbarTarget::ArtistsGrid => "artists-grid",
            BrowseScrollbarTarget::ArtistsTable => "artists-table",
            BrowseScrollbarTarget::AlbumsGrid => "albums-grid",
            BrowseScrollbarTarget::AlbumsTable => "albums-table",
            BrowseScrollbarTarget::PlaybackHistory => "playback-history",
        }
    }

    fn render_browse_header(
        &self,
        window: &Window,
        title: &'static str,
        subtitle: String,
        mode: BrowseViewMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
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
                            .child(title),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(subtitle),
            )
            .when_some(self.render_metadata_status(cx), |this, status| {
                this.child(status)
            })
            .child(div().flex_1())
            .child(self.render_search_box(window, "Search", cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        self.render_view_mode_button("Grid", title, mode == BrowseViewMode::Grid)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_browse_view_mode(title, BrowseViewMode::Grid);
                                cx.notify();
                            })),
                    )
                    .child(
                        self.render_view_mode_button("Table", title, mode == BrowseViewMode::Table)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_browse_view_mode(title, BrowseViewMode::Table);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                self.with_tooltip(
                    self.sidebar_button("⚙", "open-settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_page(Page::Settings);
                            cx.notify();
                        })),
                    "browse-open-settings-tooltip",
                    "Settings",
                    cx,
                ),
            )
    }

    fn render_view_mode_button(
        &self,
        label: &'static str,
        page: &'static str,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();
        let bg = if active {
            colors.button_hover
        } else {
            colors.button
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let border = if active {
            colors.border_strong
        } else {
            colors.waveform_border
        };

        div()
            .id(SharedString::from(format!(
                "{}-{}-view",
                page.to_ascii_lowercase(),
                label.to_ascii_lowercase()
            )))
            .h(px(24.0))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(rgb(fg))
            .hover(move |this| {
                this.bg(rgb(colors.button_hover))
                    .text_color(rgb(colors.text_strong))
            })
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    fn set_browse_view_mode(&mut self, page: &'static str, mode: BrowseViewMode) {
        match page {
            "Artists" => self.artist_view_mode = mode,
            "Albums" => self.album_view_mode = mode,
            _ => {}
        }
    }

    fn render_artist_row(
        &self,
        row_ix: usize,
        artist_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let artist = self.artists.get(artist_ix)?;
        let colors = *self.colors();
        let bg = if row_ix.is_multiple_of(2) {
            colors.surface
        } else {
            colors.panel_alt
        };
        let artist_id = artist.artist_id;

        div()
            .id(SharedString::from(format!(
                "artist-row-{}",
                artist.artist_id
            )))
            .h(px(50.0))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .child(
                div()
                    .w(px(
                        self.artist_table_column_width(ArtistTableColumn::Artwork)
                    ))
                    .child(self.row_image(
                        SharedString::from(format!("artist-row-image-{}", artist.artist_id)),
                        artist.photo_path.as_ref(),
                        artist.initials.clone(),
                        artist.color,
                    )),
            )
            .child(
                div()
                    .w(px(self.artist_table_column_width(ArtistTableColumn::Artist)))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_color(rgb(colors.text_strong))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(artist.name.clone()),
                    )
                    .when_some(artist.bio.as_ref(), |this, bio| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.text_faint))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(bio.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(self.artist_table_column_width(ArtistTableColumn::Albums)))
                    .text_color(rgb(colors.text_muted))
                    .child(artist.album_count.to_string()),
            )
            .child(
                div()
                    .w(px(self.artist_table_column_width(ArtistTableColumn::Tracks)))
                    .text_color(rgb(colors.text_muted))
                    .child(artist.track_count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_artist_tab(artist_id);
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn render_album_row(
        &self,
        row_ix: usize,
        album_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let album = self.albums.get(album_ix)?;
        let colors = *self.colors();
        let bg = if row_ix.is_multiple_of(2) {
            colors.surface
        } else {
            colors.panel_alt
        };
        let album_id = album.album_id;

        div()
            .id(SharedString::from(format!(
                "album-row-{}-{}",
                album.artist_id, album.album_id
            )))
            .h(px(50.0))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.hover)))
            .child(
                div()
                    .w(px(self.album_table_column_width(AlbumTableColumn::Artwork)))
                    .child(self.row_image(
                        SharedString::from(format!(
                            "album-row-image-{}-{}",
                            album.artist_id, album.album_id
                        )),
                        album.artwork_path.as_ref(),
                        album.initials.clone(),
                        album.color,
                    )),
            )
            .child(
                div()
                    .w(px(self.album_table_column_width(AlbumTableColumn::Album)))
                    .min_w_0()
                    .text_color(rgb(colors.text_strong))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(album.title.clone()),
            )
            .child(
                div()
                    .w(px(self.album_table_column_width(AlbumTableColumn::Artist)))
                    .min_w_0()
                    .text_color(rgb(colors.text_muted))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(album.artist.clone()),
            )
            .child(
                div()
                    .w(px(self.album_table_column_width(AlbumTableColumn::Year)))
                    .text_color(rgb(colors.text_muted))
                    .child(album.year.clone().unwrap_or_else(|| "Unknown".to_string())),
            )
            .child(
                div()
                    .w(px(self.album_table_column_width(AlbumTableColumn::Tracks)))
                    .text_color(rgb(colors.text_muted))
                    .child(album.track_count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_album_tab(album_id);
                cx.notify();
            }))
            .into_any_element()
            .into()
    }

    fn render_artist_card(
        &self,
        artist: &Artist,
        card_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let artist_id = artist.artist_id;
        if artist.photo_path.is_none() {
            self.queue_artist_metadata_demand(artist_id);
        }

        div()
            .id(SharedString::from(format!(
                "artist-card-{}",
                artist.artist_id
            )))
            .w(px(card_width))
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel_alt))
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |this| {
                this.bg(rgb(colors.hover))
                    .border_color(rgb(colors.border_strong))
            })
            .active(|this| this.opacity(0.88))
            .child(self.square_grid_image(
                SharedString::from(format!("artist-card-image-{}", artist.artist_id)),
                artist.photo_path.as_ref(),
                artist.initials.clone(),
                artist.color,
                card_width,
            ))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(artist.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .child(format!(
                                "{} albums  ·  {} tracks",
                                artist.album_count, artist.track_count
                            )),
                    )
                    .when_some(artist.bio.as_ref(), |this, bio| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(colors.text_faint))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(bio.clone()),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_artist_tab(artist_id);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_album_card(
        &self,
        album: &Album,
        card_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let album_id = album.album_id;
        if album.artwork_path.is_none() {
            self.queue_album_cover_demand(album_id);
        }

        div()
            .id(SharedString::from(format!(
                "album-card-{}-{}",
                album.artist_id, album.album_id
            )))
            .w(px(card_width))
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel_alt))
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |this| {
                this.bg(rgb(colors.hover))
                    .border_color(rgb(colors.border_strong))
            })
            .active(|this| this.opacity(0.88))
            .child(self.square_grid_image(
                SharedString::from(format!(
                    "album-card-image-{}-{}",
                    album.artist_id, album.album_id
                )),
                album.artwork_path.as_ref(),
                album.initials.clone(),
                album.color,
                card_width,
            ))
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(album.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(album.artist.clone()),
                    )
                    .child(
                        div().text_xs().text_color(rgb(colors.text_faint)).child(
                            album
                                .year
                                .as_ref()
                                .map(|year| format!("{}  ·  {} tracks", year, album.track_count))
                                .unwrap_or_else(|| format!("{} tracks", album.track_count)),
                        ),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_album_tab(album_id);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_artist_album_strip(&self, albums: &[&Album], cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("artist-album-hero-strip")
            .w_full()
            .overflow_x_scroll()
            .child(
                div().flex().gap_3().pb_2().children(
                    albums
                        .iter()
                        .map(|album| self.render_album_card(album, BROWSE_GRID_CARD_W, cx)),
                ),
            )
            .into_any_element()
    }

    fn hero_image(
        &self,
        id: SharedString,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .id(id)
            .w(px(132.0))
            .h(px(132.0))
            .flex_none()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border_strong))
            .overflow_hidden()
            .shadow_lg()
            .child(match path {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Self::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => Self::album_tile_fallback(initials, color, colors),
            })
            .into_any_element()
    }

    fn square_grid_image(
        &self,
        id: SharedString,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
        size: f32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .id(id)
            .w(px(size))
            .h(px(size))
            .border_b_1()
            .border_color(rgb(colors.border))
            .overflow_hidden()
            .child(match path {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Self::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => Self::album_tile_fallback(initials, color, colors),
            })
            .into_any_element()
    }

    fn row_image(
        &self,
        id: SharedString,
        path: Option<&PathBuf>,
        initials: String,
        color: u32,
    ) -> AnyElement {
        let colors = *self.colors();
        let fallback_initials = initials.clone();

        div()
            .id(id)
            .w(px(38.0))
            .h(px(38.0))
            .flex_none()
            .rounded_sm()
            .border_1()
            .border_color(rgb(colors.border))
            .overflow_hidden()
            .child(match path {
                Some(path) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        Self::album_tile_fallback(fallback_initials.clone(), color, colors)
                    })
                    .into_any_element(),
                None => Self::album_tile_fallback(initials, color, colors),
            })
            .into_any_element()
    }

    fn render_empty_grid_message(&self, title: &'static str, body: &'static str) -> AnyElement {
        let colors = *self.colors();

        div()
            .w_full()
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
                    .child(title),
            )
            .child(div().text_color(rgb(colors.text_muted)).child(body))
            .into_any_element()
    }
}
