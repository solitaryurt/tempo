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
                self.page == Page::Library && self.active_tab().source == TabSource::Library,
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
        let active =
            self.page == Page::Library && self.active_tab().source == TabSource::Playlist(ix);
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

        row.on_click(cx.listener(move |this, _, _, cx| {
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
            .on_click(cx.listener(move |this, _, _, cx| {
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

        if collapsed || self.queue.is_empty() {
            return div().w(px(0.0)).flex_none().into_any_element();
        }

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
                                    cx.notify();
                                }),
                            ))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(colors.text_strong))
                                    .child("Up Next"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child(format!("{} tracks", self.queue.len())),
                    ),
            )
            .child(
                div().w(px(RIGHT_SIDEBAR_W)).children(
                    self.queue
                        .iter()
                        .filter(|track_ix| **track_ix < self.tracks.len())
                        .enumerate()
                        .map(|(ix, track_ix)| self.render_queue_row(ix, &self.tracks[*track_ix])),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_queue_row(&self, ix: usize, track: &Track) -> impl IntoElement {
        let active = ix == 0;
        let colors = *self.colors();
        let bg = if active {
            colors.queue_active
        } else {
            colors.queue
        };

        div()
            .h(px(41.0))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .bg(rgb(bg))
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

        // Transparent full-window click-eater behind the anchored menu.
        // Any mouse-down here dismisses the menu without triggering
        // whatever is underneath. The menu itself stops propagation on
        // its own mouse-down so item clicks still work.
        div()
            .id("playlist-context-menu-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.close_playlist_context_menu();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.close_playlist_context_menu();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(menu_at(
                menu.position,
                Corner::TopLeft,
                point(px(2.0), px(2.0)),
                panel.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    }),
                ),
            ))
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
}
