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
            Page::Genres => self.render_genres_page(window, cx).into_any_element(),
            Page::Liked => self.render_liked_page(cx).into_any_element(),
            Page::PlaybackHistory => self.render_playback_history_page(cx).into_any_element(),
            Page::Errors => self.render_errors_page(cx).into_any_element(),
            Page::Analytics => div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(self.render_analytics_page(cx))
                .child(self.render_analytics_sidebar(cx))
                .into_any_element(),
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
            .child(self.render_tab_bar(cx))
            .when_some(self.render_detail_hero(cx), |this, hero| this.child(hero))
            .child(self.render_table(cx))
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        self.render_tab_bar_with_controls(None, cx)
    }

    pub(super) fn render_tab_bar_with_controls(
        &self,
        view_controls: Option<(&'static str, BrowseViewMode)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let scroll_handle = self.tab_bar_scroll_handle.clone();
        // Read the current scroll state out of the handle so the
        // arrow buttons can show the right enabled/disabled visual.
        // First-frame values are zero (the handle isn't painted yet);
        // that just means the buttons render disabled until layout
        // produces real bounds, which is fine.
        let max_scroll_x = f32::from(scroll_handle.max_offset().width).max(0.0);
        let scroll_x = -f32::from(scroll_handle.offset().x);
        let can_scroll = max_scroll_x > 0.5;
        let at_left = scroll_x <= 0.5;
        let at_right = scroll_x >= max_scroll_x - 0.5;

        div()
            .h(px(30.0))
            .flex_none()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.app))
            .flex()
            .items_center()
            .child(
                // Scrollable strip containing only the tab list. The
                // new-tab `+` button is intentionally a sibling (not
                // a child) of this wrapper so it stays visible even
                // when the tab list overflows. `flex_initial()`
                // (grow=0, shrink=1, basis=auto) sizes the wrapper to
                // its natural content when there's room to spare, but
                // lets the parent flex squeeze it down when the tab
                // list is wider than the available space -- at which
                // point `overflow_x_scroll` clips the contents and
                // exposes the scroll handle's max_offset for the
                // arrow buttons. `min_w_0` is required for the shrink
                // path because flex children otherwise refuse to
                // shrink below their content's intrinsic min-content
                // width. `overflow_x_scroll` also gives us the
                // built-in wheel handler: when only the X axis is
                // scrollable, vertical wheel input is mapped to
                // horizontal scrolling, which matches the user's
                // expected behavior.
                div()
                    .id("tab-bar-scroll")
                    .flex_initial()
                    .min_w_0()
                    .h_full()
                    .overflow_x_scroll()
                    .track_scroll(&scroll_handle)
                    .flex()
                    .items_center()
                    .children(
                        self.tabs
                            .iter()
                            .enumerate()
                            .map(|(ix, tab)| self.render_tab(ix, tab, cx)),
                    ),
            )
            .child(
                div()
                    .id("new-tab-button")
                    .w(px(24.0))
                    .h_full()
                    .flex_none()
                    .border_r_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.button))
                    .text_color(rgb(colors.text_muted))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |this| {
                        this.bg(rgb(colors.button_hover))
                            .text_color(rgb(colors.text_strong))
                    })
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_library_tab();
                        window.focus(&this.search_focus_handle);
                        cx.notify();
                    })),
            )
            .when(can_scroll, |this| {
                this.child(self.render_tab_bar_arrow("tab-bar-scroll-left", "‹", at_left, -1, cx))
                    .child(self.render_tab_bar_arrow("tab-bar-scroll-right", "›", at_right, 1, cx))
            })
            .child(div().flex_1())
            .when_some(view_controls, |this, (page, mode)| {
                this.child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .border_l_1()
                        .border_color(rgb(colors.border))
                        .child(
                            self.with_tooltip(
                                self.render_view_mode_button(
                                    "Grid",
                                    page,
                                    mode == BrowseViewMode::Grid,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_browse_view_mode(page, BrowseViewMode::Grid);
                                        cx.notify();
                                    },
                                )),
                                SharedString::from(format!("{page}-grid-view-tooltip")),
                                "Grid view",
                                cx,
                            ),
                        )
                        .child(
                            self.with_tooltip(
                                self.render_view_mode_button(
                                    "Table",
                                    page,
                                    mode == BrowseViewMode::Table,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.set_browse_view_mode(page, BrowseViewMode::Table);
                                        cx.notify();
                                    },
                                )),
                                SharedString::from(format!("{page}-table-view-tooltip")),
                                "Table view",
                                cx,
                            ),
                        ),
                )
            })
    }

    /// Per-arrow renderer for the tab-bar scroll arrows. `direction`
    /// is `-1` for "scroll left" and `1` for "scroll right". When the
    /// strip is at the corresponding edge the button still renders so
    /// the layout doesn't jump, but is dimmed and short-circuits the
    /// click.
    fn render_tab_bar_arrow(
        &self,
        id: &'static str,
        glyph: &'static str,
        disabled: bool,
        direction: i32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .id(SharedString::from(id))
            .w(px(20.0))
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.button))
            .text_color(rgb(if disabled {
                colors.text_faint
            } else {
                colors.text_muted
            }))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .opacity(if disabled { 0.4 } else { 1.0 })
            .hover(move |this| {
                if disabled {
                    this
                } else {
                    this.bg(rgb(colors.button_hover))
                        .text_color(rgb(colors.text_strong))
                }
            })
            .child(glyph)
            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                if this.scroll_tab_bar_by(direction as f32 * TAB_BAR_ARROW_STEP) {
                    cx.notify();
                }
            }))
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
            .max_w(px(176.0))
            .flex_none()
            .h_full()
            .px_2()
            .border_r_1()
            .border_color(rgb(border))
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap_2()
            .when(!active, |this| {
                this.hover(move |this| {
                    this.bg(rgb(colors.button_hover))
                        .text_color(rgb(colors.text_strong))
                })
            })
            .active(|this| this.opacity(0.82))
            .child(self.tab_source_icon(&tab.source, active))
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
                match &tab.source {
                    TabSource::Playlist(playlist_ix) => Some(*playlist_ix),
                    TabSource::Library
                    | TabSource::Artist(_)
                    | TabSource::Album(_)
                    | TabSource::Genre(_) => None,
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

    fn tab_source_icon(&self, source: &TabSource, active: bool) -> AnyElement {
        let colors = *self.colors();
        match source {
            TabSource::Library => Self::sidebar_nav_icon(Page::Library, active, colors),
            TabSource::Artist(_) => Self::sidebar_nav_icon(Page::Artists, active, colors),
            TabSource::Album(_) => Self::sidebar_nav_icon(Page::Albums, active, colors),
            TabSource::Genre(_) => Self::sidebar_nav_icon(Page::Genres, active, colors),
            TabSource::Playlist(_) => Self::playlist_tab_icon(active, colors),
        }
    }

    fn playlist_tab_icon(active: bool, colors: ThemeColors) -> AnyElement {
        let color = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let accent = if active { colors.accent } else { color };
        let color = format!("#{:06x}", color);
        let accent = format!("#{:06x}", accent);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">
<path d="M6.2 5.5H11L12.6 7.3H17.8C18.9 7.3 19.6 8 19.6 9.1V17.5C19.6 18.5 18.9 19.2 17.8 19.2H6.2C5.1 19.2 4.4 18.5 4.4 17.5V7.2C4.4 6.2 5.1 5.5 6.2 5.5Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M8 11.4H16M8 14.2H14.2" fill="none" stroke="{accent}" stroke-width="1.5" stroke-linecap="round"/>
</svg>"#
        );

        img(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            svg.into_bytes(),
        )))
        .w(px(15.0))
        .h(px(15.0))
        .flex_none()
        .into_any_element()
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
                        this.child(
                            self.with_tooltip(
                                self.sidebar_button("›", "open-left-sidebar").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.left_sidebar_collapsed = false;
                                        this.save_app_state();
                                        cx.notify();
                                    }),
                                ),
                                "open-left-sidebar-tooltip",
                                "Show sidebar",
                                cx,
                            ),
                        )
                    })
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .child(self.tab_title(self.active_tab())),
                    ),
            )
            // Item count, "Monitoring"/"Scanning" status, and the
            // errors badge used to live here next to the tab title.
            // They were redundant with the sidebar (which shows the
            // same scan progress and links to the dedicated Scan
            // Errors page) and added visual noise to the header.
            // The metadata-sync pill stays — it surfaces async
            // network activity that has no other indicator.
            .when_some(self.render_metadata_status(cx), |this, status| {
                this.child(status)
            })
            .child(div().flex_1())
            .child(self.render_search_box(window, "Search library", cx))
            .child(
                self.with_tooltip(
                    self.sidebar_button("←", "navigate-back")
                        .opacity(if self.back_history.is_empty() {
                            0.4
                        } else {
                            1.0
                        })
                        .cursor(if self.back_history.is_empty() {
                            CursorStyle::Arrow
                        } else {
                            CursorStyle::PointingHand
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.navigate_back();
                            cx.notify();
                        })),
                    "navigate-back-tooltip",
                    "Back",
                    cx,
                ),
            )
            .child(
                self.with_tooltip(
                    self.sidebar_button("→", "navigate-forward")
                        .opacity(if self.forward_history.is_empty() {
                            0.4
                        } else {
                            1.0
                        })
                        .cursor(if self.forward_history.is_empty() {
                            CursorStyle::Arrow
                        } else {
                            CursorStyle::PointingHand
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.navigate_forward();
                            cx.notify();
                        })),
                    "navigate-forward-tooltip",
                    "Forward",
                    cx,
                ),
            )
            .child(self.with_tooltip(
                self.render_eq_header_button("library-open-eq", cx),
                "library-open-eq-tooltip",
                "Equalizer (right-click to toggle)",
                cx,
            ))
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
                // Always surface the reopen arrow whenever the right
                // sidebar is collapsed, even if the active view is
                // empty (e.g. Up Next with an empty queue). The
                // sidebar's own render path will show an empty-state
                // message when reopened, which is preferable to the
                // toggle silently disappearing.
                self.right_sidebar_collapsed,
                |this| {
                    this.child(
                        self.with_tooltip(
                            self.sidebar_button("‹", "open-right-sidebar")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.right_sidebar_collapsed = false;
                                    this.save_app_state();
                                    cx.notify();
                                })),
                            "open-right-sidebar-tooltip",
                            "Show queue sidebar",
                            cx,
                        ),
                    )
                },
            )
    }

    pub(super) fn render_metadata_status(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if self.online_metadata_mode != OnlineMetadataMode::Automatic
            || !self.metadata_activity.is_active()
        {
            return None;
        }

        let colors = *self.colors();
        let active = self.metadata_activity.running.max(1);
        let pending = self.metadata_activity.pending;
        let label = if pending > 0 {
            format!(
                "Syncing metadata: {active} active, {pending} queued{}",
                self.metadata_sync_eta_label()
                    .map(|eta| format!(" · about {eta} left"))
                    .unwrap_or_default()
            )
        } else {
            format!("Syncing metadata: {active} active")
        };

        Some(
            div()
                .id("metadata-sync-status")
                .text_xs()
                .text_color(rgb(colors.text_strong))
                .h(px(26.0))
                .px_2()
                .rounded_full()
                .bg(rgb(colors.elevated))
                .border_1()
                .border_color(rgb(colors.border))
                .flex()
                .items_center()
                .gap_2()
                .cursor_default()
                .child(self.metadata_sync_glyph(colors))
                .when(self.metadata_status_expanded, |this| {
                    this.child(
                        div()
                            .text_color(rgb(colors.text_strong))
                            .whitespace_nowrap()
                            .child(label),
                    )
                })
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    this.metadata_status_expanded = *hovered;
                    cx.notify();
                })),
        )
    }

    fn metadata_sync_glyph(&self, colors: ThemeColors) -> AnyElement {
        let color = format!("#{:06x}", colors.text_strong);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24">
<g transform-origin="12 12">
<animateTransform attributeName="transform" attributeType="XML" type="rotate" from="0 12 12" to="360 12 12" dur="1.1s" repeatCount="indefinite"/>
<path d="M12 4A8 8 0 1 1 4 12" fill="none" stroke="{color}" stroke-width="2.4" stroke-linecap="round"/>
<path d="M4 12L7 9M4 12L7 15" fill="none" stroke="{color}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>
</g>
</svg>"#
        );

        img(Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            svg.into_bytes(),
        )))
        .w(px(16.0))
        .h(px(16.0))
        .flex_none()
        .into_any_element()
    }

    /// Compute a wall-clock ETA for the pending enrichment queue.
    ///
    /// The worker is single-threaded and serializes all jobs, but each
    /// job's pacing wait is keyed on the source it hits (MusicBrainz,
    /// TheAudioDB, Wikipedia, Discogs, Cover Art Archive). Different
    /// sources don't share rate-limit slots, so jobs that target
    /// distinct sources can overlap the *waits* even though the work
    /// itself runs serially. The wall-clock floor is therefore the
    /// max across per-source totals, not the sum.
    ///
    /// Per-source delays mirror the worker's `*_DELAY` constants in
    /// `metadata_worker.rs`. Jobs that touch two sources sequentially
    /// (Wikipedia summary jobs hit MusicBrainz first, then Wikipedia)
    /// contribute to both totals.
    ///
    /// `fetch_artist_discography` and `fetch_artist_discogs_releases`
    /// paginate; we approximate at 3 pages per job on average.
    fn metadata_sync_eta_label(&self) -> Option<String> {
        let activity = &self.metadata_activity;

        // Per-source delays, in milliseconds. Match the worker's
        // `*_DELAY` constants (1.0s MB, 2.0s TADb, 0.25s Wikipedia,
        // 2.4s Discogs, 1.0s Lidarr). Cover Art Archive has no
        // explicit slot but is bound by HTTP latency; treat as
        // 0.5s/job.
        const MB_MS: u64 = 1_000;
        const TADB_MS: u64 = 2_000;
        const WIKI_MS: u64 = 250;
        const DISCOGS_MS: u64 = 2_400;
        const CAA_MS: u64 = 500;
        const LIDARR_MS: u64 = 1_000;
        const PAGES_AVG: u64 = 3;

        // MusicBrainz: artist resolve, album resolve, MB-based discography (paginated),
        // and the MB url-rels lookup that prefixes both Wikipedia jobs.
        let mb_jobs = activity.pending_artist_resolve as u64
            + activity.pending_album_resolve as u64
            + (activity.pending_artist_discography as u64) * PAGES_AVG
            + activity.pending_artist_wikipedia_summary as u64
            + activity.pending_album_wikipedia_summary as u64;
        let mb_total_ms = mb_jobs * MB_MS;

        // TheAudioDB: artist profile, album profile, TADb-search fallbacks for both.
        let tadb_jobs = activity.pending_artist_profile as u64
            + activity.pending_album_profile as u64
            + activity.pending_artist_audiodb_search as u64
            + activity.pending_album_audiodb_search as u64;
        let tadb_total_ms = tadb_jobs * TADB_MS;

        // Wikipedia REST summary: one call per Wikipedia summary job.
        let wiki_jobs = activity.pending_artist_wikipedia_summary as u64
            + activity.pending_album_wikipedia_summary as u64;
        let wiki_total_ms = wiki_jobs * WIKI_MS;

        // Discogs: identity searches, profile, releases (paginated),
        // album image, and per-row thumbs.
        let discogs_jobs = activity.pending_artist_discogs_search as u64
            + activity.pending_album_discogs_search as u64
            + activity.pending_artist_discogs_profile as u64
            + activity.pending_album_discogs_image as u64
            + (activity.pending_artist_discogs_releases as u64) * PAGES_AVG
            + activity.pending_thumb_fetch as u64;
        let discogs_total_ms = discogs_jobs * DISCOGS_MS;

        // Cover Art Archive: album cover fetches.
        let caa_total_ms = activity.pending_album_cover as u64 * CAA_MS;

        // Lidarr: tier-0 artist + album lookups. Single round-trip
        // each; the worker paces at LIDARR_DELAY (1s).
        let lidarr_jobs =
            activity.pending_artist_lidarr as u64 + activity.pending_album_lidarr as u64;
        let lidarr_total_ms = lidarr_jobs * LIDARR_MS;

        let estimated_ms = mb_total_ms
            .max(tadb_total_ms)
            .max(wiki_total_ms)
            .max(discogs_total_ms)
            .max(caa_total_ms)
            .max(lidarr_total_ms);

        if estimated_ms == 0 {
            return None;
        }

        Some(Self::metadata_duration_label(Duration::from_millis(
            estimated_ms,
        )))
    }

    fn metadata_duration_label(duration: Duration) -> String {
        let seconds = duration.as_secs();
        if seconds < 60 {
            return format!("{}s", seconds.max(1));
        }

        let minutes = seconds.div_ceil(60);
        if minutes < 60 {
            return format!("{minutes}m");
        }

        let hours = minutes / 60;
        let remaining_minutes = minutes % 60;
        if remaining_minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {remaining_minutes}m")
        }
    }

    pub(super) fn render_errors_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let total = self.total_error_count();
        let visible_rows = self.visible_error_rows();
        let visible_count = visible_rows.len();
        let subtitle = if total == 0 {
            "No errors".to_string()
        } else if visible_count == total {
            format!(
                "{total} {} from scans and online metadata",
                if total == 1 { "error" } else { "errors" }
            )
        } else {
            format!("{visible_count} of {total} errors visible (filters applied)")
        };

        div()
            .id("errors-page")
            .flex_1()
            .min_w_0()
            .bg(rgb(colors.surface))
            .flex()
            .flex_col()
            .child(self.render_simple_page_header("Errors", subtitle))
            .child(self.render_tab_bar(cx))
            .child(self.render_errors_badge_bar(cx))
            .child(
                div()
                    .id("errors-scroll")
                    .flex_1()
                    .min_h_0()
                    .child(self.render_errors_table(visible_rows, cx)),
            )
    }

    /// Toggleable badge bar rendered between the header and the table.
    /// One pill per `ErrorCategory`, plus All/None shortcuts on the
    /// right. Active pills use `colors.accent`; inactive pills are
    /// dimmed. Counts come from `error_counts_by_category` (full
    /// underlying state, not filtered).
    fn render_errors_badge_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let counts = self.error_counts_by_category();
        let active = self.active_error_filters.clone();

        let mut row = div()
            .id("errors-badge-bar")
            .flex_none()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.app))
            .flex()
            .items_center()
            .gap_2();

        for category in ErrorCategory::ALL {
            let count = counts.get(&category).copied().unwrap_or(0);
            let is_active = active.contains(&category);
            let label = format!("{} {}", category.label(), count);
            let id = SharedString::from(format!(
                "errors-badge-{}",
                category.label().to_lowercase().replace(' ', "-")
            ));
            let bg = if is_active {
                colors.accent
            } else {
                colors.elevated
            };
            let fg = if is_active {
                colors.text_strong
            } else {
                colors.text_muted
            };
            let border = if is_active {
                colors.accent
            } else {
                colors.border
            };
            let pill = div()
                .id(id)
                .h(px(24.0))
                .px_3()
                .rounded_full()
                .border_1()
                .border_color(rgb(border))
                .bg(rgb(bg))
                .text_xs()
                .text_color(rgb(fg))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_error_filter(category);
                    cx.notify();
                }));
            row = row.child(pill);
        }

        // Right-side spacer + All/None shortcuts.
        let all_button = div()
            .id("errors-badge-all")
            .h(px(24.0))
            .px_3()
            .rounded_full()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.elevated))
            .text_xs()
            .text_color(rgb(colors.text_muted))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child("All")
            .on_click(cx.listener(|this, _, _, cx| {
                this.set_all_error_filters(true);
                cx.notify();
            }));
        let none_button = div()
            .id("errors-badge-none")
            .h(px(24.0))
            .px_3()
            .rounded_full()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.elevated))
            .text_xs()
            .text_color(rgb(colors.text_muted))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child("None")
            .on_click(cx.listener(|this, _, _, cx| {
                this.set_all_error_filters(false);
                cx.notify();
            }));

        row.child(div().flex_1())
            .child(all_button)
            .child(none_button)
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

    fn render_errors_table(
        &self,
        rows: Vec<ErrorRowView>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let scroll_handle = self.errors_scroll_handle.clone();
        let item_count = rows.len();
        let total = self.total_error_count();
        let empty_message = if total == 0 {
            "No errors".to_string()
        } else {
            "No errors match the selected filters.".to_string()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.render_resizable_table_header(
                27.0,
                &[
                    ColumnResizeTarget::Error(ErrorColumn::Index),
                    ColumnResizeTarget::Error(ErrorColumn::Kind),
                    ColumnResizeTarget::Error(ErrorColumn::Path),
                    ColumnResizeTarget::Error(ErrorColumn::Error),
                ],
                cx,
            ))
            .when(item_count == 0, |this| {
                this.child(
                    div()
                        .p_5()
                        .text_color(rgb(colors.text_muted))
                        .child(empty_message),
                )
            })
            .when(item_count > 0, |this| {
                let rows = std::sync::Arc::new(rows);
                let rows_for_processor = rows.clone();
                this.child(
                    uniform_list(
                        "errors-rows",
                        item_count,
                        cx.processor(move |this, range: Range<usize>, _window, _cx| {
                            let visible = range.end.saturating_sub(range.start);
                            let _build_span = perf::span(
                                "errors.uniform_list.build",
                                format!("rows={} range={}..{}", visible, range.start, range.end),
                            );
                            range
                                .filter_map(|row_ix| {
                                    let view = rows_for_processor.get(row_ix)?.clone();
                                    Some(this.render_error_row(row_ix, view).into_any_element())
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

    fn render_error_row(&self, ix: usize, view: ErrorRowView) -> impl IntoElement {
        let colors = *self.colors();
        let category = view.category;

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
                    .w(px(self.error_column_width(ErrorColumn::Index)))
                    .flex_none()
                    .text_color(rgb(colors.text_faint))
                    .child((ix + 1).to_string()),
            )
            .child(
                div()
                    .w(px(self.error_column_width(ErrorColumn::Kind)))
                    .flex_none()
                    .child(self.render_error_category_badge(category)),
            )
            .child(
                div()
                    .w(px(self.error_column_width(ErrorColumn::Path)))
                    .flex_none()
                    .text_color(rgb(colors.text_strong))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(view.path_label),
            )
            .child(
                div()
                    .w(px(self.error_column_width(ErrorColumn::Error)))
                    .min_w_0()
                    .text_color(rgb(colors.text_muted))
                    .child(view.message),
            )
    }

    /// Inline pill rendered inside the TYPE column. Reuses the same
    /// dimmed-elevated pill styling as the badge bar so the visual
    /// vocabulary stays consistent across the page.
    fn render_error_category_badge(&self, category: ErrorCategory) -> impl IntoElement {
        let colors = *self.colors();
        div()
            .h(px(20.0))
            .px_2()
            .rounded_full()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.elevated))
            .text_xs()
            .text_color(rgb(colors.text_muted))
            .flex()
            .items_center()
            .justify_center()
            .child(category.label())
    }

    /// Format a precomputed library byte total. `TempoApp::library_size_bytes`
    /// is updated incrementally on every track add/update/remove, so the
    /// sidebar can call this on every render without iterating tracks.
    pub(super) fn format_library_size_bytes(bytes: u64) -> String {
        if bytes >= 1_000_000_000 {
            format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
        } else if bytes >= 1_000_000 {
            format!("{:.1} MB", bytes as f64 / 1_000_000.0)
        } else {
            format!("{} KB", bytes / 1_000)
        }
    }

    /// Compact integer formatter for sidebar counts and similar UI
    /// labels: `1234` -> `1.2K`, `1_500_000` -> `1.5M`, `2_000_000_000`
    /// -> `2.0B`. Counts under 1000 are printed as-is so small
    /// libraries don't get the "1K" treatment.
    ///
    /// The split between 4-digit and 5+digit thousands keeps "1.2K"
    /// for 1234 but `12K` for 12345 -- one decimal place buys you
    /// resolution where it matters and reads as noise once the
    /// integer part has two digits.
    pub(super) fn format_count_short(count: usize) -> String {
        const K: f64 = 1_000.0;
        const M: f64 = 1_000_000.0;
        const B: f64 = 1_000_000_000.0;
        let count_f = count as f64;
        if count_f >= B {
            format!("{:.1}B", count_f / B)
        } else if count_f >= M {
            format!("{:.1}M", count_f / M)
        } else if count >= 10_000 {
            format!("{:.0}K", count_f / K)
        } else if count >= 1_000 {
            format!("{:.1}K", count_f / K)
        } else {
            count.to_string()
        }
    }
}
