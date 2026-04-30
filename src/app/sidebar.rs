use super::*;
use std::sync::{Mutex, OnceLock};

/// Process-wide cache of pre-rasterized sidebar nav icons. Keyed by
/// `(target, active, color, accent)` so theme switches and the
/// active/inactive state both invalidate the right entries while still
/// giving every render after the first a cheap `Arc<Image>::clone`
/// instead of a fresh SVG encode.
type SidebarIconCacheKey = (Page, bool, u32, u32);
fn sidebar_icon_cache() -> &'static Mutex<HashMap<SidebarIconCacheKey, Arc<Image>>> {
    static CACHE: OnceLock<Mutex<HashMap<SidebarIconCacheKey, Arc<Image>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl TempoApp {
    pub(super) fn render_left_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.left_sidebar_collapsed;
        let colors = *self.colors();

        if collapsed {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

        div()
            .w(px(LEFT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .child(
                div()
                    .w(px(LEFT_SIDEBAR_W))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.render_sidebar_header(cx))
                    .child(self.render_library_nav(cx))
                    .child(self.render_playlists_nav(cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(colors.border))
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .flex()
                            .justify_between()
                            .child(format!(
                                "{} tracks",
                                Self::format_count_short(self.tracks.len())
                            ))
                            .child(Self::format_library_size_bytes(self.library_size_bytes)),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *self.colors();

        div()
            .h(px(50.0))
            .flex()
            .items_center()
            .px_4()
            .border_b_1()
            .border_color(rgb(colors.border_subtle))
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .child("Tempo"),
            )
            .child(
                self.sidebar_button("‹", "toggle-left-sidebar")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.left_sidebar_collapsed = !this.left_sidebar_collapsed;
                        cx.notify();
                    })),
            )
    }

    pub(super) fn sidebar_button(
        &self,
        label: &'static str,
        id: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = *self.colors();

        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.waveform_border))
            .bg(rgb(colors.button))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(colors.text_muted))
            .hover(move |this| {
                this.bg(rgb(colors.button_hover))
                    .text_color(rgb(colors.text_strong))
            })
            .active(|this| this.opacity(0.82))
            .child(label)
    }

    pub(super) fn render_library_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.nav_group_title("LIBRARY"))
            .child(self.render_nav_item(
                "All Music",
                Self::format_count_short(self.tracks.len()),
                self.page == Page::Library
                    && matches!(&self.active_tab().source, TabSource::Library),
                Page::Library,
                cx,
            ))
            .child(self.render_nav_item(
                "Artists",
                Self::format_count_short(self.artists.len()),
                self.page == Page::Artists,
                Page::Artists,
                cx,
            ))
            .child(self.render_nav_item(
                "Albums",
                Self::format_count_short(self.albums.len()),
                self.page == Page::Albums,
                Page::Albums,
                cx,
            ))
            .child(self.render_nav_item(
                "Genres",
                Self::format_count_short(self.genres.len()),
                self.page == Page::Genres,
                Page::Genres,
                cx,
            ))
            .child(self.render_nav_item(
                "Liked",
                Self::format_count_short(self.liked_track_count()),
                self.page == Page::Liked,
                Page::Liked,
                cx,
            ))
            .child(self.render_nav_item(
                "History",
                Self::format_count_short(self.playback_history.len()),
                self.page == Page::PlaybackHistory,
                Page::PlaybackHistory,
                cx,
            ))
            .when(self.scan_progress.errors > 0, |this| {
                this.child(self.render_nav_item(
                    "Scan Errors",
                    Self::format_count_short(self.scan_progress.errors),
                    self.page == Page::ScanErrors,
                    Page::ScanErrors,
                    cx,
                ))
            })
    }

    pub(super) fn render_playlists_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .px_3()
            .pb_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(self.nav_group_title("PLAYLISTS"))
                    .child(
                        self.sidebar_button("+", "new-playlist")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_playlist();
                                cx.notify();
                            })),
                    ),
            )
            .when(self.playlists.is_empty(), |this| {
                this.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(rgb(self.colors().text_faint))
                        .child("No playlists yet"),
                )
            })
            .children(
                self.playlists
                    .iter()
                    .enumerate()
                    .map(|(ix, playlist)| self.render_playlist_nav_item(ix, playlist, cx)),
            )
    }

    pub(super) fn nav_group_title(&self, title: &'static str) -> impl IntoElement {
        div()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(self.colors().text_faint))
            .child(title)
    }

    pub(super) fn render_playlist_nav_item(
        &self,
        ix: usize,
        playlist: &Playlist,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let active = self.page == Page::Library
            && matches!(&self.active_tab().source, TabSource::Playlist(active_ix) if *active_ix == ix);
        let colors = *self.colors();
        let bg = if active {
            colors.button_hover
        } else {
            colors.panel
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text
        };
        let renaming = self
            .playlist_rename
            .as_ref()
            .is_some_and(|rename| rename.playlist_ix == ix);

        let mut row = div()
            .id(SharedString::from(format!("playlist-{ix}")))
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(bg))
            .text_color(rgb(fg));

        if !renaming {
            row = row.cursor_pointer().active(|this| this.opacity(0.82));
        }

        let label_child: AnyElement = if renaming {
            self.render_playlist_rename_input(cx).into_any_element()
        } else {
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(playlist.name.clone())
                .into_any_element()
        };

        let count_child: AnyElement = div()
            .text_xs()
            .text_color(rgb(colors.text_faint))
            .child(Self::format_count_short(playlist.track_paths.len()))
            .into_any_element();

        let row = row.child(label_child).child(count_child);

        if renaming {
            // While renaming, suppress click-to-open and drop targets so
            // the input can take focus + accept text without triggering
            // tab navigation underneath.
            return row;
        }

        row.on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
            // Ctrl+click opens the playlist in the right sidebar
            // instead of taking over the main view with a new tab.
            if event.modifiers().control {
                this.right_sidebar_view = RightSidebarView::Playlist(ix);
                this.right_sidebar_collapsed = false;
                this.right_sidebar_view_menu_open = false;
                this.save_app_state();
                cx.notify();
                return;
            }
            this.open_playlist_tab(ix);
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.open_playlist_context_menu(ix, event.position);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
            this.add_track_to_playlist(drag.track_ix, ix);
            cx.notify();
        }))
    }

    /// Render the inline rename input that replaces the playlist label
    /// while a rename is in progress. Mirrors the search-input pattern:
    /// a focusable div that consumes key events and renders cursor /
    /// selection by hand.
    fn render_playlist_rename_input(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = *self.colors();
        let Some(rename) = self.playlist_rename.as_ref() else {
            return div().into_any_element();
        };
        let Some(focus_handle) = self.playlist_rename_focus_handle.as_ref() else {
            return div().into_any_element();
        };
        let text = rename.input.text().to_string();
        let selection = rename.input.selection_range();

        let mut children: Vec<AnyElement> = Vec::new();
        if let Some(range) = selection {
            if range.start > 0 {
                children.push(
                    div()
                        .flex_none()
                        .child(text[..range.start].to_string())
                        .into_any_element(),
                );
            }
            children.push(
                div()
                    .flex_none()
                    .rounded_sm()
                    .bg(rgb(colors.selected))
                    .text_color(rgb(colors.text_strong))
                    .child(text[range.clone()].to_string())
                    .into_any_element(),
            );
            if range.end < text.len() {
                children.push(
                    div()
                        .flex_none()
                        .child(text[range.end..].to_string())
                        .into_any_element(),
                );
            }
        } else {
            let cursor = rename.input.cursor();
            if cursor > 0 {
                children.push(
                    div()
                        .flex_none()
                        .child(text[..cursor].to_string())
                        .into_any_element(),
                );
            }
            // Block-style caret so the user can see where they are.
            children.push(
                div()
                    .flex_none()
                    .w(px(1.0))
                    .h(px(14.0))
                    .bg(rgb(colors.text_strong))
                    .into_any_element(),
            );
            if cursor < text.len() {
                children.push(
                    div()
                        .flex_none()
                        .child(text[cursor..].to_string())
                        .into_any_element(),
                );
            }
        }

        div()
            .id("playlist-rename-input")
            .min_w_0()
            .flex_1()
            .h(px(20.0))
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(colors.accent))
            .bg(rgb(colors.button))
            .text_color(rgb(colors.text_strong))
            .flex()
            .items_center()
            .overflow_hidden()
            .track_focus(focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_playlist_rename_key_down(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .children(children)
            .into_any_element()
    }

    pub(super) fn render_nav_item(
        &self,
        label: &'static str,
        count: impl Into<SharedString>,
        active: bool,
        target: Page,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let bg = if active {
            colors.button_hover
        } else {
            colors.panel
        };
        let fg = if active {
            colors.text_strong
        } else {
            colors.text
        };

        div()
            .id(SharedString::from(format!("nav-{label}")))
            .h(px(22.0))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .active(|this| this.opacity(0.82))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Self::sidebar_nav_icon(target, active, colors))
                    .child(div().overflow_hidden().text_ellipsis().child(label)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(count.into()),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                // Ctrl+click on the History nav item flips the right
                // sidebar to its History view instead of navigating
                // away from the current page. Other nav items don't
                // have meaningful right-sidebar peers yet so they
                // ignore the modifier.
                if event.modifiers().control && target == Page::PlaybackHistory {
                    this.right_sidebar_view = RightSidebarView::History;
                    this.right_sidebar_collapsed = false;
                    this.right_sidebar_view_menu_open = false;
                    this.save_app_state();
                    cx.notify();
                    return;
                }
                if target == Page::Library {
                    this.open_all_music_tab();
                } else {
                    this.open_page(target);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn sidebar_nav_icon(target: Page, active: bool, colors: ThemeColors) -> AnyElement {
        // Cache key uses the raw u32 colors so we don't have to format
        // strings before the lookup. The cache lives for the process
        // lifetime; theme changes simply add a few new entries.
        let color_u32 = if active {
            colors.text_strong
        } else {
            colors.text_muted
        };
        let accent_u32 = colors.accent;
        let cache_key = (target, active, color_u32, accent_u32);

        if let Ok(cache) = sidebar_icon_cache().lock()
            && let Some(image) = cache.get(&cache_key)
        {
            return img(Arc::clone(image))
                .w(px(15.0))
                .h(px(15.0))
                .flex_none()
                .into_any_element();
        }

        let color = format!("#{:06x}", color_u32);
        let accent = format!("#{:06x}", accent_u32);
        let accent_stroke = if active {
            accent.as_str()
        } else {
            color.as_str()
        };
        let paths = match target {
            Page::Library => format!(
                r#"<path d="M5 5.5H10.2C11.3 5.5 12 6.2 12 7.3V18.2C12 17.2 11.2 16.5 10.1 16.5H5V5.5Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M19 5.5H13.8C12.7 5.5 12 6.2 12 7.3V18.2C12 17.2 12.8 16.5 13.9 16.5H19V5.5Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M7.2 8.7H9.8M14.2 8.7H16.8M7.2 11.7H9.8M14.2 11.7H16.8" fill="none" stroke="{accent_stroke}" stroke-width="1.4" stroke-linecap="round"/>"#
            ),
            Page::Artists => format!(
                r#"<circle cx="9" cy="8" r="3" fill="none" stroke="{color}" stroke-width="1.6"/>
<path d="M3.8 18.5C4.7 15.6 6.5 14.2 9 14.2C11.5 14.2 13.3 15.6 14.2 18.5" fill="none" stroke="{color}" stroke-width="1.6" stroke-linecap="round"/>
<circle cx="16.5" cy="9.2" r="2.2" fill="none" stroke="{accent_stroke}" stroke-width="1.5"/>
<path d="M14.8 14.6C17.1 14.8 18.7 16.1 19.6 18.5" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>"#
            ),
            Page::Albums => format!(
                r#"<rect x="4.2" y="4.2" width="15.6" height="15.6" rx="2.2" fill="none" stroke="{color}" stroke-width="1.6"/>
<circle cx="12" cy="12" r="4.1" fill="none" stroke="{color}" stroke-width="1.6"/>
<circle cx="12" cy="12" r="1.1" fill="{accent_stroke}"/>
<path d="M15.1 8.9L17.1 6.9" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>"#
            ),
            Page::Genres => format!(
                r#"<path d="M5 17.5L9.8 6.2C10.2 5.3 11.2 4.9 12.1 5.3L18.9 8.2C19.8 8.6 20.2 9.6 19.8 10.5L15 21.8" fill="none" stroke="{color}" stroke-width="1.5" stroke-linecap="round"/>
<path d="M4.2 15.5L6.8 5.7C7 4.8 8 4.2 8.9 4.5L16.2 6.5" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>
<path d="M8.1 13.4L16.4 16.9M9.3 10.5L17.6 14" fill="none" stroke="{color}" stroke-width="1.2" stroke-linecap="round"/>"#
            ),
            Page::Liked => format!(
                r#"<path d="M12 19.3L4.9 12.4C3 10.5 3 7.5 4.9 5.6C6.8 3.7 9.8 3.7 11.7 5.6L12 5.9L12.3 5.6C14.2 3.7 17.2 3.7 19.1 5.6C21 7.5 21 10.5 19.1 12.4L12 19.3Z" fill="none" stroke="{accent_stroke}" stroke-width="1.7" stroke-linejoin="round"/>"#
            ),
            Page::PlaybackHistory => format!(
                r#"<circle cx="12" cy="12" r="7.6" fill="none" stroke="{color}" stroke-width="1.6"/>
<path d="M12 7.4V12L15.5 14.1" fill="none" stroke="{accent_stroke}" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>
<path d="M5.5 6.3L3.8 4.6M18.5 6.3L20.2 4.6" fill="none" stroke="{color}" stroke-width="1.4" stroke-linecap="round"/>"#
            ),
            Page::ScanErrors => format!(
                r#"<path d="M12 4.2L20 18.2H4L12 4.2Z" fill="none" stroke="{color}" stroke-width="1.6" stroke-linejoin="round"/>
<path d="M12 9V13" fill="none" stroke="{accent_stroke}" stroke-width="1.8" stroke-linecap="round"/>
<circle cx="12" cy="16" r="1" fill="{accent_stroke}"/>"#
            ),
            Page::Settings => format!(
                r#"<circle cx="12" cy="12" r="3" fill="none" stroke="{color}" stroke-width="1.6"/>
<path d="M12 4.5V6.5M12 17.5V19.5M4.5 12H6.5M17.5 12H19.5M6.7 6.7L8.1 8.1M15.9 15.9L17.3 17.3M17.3 6.7L15.9 8.1M8.1 15.9L6.7 17.3" fill="none" stroke="{accent_stroke}" stroke-width="1.5" stroke-linecap="round"/>"#
            ),
        };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">{paths}</svg>"#
        );

        let image = Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()));
        if let Ok(mut cache) = sidebar_icon_cache().lock() {
            cache.insert(cache_key, Arc::clone(&image));
        }

        img(image)
            .w(px(15.0))
            .h(px(15.0))
            .flex_none()
            .into_any_element()
    }

    pub(super) fn render_queue(&self, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.right_sidebar_collapsed;
        let colors = *self.colors();
        // Sanitize the view: a Playlist(ix) variant becomes Queue if
        // the playlist was deleted out from under us. This keeps the
        // sidebar from disappearing on a stale persisted state.
        let view = self.sanitized_right_sidebar_view();

        // Pre-filter the queue to drop stale indices (indices >=
        // tracks.len() can linger after a rescan). The same filtered
        // list is fed to both the row count and the virtual list's
        // processor so they stay in sync.
        let queue_indices: Vec<usize> = self
            .queue
            .iter()
            .copied()
            .filter(|track_ix| *track_ix < self.tracks.len())
            .collect();
        let queue_empty = queue_indices.is_empty();
        let history_indices = self.sorted_playback_history_indices();
        let history_empty = history_indices.is_empty();
        let playlist_track_ix_for_view = match view {
            RightSidebarView::Playlist(ix) => Some(self.playlist_track_indices(ix)),
            _ => None,
        };

        let active_view_empty = match view {
            RightSidebarView::Queue => queue_empty,
            RightSidebarView::History => history_empty,
            RightSidebarView::Playlist(_) => playlist_track_ix_for_view
                .as_ref()
                .map(|ix| ix.is_empty())
                .unwrap_or(true),
        };

        // Only collapse the sidebar entirely when the user explicitly
        // closed it, or when there is genuinely nothing across any
        // view to switch to. An empty *active* view should still show
        // the sidebar chrome with an empty-state message — silently
        // closing the sidebar after the user picks "Up Next" while
        // the queue is empty made it look like the click was a no-op.
        if collapsed {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

        let count_text = match view {
            RightSidebarView::Queue => format!("{} tracks", queue_indices.len()),
            RightSidebarView::History => format!("{} plays", history_indices.len()),
            RightSidebarView::Playlist(_) => format!(
                "{} tracks",
                playlist_track_ix_for_view
                    .as_ref()
                    .map(|ix| ix.len())
                    .unwrap_or(0)
            ),
        };

        let body: AnyElement = if active_view_empty {
            self.render_right_sidebar_empty_state(view, cx)
        } else {
            match view {
                RightSidebarView::Queue => self
                    .render_queue_virtual_list(queue_indices, cx)
                    .into_any_element(),
                RightSidebarView::History => self
                    .render_history_virtual_list(history_indices, cx)
                    .into_any_element(),
                RightSidebarView::Playlist(_) => self
                    .render_playlist_virtual_list(
                        playlist_track_ix_for_view.unwrap_or_default(),
                        cx,
                    )
                    .into_any_element(),
            }
        };

        div()
            .w(px(RIGHT_SIDEBAR_W))
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_l_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.queue))
            .child(
                div()
                    .w(px(RIGHT_SIDEBAR_W))
                    .h(px(54.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(self.sidebar_button("›", "toggle-right-sidebar").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.right_sidebar_collapsed = !this.right_sidebar_collapsed;
                                    this.right_sidebar_view_menu_open = false;
                                    cx.notify();
                                }),
                            ))
                            .child(self.render_right_sidebar_view_trigger(cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(colors.text_faint))
                                    .child(count_text),
                            )
                            // Clear-queue button only makes sense in
                            // the Queue view; History has no
                            // user-visible clear action yet.
                            .when(matches!(view, RightSidebarView::Queue), |this| {
                                this.child(self.sidebar_button("✕", "clear-queue").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.clear_queue(cx);
                                    }),
                                ))
                            }),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    /// Virtualized list of Up Next queue rows. Reuses the
    /// `uniform_list` pattern from the History page / Liked page so
    /// only the visible rows are constructed each frame -- a 10k-track
    /// queue still renders in O(viewport) time. The end-of-list drop
    /// zone is appended *after* the virtual list so dropping past the
    /// last visible item still appends.
    fn render_queue_virtual_list(
        &self,
        queue_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let item_count = queue_indices.len();
        let scroll_handle = self.queue_sidebar_scroll_handle.clone();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                uniform_list(
                    "queue-rows",
                    item_count,
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        let _build_span = perf::span(
                            "queue.uniform_list.build",
                            format!(
                                "rows={} range={}..{}",
                                range.end.saturating_sub(range.start),
                                range.start,
                                range.end
                            ),
                        );
                        range
                            .filter_map(|row_ix| {
                                let track_ix = queue_indices.get(row_ix).copied()?;
                                let track = this.tracks.get(track_ix)?;
                                Some(
                                    this.render_queue_row(row_ix, track_ix, track, cx)
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
            // End-of-queue drop zone so users can drop a row past the
            // last item to append. Sits below the virtual list as a
            // fixed 24px strip.
            .child(self.render_queue_end_drop_zone(cx))
    }

    /// Virtualized list of playback history rows.
    fn render_history_virtual_list(
        &self,
        history_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let item_count = history_indices.len();
        let scroll_handle = self.history_sidebar_scroll_handle.clone();
        let colors = *self.colors();

        div().flex_1().min_h_0().child(
            uniform_list(
                "history-rows",
                item_count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    let _build_span = perf::span(
                        "history_sidebar.uniform_list.build",
                        format!(
                            "rows={} range={}..{}",
                            range.end.saturating_sub(range.start),
                            range.start,
                            range.end
                        ),
                    );
                    range
                        .filter_map(|row_ix| {
                            let history_index = history_indices.get(row_ix).copied()?;
                            let entry = this.playback_history.get(history_index)?;
                            let resolved = this
                                .tracks
                                .iter()
                                .position(|track| track.path == entry.track_path);
                            Some(
                                this.render_history_row(history_index, entry, resolved, colors, cx)
                                    .into_any_element(),
                            )
                        })
                        .collect()
                }),
            )
            .size_full()
            .track_scroll(scroll_handle),
        )
    }

    /// Empty-state body rendered when the active right-sidebar view
    /// has no rows. Keeps the sidebar chrome (header + view picker)
    /// visible so the user can switch to a populated view rather than
    /// having the whole panel disappear out from under them.
    fn render_right_sidebar_empty_state(
        &self,
        view: RightSidebarView,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();
        let (title, hint) = match view {
            RightSidebarView::Queue => (
                "Up Next is empty",
                "Right-click a track and choose \"Add to queue\" to line up plays.",
            ),
            RightSidebarView::History => (
                "No plays yet",
                "Tracks you listen to for at least 15 seconds show up here.",
            ),
            RightSidebarView::Playlist(_) => (
                "Playlist is empty",
                "Drop tracks into this playlist from the table or right-click menu.",
            ),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px_4()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_muted))
                    .child(title),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .text_center()
                    .child(hint),
            )
            .into_any_element()
    }

    /// Header label that doubles as a click target to toggle the
    /// view-picker dropdown. Renders the active view's name plus a
    /// chevron, matching the player-bar output-device dropdown
    /// pattern.
    fn render_right_sidebar_view_trigger(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let label: SharedString = match self.sanitized_right_sidebar_view() {
            RightSidebarView::Queue => "Up Next".into(),
            RightSidebarView::History => "History".into(),
            RightSidebarView::Playlist(ix) => self
                .playlists
                .get(ix)
                .map(|playlist| SharedString::from(playlist.name.clone()))
                .unwrap_or_else(|| "Up Next".into()),
        };

        div()
            .id("right-sidebar-view-trigger")
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(colors.text_strong))
                    .overflow_hidden()
                    .text_ellipsis()
                    .max_w(px(160.0))
                    .child(label),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child("▾"),
            )
            .hover(move |this| this.text_color(rgb(colors.text)))
            // We use `on_mouse_down` (not `on_click`) so the menu can
            // anchor at the click position. Stop propagation so the
            // newly-opened menu's backdrop doesn't immediately close
            // the menu on the same mouse-down.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let opening = !this.right_sidebar_view_menu_open;
                    this.right_sidebar_view_menu_open = opening;
                    if opening {
                        this.right_sidebar_view_menu_position = event.position;
                    }
                    this.queue_context_menu = None;
                    this.history_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// 24px-tall drop target appended after the last queue row so
    /// drops past the end of the list are accepted. Visually empty in
    /// the resting state; styled drop indicator could be added later.
    fn render_queue_end_drop_zone(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let queue_len = self.queue.len();
        div()
            .id("queue-end-drop-zone")
            .h(px(24.0))
            .w(px(RIGHT_SIDEBAR_W))
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                this.insert_in_queue(queue_len, drag.track_ix, cx);
            }))
            .on_drop(cx.listener(move |this, drag: &QueueRowDrag, _window, cx| {
                this.move_queue_entry(drag.queue_position, queue_len, cx);
            }))
    }

    pub(super) fn render_queue_row(
        &self,
        ix: usize,
        track_ix: usize,
        track: &Track,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        // Active row tracks the queue cursor (which entry is currently
        // playing from the queue). When nothing is playing from the
        // queue (`queue_cursor == None`), no row is highlighted.
        let active = self.queue_cursor == Some(ix);
        let colors = *self.colors();
        let bg = if active {
            colors.queue_active
        } else {
            colors.queue
        };
        let drag_track = track.clone();
        let row_id = SharedString::from(format!("queue-row-{ix}"));

        div()
            .id(row_id)
            .h(px(41.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(bg))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.queue_active)))
            .child(
                div()
                    .w(px(22.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(format!("{}", ix + 1)),
            )
            .child(artwork::album_tile(track, 28.0, colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(if active {
                                colors.accent
                            } else {
                                colors.text_strong
                            }))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(track.artist.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(track.duration.clone()),
            )
            // Left-click: play this queue entry. The entry stays in
            // place; only the cursor moves. That keeps the visible
            // queue intact so the user can scrub backward through
            // recently-played queue items if they want.
            .on_click(cx.listener(move |this, _, _, cx| {
                this.play_queue_entry(ix, cx);
                cx.notify();
            }))
            // Right-click: open the queue context menu anchored at the
            // mouse-down position.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.queue_context_menu = Some(QueueContextMenu {
                        queue_position: ix,
                        position: event.position,
                    });
                    this.history_context_menu = None;
                    this.right_sidebar_view_menu_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            // Drag source: in-queue reorder + cross-list moves.
            .on_drag(
                QueueRowDrag::new(ix, track_ix, &drag_track),
                |drag: &QueueRowDrag, position, _, cx| {
                    let preview = drag.clone().position(position);
                    cx.new(|_| preview)
                },
            )
            // Drop target for an external `TrackDrag`: insert above
            // this row.
            .on_drop(cx.listener(move |this, drag: &TrackDrag, _window, cx| {
                this.insert_in_queue(ix, drag.track_ix, cx);
            }))
            // Drop target for a `QueueRowDrag`: move within the queue.
            .on_drop(cx.listener(move |this, drag: &QueueRowDrag, _window, cx| {
                this.move_queue_entry(drag.queue_position, ix, cx);
            }))
    }

    /// Right-click context menu for sidebar playlist nav items. The
    /// menu itself is anchored at the mouse-down position; a
    /// transparent full-window backdrop sits behind it so any
    /// mouse-down outside the menu dismisses it.
    pub(super) fn render_playlist_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let menu = self
            .playlist_context_menu
            .expect("called only when context menu is open");
        let playlist_ix = menu.playlist_ix;
        let name = self
            .playlists
            .get(playlist_ix)
            .map(|playlist| playlist.name.clone())
            .unwrap_or_default();
        let colors = *self.colors();

        let panel = menu_panel(190.0, colors)
            .child(menu_header(name, colors))
            .child(
                self.context_menu_item("Open")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_playlist_context_menu();
                        this.open_playlist_tab(playlist_ix);
                        cx.notify();
                    })),
            )
            .child(self.context_menu_item("Rename").on_click(cx.listener(
                move |this, _, window, cx| {
                    this.start_playlist_rename(playlist_ix, window, cx);
                    cx.notify();
                },
            )))
            .child(self.context_menu_item("Delete").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.request_delete_playlist(playlist_ix);
                    cx.notify();
                },
            )));

        menu_at(
            menu.position,
            Corner::TopLeft,
            point(px(2.0), px(2.0)),
            panel.on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            ),
        )
    }

    /// Centered modal dialog asking the user to confirm playlist
    /// deletion. The backdrop intercepts mouse-down events so clicking
    /// outside the dialog dismisses it without triggering whatever was
    /// behind it (sidebar buttons, table rows, etc.).
    pub(super) fn render_playlist_delete_confirm(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let playlist_ix = self
            .playlist_delete_confirm
            .expect("called only when delete confirm is open");
        let name = self
            .playlists
            .get(playlist_ix)
            .map(|playlist| playlist.name.clone())
            .unwrap_or_default();
        let colors = *self.colors();

        // Full-window backdrop. We layer the dialog inside it (centered
        // via flex) instead of using `anchored()` because the dialog is
        // modal -- we want it pinned to viewport center, not anchored
        // near the click site.
        div()
            .id("playlist-delete-confirm-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.cancel_delete_playlist();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("playlist-delete-confirm-dialog")
                    .w(px(360.0))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(colors.border_strong))
                    .bg(rgb(colors.elevated))
                    .shadow_lg()
                    .overflow_hidden()
                    // Eat clicks inside the dialog so they don't bubble
                    // up to the backdrop and trigger a cancel.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(colors.border))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .child("Delete playlist?"),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .text_color(rgb(colors.text))
                            .child(format!(
                                "\"{name}\" will be removed from your library. \
                                 The audio files on disk are unchanged."
                            )),
                    )
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(colors.border))
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("playlist-delete-cancel")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(colors.border))
                                    .bg(rgb(colors.button))
                                    .text_color(rgb(colors.text))
                                    .cursor_pointer()
                                    .hover(move |this| {
                                        this.bg(rgb(colors.button_hover))
                                            .text_color(rgb(colors.text_strong))
                                    })
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_delete_playlist();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("playlist-delete-confirm")
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(colors.accent))
                                    .text_color(rgb(colors.text_strong))
                                    .cursor_pointer()
                                    .hover(|this| this.opacity(0.85))
                                    .child("Delete")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_delete_playlist();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
    }

    fn render_history_row(
        &self,
        history_index: usize,
        entry: &PlaybackHistoryEntry,
        resolved_track_ix: Option<usize>,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row_id = SharedString::from(format!("history-row-{history_index}"));
        let title_color = if resolved_track_ix.is_some() {
            colors.text_strong
        } else {
            colors.text_faint
        };
        let subtitle_color = if resolved_track_ix.is_some() {
            colors.text_muted
        } else {
            colors.text_faint
        };
        let dim = resolved_track_ix.is_none();

        let mut row = div()
            .id(row_id)
            .h(px(41.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(colors.queue))
            .when(!dim, |this| {
                this.cursor_pointer()
                    .hover(move |this| this.bg(rgb(colors.queue_active)))
            })
            .child(
                // Fixed-width relative-time gutter ("now", "5m", "23h",
                // "364d"). `flex_none` + `overflow_hidden` keep
                // 3-character labels on a single line — without these
                // the parent flex squeezed the column and "25m" wrapped
                // to two lines.
                div()
                    .w(px(28.0))
                    .flex_none()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(Self::format_history_relative(entry.played_at_unix_secs)),
            );

        // Optional artwork tile when the track is still in the
        // library; otherwise a small placeholder so the layout stays
        // stable.
        if let Some(track_ix) = resolved_track_ix {
            row = row.child(artwork::album_tile(&self.tracks[track_ix], 28.0, colors));
        } else {
            row = row.child(
                div()
                    .w(px(28.0))
                    .h(px(28.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.panel))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child("♪"),
            );
        }

        row = row
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(title_color))
                            .child(SharedString::from(entry.title.clone())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(subtitle_color))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from(entry.artist.clone())),
                    ),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(SharedString::from(entry.duration.clone())),
            )
            // Right-click menu (works whether or not the track resolves
            // -- the "Remove from history" action stays available).
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.history_context_menu = Some(HistoryContextMenu {
                        history_index,
                        position: event.position,
                    });
                    this.queue_context_menu = None;
                    this.right_sidebar_view_menu_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            );

        if let Some(track_ix) = resolved_track_ix {
            row = row.on_click(cx.listener(move |this, _, _, cx| {
                if track_ix < this.tracks.len() {
                    this.play_track(track_ix, cx);
                    cx.notify();
                }
            }));
        }

        row
    }

    /// Format a unix-secs timestamp as a short relative-time label
    /// suitable for the narrow history-row gutter ("now", "5m", "2h",
    /// "3d", "6w"). Falls back to "—" for clock skew or missing data.
    fn format_history_relative(played_at_unix_secs: u64) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if played_at_unix_secs == 0 || played_at_unix_secs > now {
            return "—".to_string();
        }
        let diff = now - played_at_unix_secs;
        if diff < 60 {
            "now".to_string()
        } else if diff < 3600 {
            format!("{}m", diff / 60)
        } else if diff < 86_400 {
            format!("{}h", diff / 3600)
        } else if diff < 604_800 {
            format!("{}d", diff / 86_400)
        } else {
            format!("{}w", diff / 604_800)
        }
    }

    /// Right-click context menu for an Up Next queue row. Mirrors the
    /// playlist context menu's backdrop pattern so any outside click
    /// dismisses it.
    pub(super) fn render_queue_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let menu = self
            .queue_context_menu
            .expect("called only when queue context menu is open");
        let queue_position = menu.queue_position;
        let track_label = self
            .queue
            .get(queue_position)
            .and_then(|track_ix| self.tracks.get(*track_ix))
            .map(|track| track.title.clone())
            .unwrap_or_default();
        let colors = *self.colors();
        let queue_len = self.queue.len();
        let can_move_up = queue_position > 0;
        let can_move_down = queue_position + 1 < queue_len;

        let mut panel = menu_panel(200.0, colors)
            .child(menu_header(track_label, colors))
            .child(self.context_menu_item("Play now").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.queue_context_menu = None;
                    // Cursor-style: leave the entry in place; just
                    // move the cursor and play.
                    this.play_queue_entry(queue_position, cx);
                    cx.notify();
                },
            )));

        if can_move_up {
            panel = panel.child(self.context_menu_item("Move to top").on_click(cx.listener(
                move |this, _, _, cx| {
                    this.queue_context_menu = None;
                    this.move_queue_entry(queue_position, 0, cx);
                },
            )));
        }
        if can_move_down {
            panel = panel.child(
                self.context_menu_item("Move to bottom")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.queue_context_menu = None;
                        let len = this.queue.len();
                        this.move_queue_entry(queue_position, len, cx);
                    })),
            );
        }

        panel = panel
            .child(
                self.context_menu_item("Remove from queue")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.remove_queue_entry(queue_position, cx);
                    })),
            )
            .child(self.context_menu_item("Clear queue").on_click(cx.listener(
                |this, _, _, cx| {
                    this.queue_context_menu = None;
                    this.clear_queue(cx);
                },
            )));

        menu_at(
            menu.position,
            Corner::TopLeft,
            point(px(2.0), px(2.0)),
            panel.on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            ),
        )
    }

    /// Right-click context menu for a History row.
    pub(super) fn render_history_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let menu = self
            .history_context_menu
            .expect("called only when history context menu is open");
        let history_index = menu.history_index;
        let colors = *self.colors();
        let entry = self.playback_history.get(history_index);
        let title = entry.map(|e| e.title.clone()).unwrap_or_default();
        let resolved_track_ix = entry.and_then(|e| {
            self.tracks
                .iter()
                .position(|track| track.path == e.track_path)
        });

        let mut panel = menu_panel(210.0, colors).child(menu_header(title, colors));

        if let Some(track_ix) = resolved_track_ix {
            panel = panel
                .child(self.context_menu_item("Play now").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.history_context_menu = None;
                        if track_ix < this.tracks.len() {
                            this.play_track(track_ix, cx);
                        }
                        cx.notify();
                    },
                )))
                .child(
                    self.context_menu_item("Add to start of queue")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.history_context_menu = None;
                            this.queue_track_at_start(track_ix);
                            cx.notify();
                        })),
                )
                .child(
                    self.context_menu_item("Add to end of queue")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.history_context_menu = None;
                            this.queue_track_at_end(track_ix);
                            cx.notify();
                        })),
                );
        }

        panel = panel.child(
            self.context_menu_item("Remove from history")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.history_context_menu = None;
                    if history_index < this.playback_history.len() {
                        this.playback_history.remove(history_index);
                        this.save_app_state();
                    }
                    cx.notify();
                })),
        );

        menu_at(
            menu.position,
            Corner::TopLeft,
            point(px(2.0), px(2.0)),
            panel.on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            ),
        )
    }

    /// Dropdown shown when the user clicks the "Up Next ▾" / "History
    /// ▾" / "<playlist> ▾" header label. Anchored at the click
    /// position recorded when the dropdown was opened.
    pub(super) fn render_right_sidebar_view_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();
        let position = self.right_sidebar_view_menu_position;
        let active = self.right_sidebar_view;

        let queue_label = if matches!(active, RightSidebarView::Queue) {
            "Up Next  ✓"
        } else {
            "Up Next"
        };
        let history_label = if matches!(active, RightSidebarView::History) {
            "History  ✓"
        } else {
            "History"
        };

        let mut panel = menu_panel(200.0, colors)
            .child(
                self.context_menu_item_dynamic(queue_label.to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.right_sidebar_view = RightSidebarView::Queue;
                        this.right_sidebar_view_menu_open = false;
                        this.save_app_state();
                        cx.notify();
                    })),
            )
            .child(
                self.context_menu_item_dynamic(history_label.to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.right_sidebar_view = RightSidebarView::History;
                        this.right_sidebar_view_menu_open = false;
                        this.save_app_state();
                        cx.notify();
                    })),
            );

        // Per-playlist entries grouped under a small section label.
        if !self.playlists.is_empty() {
            panel = panel.child(menu_section_label("PLAYLISTS", colors));
            for (ix, playlist) in self.playlists.iter().enumerate() {
                let active_mark = if matches!(active, RightSidebarView::Playlist(p) if p == ix) {
                    "  ✓"
                } else {
                    ""
                };
                let label = format!("{}{}", playlist.name, active_mark);
                panel = panel.child(self.context_menu_item_dynamic(label).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.right_sidebar_view = RightSidebarView::Playlist(ix);
                        this.right_sidebar_view_menu_open = false;
                        this.right_sidebar_collapsed = false;
                        this.save_app_state();
                        cx.notify();
                    },
                )));
            }
        }

        menu_at(
            position,
            Corner::TopLeft,
            point(px(2.0), px(6.0)),
            panel.on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                }),
            ),
        )
    }

    /// Returns the active right-sidebar view, falling back to `Queue`
    /// when the view points at a playlist index that no longer
    /// exists. Read-side guard so render code never has to special-
    /// case a stale `Playlist(ix)` itself.
    pub(super) fn sanitized_right_sidebar_view(&self) -> RightSidebarView {
        match self.right_sidebar_view {
            RightSidebarView::Playlist(ix) if ix >= self.playlists.len() => RightSidebarView::Queue,
            other => other,
        }
    }

    /// Resolve a playlist's stored `track_paths` to current
    /// `self.tracks` indices. Mirrors the resolution logic in
    /// `source_track_indices` so missing files are silently dropped.
    pub(super) fn playlist_track_indices(&self, playlist_ix: usize) -> Vec<usize> {
        let Some(playlist) = self.playlists.get(playlist_ix) else {
            return Vec::new();
        };
        playlist
            .track_paths
            .iter()
            .filter_map(|path| self.tracks.iter().position(|track| &track.path == path))
            .collect()
    }

    /// Virtualized list for the right sidebar's playlist view. Mirrors
    /// `render_history_virtual_list` so a 10k-track playlist is still
    /// O(viewport) to render.
    fn render_playlist_virtual_list(
        &self,
        track_indices: Vec<usize>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let item_count = track_indices.len();
        let scroll_handle = self.playlist_sidebar_scroll_handle.clone();
        let colors = *self.colors();

        div().flex_1().min_h_0().child(
            uniform_list(
                "playlist-sidebar-rows",
                item_count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    let _build_span = perf::span(
                        "playlist_sidebar.uniform_list.build",
                        format!(
                            "rows={} range={}..{}",
                            range.end.saturating_sub(range.start),
                            range.start,
                            range.end
                        ),
                    );
                    range
                        .filter_map(|row_ix| {
                            let track_ix = track_indices.get(row_ix).copied()?;
                            let track = this.tracks.get(track_ix)?;
                            Some(
                                this.render_playlist_sidebar_row(
                                    row_ix, track_ix, track, colors, cx,
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
    }

    /// Single row in the right sidebar's playlist view. Click plays
    /// the track (resetting the queue cursor since the user is
    /// playing outside the queue); right-click offers the standard
    /// queue actions.
    fn render_playlist_sidebar_row(
        &self,
        row_ix: usize,
        track_ix: usize,
        track: &Track,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row_id = SharedString::from(format!("playlist-sidebar-row-{row_ix}"));
        let active = self.playing_track == track_ix;

        div()
            .id(row_id)
            .h(px(41.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(if active {
                colors.queue_active
            } else {
                colors.queue
            }))
            .cursor_pointer()
            .hover(move |this| this.bg(rgb(colors.queue_active)))
            .child(
                div()
                    .w(px(22.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(format!("{}", row_ix + 1)),
            )
            .child(artwork::album_tile(track, 28.0, colors))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(if active {
                                colors.accent
                            } else {
                                colors.text_strong
                            }))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(track.artist.clone()),
                    ),
            )
            .child(
                div()
                    .w(px(42.0))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(track.duration.clone()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if track_ix < this.tracks.len() {
                    this.play_track(track_ix, cx);
                    cx.notify();
                }
            }))
    }
}
