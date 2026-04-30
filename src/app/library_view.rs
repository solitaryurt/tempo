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
            Page::ScanErrors => self.render_scan_errors_page(cx).into_any_element(),
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
            .child(self.render_scan_status(cx))
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

    fn metadata_sync_eta_label(&self) -> Option<String> {
        let activity = &self.metadata_activity;
        let estimated_seconds = activity.pending_artist_resolve
            + activity.pending_artist_discography
            + activity.pending_album_resolve
            + activity.pending_album_cover
            + (activity.pending_artist_profile * 2);

        if estimated_seconds == 0 {
            return None;
        }

        Some(Self::metadata_duration_label(Duration::from_secs(
            estimated_seconds as u64,
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
            .child(self.render_tab_bar(cx))
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
                            let visible = range.end.saturating_sub(range.start);
                            let _build_span = perf::span(
                                "scan_errors.uniform_list.build",
                                format!("rows={} range={}..{}", visible, range.start, range.end),
                            );
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
