use super::*;

impl TempoApp {
    pub(super) fn start_playback_tick(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;

                if this
                    .update(cx, |app, cx| {
                        if app.is_playing
                            && app
                                .playback
                                .as_ref()
                                .is_some_and(|playback| playback.is_empty())
                        {
                            app.play_finished_track();
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    pub(super) fn remove_track_from_queue(&mut self, removed_ix: usize) {
        self.queue = self
            .queue
            .iter()
            .filter_map(|track_ix| {
                if *track_ix == removed_ix {
                    None
                } else if *track_ix > removed_ix {
                    Some(*track_ix - 1)
                } else {
                    Some(*track_ix)
                }
            })
            .collect();
    }

    pub(super) fn queue_track(&mut self, track_ix: usize) {
        if track_ix >= self.tracks.len() {
            return;
        }

        self.queue.push(track_ix);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    pub(super) fn queue_album_from_track(&mut self, track_ix: usize, shuffled: bool) {
        let Some(album) = self.tracks.get(track_ix).map(|track| track.album.clone()) else {
            return;
        };

        let mut album_tracks = self
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(ix, track)| (track.album == album).then_some(ix))
            .collect::<Vec<_>>();

        if shuffled {
            let seed = Self::shuffle_seed();
            album_tracks.sort_by_key(|track_ix| {
                Self::shuffle_key(&self.tracks[*track_ix], *track_ix, seed)
            });
        }

        self.queue.extend(album_tracks);
        self.right_sidebar_collapsed = false;
        self.context_menu_track = None;
    }

    pub(super) fn shuffle_seed() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_default()
    }

    pub(super) fn shuffle_key(track: &Track, track_ix: usize, seed: u64) -> u64 {
        let mut hash = seed ^ ((track_ix as u64).wrapping_mul(0x9e3779b97f4a7c15));

        for part in [&track.title, &track.artist, &track.album] {
            for byte in part.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }

        hash ^ (hash >> 33)
    }

    pub(super) fn clamp_track_indices(&mut self) {
        if self.tracks.is_empty() {
            for tab in &mut self.tabs {
                tab.selected_track = 0;
            }
            self.playing_track = 0;
            self.context_menu_track = None;
            self.is_playing = false;
            return;
        }

        let last = self.tracks.len() - 1;
        self.playing_track = self.playing_track.min(last);

        for tab_ix in 0..self.tabs.len() {
            let selected_track = self.tabs[tab_ix].selected_track.min(last);
            let replacement_track = {
                let indices = self.track_indices_for_tab(tab_ix);
                (!indices.contains(&selected_track)).then(|| indices.first().copied().unwrap_or(0))
            };
            let tab = &mut self.tabs[tab_ix];
            tab.selected_track = replacement_track.unwrap_or(selected_track);
        }

        if self
            .context_menu_track
            .is_some_and(|track_ix| track_ix > last)
        {
            self.context_menu_track = None;
        }

        self.queue.retain(|track_ix| *track_ix <= last);
    }

    pub(super) fn play_track(&mut self, track_ix: usize) {
        let Some(track) = self.tracks.get(track_ix) else {
            return;
        };
        let track_path = track.path.clone();

        self.playing_track = track_ix;
        self.set_active_selected_track(track_ix);
        self.context_menu_track = None;

        let Some(playback) = &self.playback else {
            self.is_playing = false;
            return;
        };

        match playback.play_path(&track_path) {
            Ok(()) => {
                self.is_playing = true;
                self.playback_status = "Playing through default output".to_string();
            }
            Err(error) => {
                self.is_playing = false;
                self.playback_status = format!("Playback failed: {error:#}");
            }
        }
    }

    pub(super) fn toggle_playback(&mut self) {
        if self.tracks.is_empty() {
            return;
        }

        if self.is_playing {
            if let Some(playback) = &self.playback {
                playback.pause();
            }
            self.is_playing = false;
            self.playback_status = "Playback paused".to_string();
            self.context_menu_track = None;
            return;
        }

        if self
            .playback
            .as_ref()
            .is_some_and(|playback| playback.is_empty())
        {
            self.play_track(self.playing_track);
            return;
        }

        if let Some(playback) = &self.playback {
            playback.resume();
            self.is_playing = true;
            self.playback_status = "Playing through default output".to_string();
        }

        self.context_menu_track = None;
    }

    pub(super) fn stop_current_playback(&mut self) {
        if let Some(playback) = &self.playback {
            playback.stop();
        }
        self.is_playing = false;
    }

    pub(super) fn play_adjacent_track(&mut self, delta: isize) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let position = indices
            .iter()
            .position(|ix| *ix == self.playing_track)
            .unwrap_or(0);
        let next = (position as isize + delta).clamp(0, indices.len().saturating_sub(1) as isize);
        self.play_track(indices[next as usize]);
    }

    pub(super) fn play_finished_track(&mut self) {
        match self.playback_mode {
            PlaybackMode::Loop => self.play_track(self.playing_track),
            PlaybackMode::Shuffle => self.play_random_track(),
            PlaybackMode::Straight => {
                if let Some(next) = self.next_track_after(self.playing_track) {
                    self.play_track(next);
                } else {
                    self.is_playing = false;
                    self.playback_status = "Playback finished".to_string();
                }
            }
        }
    }

    pub(super) fn next_track_after(&self, track_ix: usize) -> Option<usize> {
        let indices = self.current_track_indices();
        let position = indices.iter().position(|ix| *ix == track_ix)?;
        indices.get(position + 1).copied()
    }

    pub(super) fn play_random_track(&mut self) {
        let indices = self.current_track_indices();
        if indices.is_empty() {
            return;
        }

        let seed = Self::shuffle_seed();
        let next = indices
            .iter()
            .copied()
            .filter(|track_ix| indices.len() == 1 || *track_ix != self.playing_track)
            .min_by_key(|track_ix| Self::shuffle_key(&self.tracks[*track_ix], *track_ix, seed))
            .unwrap_or(self.playing_track);
        self.play_track(next);
    }

    pub(super) fn cycle_playback_mode(&mut self) {
        self.playback_mode = match self.playback_mode {
            PlaybackMode::Straight => PlaybackMode::Loop,
            PlaybackMode::Loop => PlaybackMode::Shuffle,
            PlaybackMode::Shuffle => PlaybackMode::Straight,
        };
        self.playback_status = format!("{} mode", self.playback_mode_label());
    }

    pub(super) fn playback_mode_label(&self) -> &'static str {
        match self.playback_mode {
            PlaybackMode::Straight => "Straight play",
            PlaybackMode::Loop => "Loop",
            PlaybackMode::Shuffle => "Shuffle",
        }
    }

    pub(super) fn playback_position(&self) -> Duration {
        self.playback
            .as_ref()
            .filter(|playback| !playback.is_empty())
            .map(PlaybackController::position)
            .unwrap_or_default()
    }

    pub(super) fn seek_from_waveform_click(&mut self, click_x: f32, viewport_width: f32) {
        let Some(track) = self.tracks.get(self.playing_track) else {
            return;
        };

        let waveform_left = PLAYER_BAR_PAD + PLAYER_ART_W + PLAYER_GAP + PLAYER_INFO_W + PLAYER_GAP;
        let waveform_right = viewport_width - (PLAYER_GAP + PLAYER_CONTROLS_W + PLAYER_BAR_PAD);
        let waveform_width = (waveform_right - waveform_left).max(1.0);
        let ratio = ((click_x - waveform_left) / waveform_width).clamp(0.0, 1.0);
        let target = track.duration_value.mul_f32(ratio);

        self.seek_playback(target);
    }

    pub(super) fn seek_playback(&mut self, position: Duration) {
        if self
            .playback
            .as_ref()
            .is_some_and(|playback| playback.is_empty())
        {
            self.play_track(self.playing_track);
        }

        match &self.playback {
            Some(playback) => match playback.seek(position) {
                Ok(()) => {
                    self.playback_status = format!("Seeked to {}", format_duration(position));
                }
                Err(error) => {
                    self.playback_status = format!("Seek failed: {error:#}");
                }
            },
            None => {
                self.playback_status = "Playback unavailable".to_string();
            }
        }
    }

    pub(super) fn cached_waveform(
        &mut self,
        track_ix: usize,
        cx: &mut Context<Self>,
    ) -> (Vec<f32>, bool) {
        if self.waveform_cache.len() < self.tracks.len() {
            self.waveform_cache.resize_with(self.tracks.len(), || None);
        }
        if self.waveform_loading.len() < self.tracks.len() {
            self.waveform_loading.resize(self.tracks.len(), false);
        }

        if let Some(waveform) = self.waveform_cache[track_ix].as_ref() {
            return (waveform.clone(), self.waveform_loading[track_ix]);
        }

        let source = WaveformSource::from_track(&self.tracks[track_ix]);

        if !self.waveform_loading[track_ix] {
            self.waveform_loading[track_ix] = true;
            let expected_path = source.path.clone();
            cx.spawn(async move |this, cx| {
                let waveform = cx
                    .background_executor()
                    .spawn(async move { TempoApp::generate_audio_waveform(&source) })
                    .await;

                let _ = this.update(cx, |app, cx| {
                    if app
                        .tracks
                        .get(track_ix)
                        .is_some_and(|track| track.path == expected_path)
                    {
                        app.waveform_cache[track_ix] = Some(waveform);
                        if track_ix < app.waveform_loading.len() {
                            app.waveform_loading[track_ix] = false;
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }

        (
            Self::generate_loading_waveform(Self::waveform_loading_phase()),
            true,
        )
    }

    pub(super) fn generate_audio_waveform(track: &WaveformSource) -> Vec<f32> {
        Self::decode_waveform(track).unwrap_or_else(|| Self::generate_fallback_waveform(track))
    }

    pub(super) fn decode_waveform(track: &WaveformSource) -> Option<Vec<f32>> {
        let file = fs::File::open(&track.path).ok()?;
        let mut decoder = Decoder::try_from(file).ok()?;
        let duration = decoder.total_duration().unwrap_or(track.duration_value);
        let sample_rate = decoder.sample_rate().get() as f64;
        let channels = decoder.channels().get() as f64;
        let total_samples = (duration.as_secs_f64() * sample_rate * channels)
            .ceil()
            .max(1.0) as usize;

        if total_samples == 0 {
            return None;
        }

        let mut peaks = vec![0.0_f32; WAVEFORM_SEGMENTS];
        let mut saw_sample = false;
        let samples_per_bin = (total_samples / WAVEFORM_SEGMENTS).max(1);
        let mut bin = 0;
        let mut next_bin_sample = samples_per_bin;

        for (sample_ix, sample) in decoder.by_ref().enumerate() {
            while sample_ix >= next_bin_sample && bin < WAVEFORM_SEGMENTS - 1 {
                bin += 1;
                next_bin_sample = next_bin_sample.saturating_add(samples_per_bin);
            }

            peaks[bin] = peaks[bin].max(sample.abs());
            saw_sample = true;
        }

        if !saw_sample {
            return None;
        }

        let max_peak = peaks.iter().copied().fold(0.0_f32, f32::max).max(0.001);
        Some(
            peaks
                .into_iter()
                .map(|peak| 8.0 + (peak / max_peak).sqrt() * 50.0)
                .collect(),
        )
    }

    pub(super) fn generate_fallback_waveform(track: &WaveformSource) -> Vec<f32> {
        let mut seed = 0xcbf29ce484222325_u64;

        for part in [&track.title, &track.artist, &track.album, &track.duration] {
            for byte in part.bytes() {
                seed ^= byte as u64;
                seed = seed.wrapping_mul(0x100000001b3);
            }
        }

        let pulse_count = 3.0 + (track.title.len() % 5) as f32;
        let mut previous = 0.38;

        (0..WAVEFORM_SEGMENTS)
            .map(|ix| {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);

                let noise = ((seed >> 33) as f32) / ((1_u64 << 31) as f32);
                let position = ix as f32 / WAVEFORM_SEGMENTS as f32;
                let pulse = (position * std::f32::consts::TAU * pulse_count).sin().abs();
                let target = (0.16 + noise * 0.5 + pulse * 0.34).min(1.0);

                previous = previous * 0.66 + target * 0.34;
                8.0 + previous * 50.0
            })
            .collect()
    }

    pub(super) fn generate_loading_waveform(phase: f32) -> Vec<f32> {
        (0..WAVEFORM_SEGMENTS)
            .map(|ix| {
                let position = ix as f32 / 12.0;
                let sweep = ((position - phase).sin() + 1.0) * 0.5;
                let ripple = ((position * 0.35 + phase * 0.6).sin() + 1.0) * 0.5;
                (10.0 + (sweep * 0.7 + ripple * 0.3) * 42.0).round()
            })
            .collect()
    }

    pub(super) fn waveform_loading_phase() -> f32 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as f32 / 90.0)
            .unwrap_or_default()
    }

    pub(super) fn render_player_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = *self.colors();

        if self.tracks.is_empty() {
            return div()
                .h(px(86.0))
                .flex_none()
                .flex()
                .items_center()
                .gap_4()
                .px_4()
                .border_t_1()
                .border_color(rgb(colors.button_hover))
                .bg(rgb(colors.player))
                .child(
                    div()
                        .w(px(54.0))
                        .h(px(54.0))
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(colors.border_strong))
                        .bg(rgb(colors.playing))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(colors.text_faint))
                        .child("♪"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(rgb(colors.text_strong))
                                .child(if self.is_scanning {
                                    "Scanning library"
                                } else {
                                    "Library scanner idle"
                                }),
                        )
                        .child(
                            div()
                                .text_color(rgb(colors.text_muted))
                                .child(self.visible_scan_status()),
                        ),
                )
                .into_any_element();
        }

        self.playing_track = self.playing_track.min(self.tracks.len() - 1);
        let (waveform, waveform_loading) = self.cached_waveform(self.playing_track, cx);
        if waveform_loading {
            window.request_animation_frame();
        }
        let playback_position = self.playback_position();
        let playing_track_ix = self.playing_track;
        let track = &self.tracks[self.playing_track];
        let playback_position = playback_position.min(track.duration_value);
        let playback_progress = if track.duration_value.is_zero() {
            0.0
        } else {
            (playback_position.as_secs_f32() / track.duration_value.as_secs_f32()).clamp(0.0, 1.0)
        };
        let now_playing_active_color = colors.accent_soft;

        div()
            .h(px(86.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .border_t_1()
            .border_color(rgb(colors.button_hover))
            .bg(rgb(colors.player))
            .child(
                div()
                    .id("now-playing-album-link")
                    .cursor_pointer()
                    .child(self.album_tile_with_hover_border(
                        track,
                        54.0,
                        Some(now_playing_active_color),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_album_tab_for_track(playing_track_ix);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(220.0))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id("now-playing-title-link")
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(colors.text_strong))
                            .cursor_pointer()
                            .hover(move |this| this.text_color(rgb(now_playing_active_color)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_track_in_all_music(playing_track_ix);
                                cx.notify();
                            }))
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_color(rgb(colors.text_muted))
                            .child(
                                div()
                                    .id("now-playing-artist-link")
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .cursor_pointer()
                                    .hover(move |this| {
                                        this.text_color(rgb(now_playing_active_color))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_artist_tab_for_track(playing_track_ix);
                                        cx.notify();
                                    }))
                                    .child(track.artist.clone()),
                            )
                            .child(div().text_color(rgb(colors.text_faint)).child("-"))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(track.album.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(colors.text_faint))
                            .child(format!(
                                "{}  ·  {}  ·  {}  ·  {}",
                                track.codec,
                                Self::bitrate_label(track),
                                track.year,
                                self.playback_status.clone()
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .relative()
                    .child(self.waveform_seekbar(
                        format_duration(playback_position),
                        track.duration.clone(),
                        playback_progress,
                        waveform,
                        waveform_loading,
                        cx,
                    )),
            )
            .child(
                div()
                    .w(px(170.0))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_color(rgb(colors.text_muted))
                    .child(self.transport_overlay(self.is_playing, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child("☰")
                            .child("♩")
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(3.0))
                                    .rounded_full()
                                    .bg(rgb(colors.text_faint))
                                    .child(
                                        div()
                                            .w(px(104.0))
                                            .h(px(3.0))
                                            .rounded_full()
                                            .bg(rgb(colors.text)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn playback_mode_icon(&self) -> &'static str {
        match self.playback_mode {
            PlaybackMode::Straight => "→",
            PlaybackMode::Loop => "↻",
            PlaybackMode::Shuffle => "⤨",
        }
    }

    pub(super) fn bitrate_label(track: &Track) -> String {
        track
            .bitrate
            .map(|bitrate| format!("{bitrate} kbps"))
            .unwrap_or_else(|| "unknown bitrate".to_string())
    }

    pub(super) fn waveform_seekbar(
        &self,
        elapsed: String,
        duration: String,
        progress: f32,
        waveform: Vec<f32>,
        loading: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let progress_segments = (waveform.len() as f32 * progress).round() as usize;
        let colors = *self.colors();

        div()
            .id("waveform-seekbar")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .cursor_pointer()
            .rounded_lg()
            .overflow_hidden()
            .bg(rgb(colors.waveform_bg))
            .border_1()
            .border_color(rgb(colors.waveform_border))
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                if event.standard_click() {
                    let click_x = f32::from(event.position().x);
                    let viewport_width = f32::from(window.viewport_size().width);
                    this.seek_from_waveform_click(click_x, viewport_width);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .absolute()
                    .top(px(42.0))
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(rgb(colors.waveform_line)),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .children(waveform.into_iter().enumerate().map(move |(ix, height)| {
                        Self::waveform_bar(ix, height, progress_segments, loading, colors)
                    })),
            )
            .when(loading, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_2()
                        .left_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(colors.waveform_bg))
                        .text_xs()
                        .text_color(rgb(colors.waveform_played_peak))
                        .child("Loading waveform"),
                )
            })
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .left_3()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(colors.waveform_bg))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(elapsed),
            )
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .right_3()
                    .px_1()
                    .rounded_sm()
                    .bg(rgb(colors.waveform_bg))
                    .text_xs()
                    .text_color(rgb(colors.text_faint))
                    .child(duration),
            )
    }

    pub(super) fn waveform_bar(
        ix: usize,
        height: f32,
        progress_segments: usize,
        loading: bool,
        colors: ThemeColors,
    ) -> impl IntoElement {
        let played = ix < progress_segments;
        let playhead = ix == progress_segments;
        let peak = height > 44.0;
        let color = if loading && peak {
            colors.waveform_played
        } else if loading {
            colors.waveform_idle_peak
        } else if playhead {
            colors.waveform_playhead
        } else if played && peak {
            colors.waveform_played_peak
        } else if played {
            colors.waveform_played
        } else if peak {
            colors.waveform_idle_peak
        } else {
            colors.waveform_idle
        };

        div()
            .flex_1()
            .min_w(px(1.0))
            .h(px(if playhead { 58.0 } else { height }))
            .rounded_full()
            .bg(rgb(color))
            .opacity(if loading || played || playhead {
                1.0
            } else {
                0.78
            })
    }

    pub(super) fn transport_overlay(
        &self,
        is_playing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *self.colors();

        div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_full()
            .bg(rgb(colors.app))
            .border_1()
            .border_color(rgb(colors.waveform_border))
            .child(
                self.transport_button(
                    self.playback_mode_icon(),
                    false,
                    self.playback_mode != PlaybackMode::Straight,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.cycle_playback_mode();
                    cx.notify();
                })),
            )
            .child(
                self.transport_button("◀", false, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.play_adjacent_track(-1);
                        cx.notify();
                    })),
            )
            .child(
                self.transport_button(if is_playing { "Ⅱ" } else { "▶" }, true, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_playback();
                        cx.notify();
                    })),
            )
            .child(
                self.transport_button("▶", false, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.play_adjacent_track(1);
                        cx.notify();
                    })),
            )
            .child(
                self.transport_button("↻", false, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.play_random_track();
                        cx.notify();
                    })),
            )
    }

    pub(super) fn transport_button(
        &self,
        label: &'static str,
        primary: bool,
        active: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let size = if primary { 28.0 } else { 22.0 };
        let hover_size = if primary { 32.0 } else { 26.0 };
        let colors = *self.colors();
        let bg = if primary {
            colors.transport_primary_bg
        } else if active {
            colors.text_strong
        } else {
            colors.player
        };
        let fg = if primary {
            colors.transport_primary_fg
        } else if active {
            colors.app
        } else {
            colors.text_muted
        };

        div()
            .id(SharedString::from(format!("transport-{label}-{primary}")))
            .w(px(size))
            .h(px(size))
            .rounded_full()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(gpui::FontWeight::BOLD)
            .hover(move |this| {
                this.w(px(hover_size))
                    .h(px(hover_size))
                    .bg(rgb(colors.text_strong))
                    .text_color(rgb(colors.app))
            })
            .child(label)
    }
}
