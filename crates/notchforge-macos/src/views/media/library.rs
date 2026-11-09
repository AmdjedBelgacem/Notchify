//! The library half of the music page: the playlists of the chosen player
//! and YouTube's up-next list.

use super::*;

impl MediaPage {
    /// Which player's playlists to list.
    fn list_player(&self, media: &MediaState) -> MediaPlayer {
        if let Some(p) = self.state.borrow().chosen_list_player {
            return p;
        }
        if let Some(p) = media.primary() {
            return p.player;
        }
        MediaPlayer::ALL
            .into_iter()
            .find(|p| media.playlists_for(*p).next().is_some())
            .or_else(|| media.running.first().copied())
            .unwrap_or(if glue::is_installed(MediaPlayer::Spotify) { MediaPlayer::Spotify } else { MediaPlayer::Music })
    }

    pub(super) fn render_playlists(&self, media: &MediaState) {
        let (searching, show_lyrics) = {
            let st = self.state.borrow();
            (st.searching, st.show_lyrics)
        };
        let player = self.list_player(media);
        seg_style(&self.seg_spotify, "Spotify", !searching && !show_lyrics && player == MediaPlayer::Spotify);
        seg_style(&self.seg_music, "Music", !searching && !show_lyrics && player == MediaPlayer::Music);
        seg_style(&self.seg_up_next, "Up next", !searching && !show_lyrics && player == MediaPlayer::Browser);
        seg_style(&self.seg_lyrics, "Lyrics", show_lyrics && !searching);
        self.seg_up_next.setEnabled(!media.up_next.is_empty());

        if searching {
            self.render_search();
            return;
        }
        self.search_field.setHidden(true);
        self.list_scroll.setFrame(rect(6.0, 48.0, self.list_width, self.list_height()));
        if show_lyrics {
            self.render_lyrics(media);
            return;
        }
        // The Up next list replaces the playlist table while a YouTube video
        // is the featured track.
        if player == MediaPlayer::Browser {
            self.render_up_next(media);
            return;
        }

        let status = media.playlist_status(player);
        let lists: Vec<&Playlist> = media.playlists_for(player).collect();
        let running = media.running.contains(&player);
        let denied = media.automation_denied.contains(&player);
        let playing = media.player(player).is_some_and(NowPlaying::is_playing);
        let active = self.state.borrow().active_playlist.clone().filter(|_| playing);
        let queue: &[notchforge_core::model::QueueTrack] =
            if player == MediaPlayer::Spotify { &media.spotify_queue } else { &[] };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        player.hash(&mut h);
        format!("{status:?}{running}{denied}{active:?}").hash(&mut h);
        for l in &lists {
            l.id.hash(&mut h);
            l.name.hash(&mut h);
            l.track_count.hash(&mut h);
            l.artwork_path.hash(&mut h);
        }
        for t in queue {
            t.title.hash(&mut h);
            t.artist.hash(&mut h);
        }
        let key = h.finish();
        if self.state.borrow().playlists_key == key {
            return;
        }
        self.state.borrow_mut().playlists_key = key;
        self.row_targets.clear();
        remove_all_subviews(&self.list_doc);

        let (message, action, action_title): (Option<String>, ListAction, &str) =
            if denied || status == PlaylistStatus::NeedsAutomation {
                (
                    Some(format!("Allow Notchisland to control {} to see your playlists.", player.display_name())),
                    ListAction::AllowAutomation,
                    "Allow…",
                )
            } else if !lists.is_empty() {
                (None, ListAction::None, "")
            } else {
                match (player, &status) {
                    (MediaPlayer::Spotify, PlaylistStatus::SigningIn) => (
                        Some("Waiting for you to approve Notchisland in your browser…".into()),
                        ListAction::CancelSpotify,
                        "Cancel",
                    ),
                    (MediaPlayer::Spotify, PlaylistStatus::NeedsSpotifyLogin) => (
                        Some("Sign in to Spotify to list your playlists.".into()),
                        ListAction::ConnectSpotify,
                        "Connect Spotify",
                    ),
                    (MediaPlayer::Music, _) if !running => {
                        (Some("Open Music to see your playlists.".into()), ListAction::OpenMusic, "Open Music")
                    }
                    (_, PlaylistStatus::Failed(e)) => {
                        (Some(format!("Couldn't load playlists. {e}")), ListAction::Retry, "Retry")
                    }
                    (_, PlaylistStatus::Loaded) => (Some("No playlists yet.".into()), ListAction::None, ""),
                    _ => (Some("Loading playlists…".into()), ListAction::None, ""),
                }
            };
        self.list_action_kind.set(action);
        match message {
            Some(m) => {
                set_text(&self.list_message, &m);
                self.list_message.setHidden(false);
            }
            None => self.list_message.setHidden(true),
        }
        style_pill(
            &self.list_action,
            action_title,
            None,
            &font(12.0, Weight::Semibold),
            &self.p.primary,
            &self.p.accent,
            14.0,
        );
        self.list_action.setHidden(action == ListAction::None);
        self.list_scroll.setHidden(lists.is_empty() && queue.is_empty());

        let p = &self.p;
        let w = self.list_width;
        let brand = rgb8(player.color());
        let visible_h = self.list_scroll.frame().size.height;
        // Five rows fill the tile; a shorter list spreads out, a longer one scrolls.
        let rows = lists.len() + queue.len();
        let header_h = if queue.is_empty() { 0.0 } else { 20.0 };
        let row_h = if rows == 0 || rows as f64 >= VISIBLE_ROWS {
            visible_h / VISIBLE_ROWS
        } else {
            (visible_h / rows as f64).min(MAX_ROW_H)
        };
        let doc_h = (rows as f64 * row_h + header_h).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        let detail_color = rgba(1.0, 1.0, 1.0, 0.74);
        // Spotify's play queue sits above the playlists, under an "Up next" heading.
        let mut base = 0.0;
        if !queue.is_empty() {
            let head = label(
                self.mtm,
                "Up next",
                &font(10.5, Weight::Semibold),
                &p.secondary,
                rect(10.0, base, w - 20.0, header_h),
            );
            self.list_doc.addSubview(&head);
            base += header_h;
            for (i, t) in queue.iter().enumerate() {
                let row =
                    shape_view(self.mtm, rect(0.0, base + i as f64 * row_h, w, row_h), &rgba(1.0, 1.0, 1.0, 0.0), 13.0);
                let cy = (row_h - 40.0) / 2.0;
                let cover = shape_view(self.mtm, rect(8.0, cy, 40.0, 40.0), &brand, 8.0);
                cover.addSubview(&symbol_view(
                    self.mtm,
                    "music.quarternote.3",
                    16.0,
                    &rgba(1.0, 1.0, 1.0, 0.92),
                    rect(10.0, 10.0, 20.0, 20.0),
                    &t.title,
                ));
                row.addSubview(&cover);
                let ty = (row_h - 33.0) / 2.0;
                row.addSubview(&label(
                    self.mtm,
                    &t.title,
                    &font(13.5, Weight::Medium),
                    &p.primary,
                    rect(58.0, ty, w - 66.0, 18.0),
                ));
                row.addSubview(&label(
                    self.mtm,
                    &t.artist,
                    &font(12.0, Weight::Regular),
                    &detail_color,
                    rect(58.0, ty + 18.0, w - 66.0, 15.0),
                ));
                set_accessibility_label(&row, &format!("Queued: {}, {}", t.title, t.artist));
                self.list_doc.addSubview(&row);
            }
            base += queue.len() as f64 * row_h;
        }
        for (i, pl) in lists.iter().enumerate() {
            let is_active = active.as_deref() == Some(pl.id.as_str());
            let row_bg = if is_active { rgba(1.0, 1.0, 1.0, 0.14) } else { rgba(1.0, 1.0, 1.0, 0.0) };
            let row = shape_view(self.mtm, rect(0.0, base + i as f64 * row_h, w, row_h), &row_bg, 13.0);
            let cy = (row_h - 40.0) / 2.0;
            let cover = shape_view(self.mtm, rect(8.0, cy, 40.0, 40.0), &brand, 8.0);
            if let Some(l) = cover.layer() {
                l.setMasksToBounds(true);
            }
            match pl.artwork_path.as_deref().and_then(icons::file_image) {
                Some(img) => {
                    let v = NSImageView::new(self.mtm);
                    v.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                    v.setFrame(rect(0.0, 0.0, 40.0, 40.0));
                    v.setImage(Some(&img));
                    cover.addSubview(&v);
                }
                None => cover.addSubview(&symbol_view(
                    self.mtm,
                    "music.note.list",
                    16.0,
                    &rgba(1.0, 1.0, 1.0, 0.92),
                    rect(10.0, 10.0, 20.0, 20.0),
                    &pl.name,
                )),
            }
            row.addSubview(&cover);
            let ty = (row_h - 33.0) / 2.0;
            row.addSubview(&label(
                self.mtm,
                &pl.name,
                &font(13.5, Weight::Medium),
                &p.primary,
                rect(58.0, ty, w - 88.0, 18.0),
            ));
            let mut detail = Vec::new();
            if let Some(c) = pl.track_count {
                detail.push(if c == 1 { "1 song".to_owned() } else { format!("{c} songs") });
            }
            if let Some(o) = &pl.owner {
                detail.push(o.clone());
            }
            row.addSubview(&label(
                self.mtm,
                &detail.join(" · "),
                &font(12.0, Weight::Regular),
                &detail_color,
                rect(58.0, ty + 18.0, w - 88.0, 15.0),
            ));
            if is_active {
                row.addSubview(&equalizer_view(self.mtm, rect(w - 26.0, (row_h - 12.0) / 2.0, 12.0, 12.0), &brand));
            }
            let (id, name) = (pl.id.clone(), pl.name.clone());
            let st = self.state.clone();
            let t = self.row_targets.add(self.mtm, move |_| {
                st.borrow_mut().active_playlist = Some(id.clone());
                glue::control(player, MediaAction::PlayPlaylist(id.clone()));
            });
            row.addSubview(&click_overlay(self.mtm, &t, rect(0.0, 0.0, w, row_h), &format!("Play playlist {name}")));
            set_accessibility_label(&row, &format!("{}, {}", pl.name, detail.join(", ")));
            self.list_doc.addSubview(&row);
        }
        self.list_doc.scrollPoint(NSPoint::new(0.0, 0.0));
    }

    /// YouTube's up-next list, in the playlists tile: click a row to open the
    /// video.
    fn render_up_next(&self, media: &MediaState) {
        let mtm = self.mtm;
        let p = &self.p;
        let videos = &media.up_next;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in videos {
            v.url.hash(&mut h);
            v.title.hash(&mut h);
        }
        let key = h.finish();
        if self.state.borrow().playlists_key == key {
            return;
        }
        self.state.borrow_mut().playlists_key = key;
        self.row_targets.clear();
        remove_all_subviews(&self.list_doc);
        self.list_message.setHidden(true);
        self.list_action.setHidden(true);
        self.list_scroll.setHidden(videos.is_empty());
        if videos.is_empty() {
            set_text(&self.list_message, "No recommendations found.");
            self.list_message.setHidden(false);
            return;
        }
        let w = self.list_width;
        let visible_h = self.list_scroll.frame().size.height;
        let row_h = if videos.len() as f64 >= VISIBLE_ROWS {
            visible_h / VISIBLE_ROWS
        } else {
            (visible_h / videos.len() as f64).min(MAX_ROW_H)
        };
        let doc_h = (videos.len() as f64 * row_h).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        for (i, v) in videos.iter().enumerate() {
            let row = shape_view(self.mtm, rect(0.0, i as f64 * row_h, w, row_h), &rgba(1.0, 1.0, 1.0, 0.0), 13.0);
            let cy = (row_h - 40.0) / 2.0;
            let cover = shape_view(mtm, rect(8.0, cy, 40.0, 40.0), &rgb8(MediaPlayer::Browser.color()), 8.0);
            cover.addSubview(&symbol_view(
                mtm,
                "play.rectangle.fill",
                16.0,
                &rgba(1.0, 1.0, 1.0, 0.92),
                rect(10.0, 10.0, 20.0, 20.0),
                &v.title,
            ));
            row.addSubview(&cover);
            let ty = (row_h - 33.0) / 2.0;
            row.addSubview(&label(
                mtm,
                &v.title,
                &font(13.5, Weight::Medium),
                &p.primary,
                rect(58.0, ty, w - 66.0, 18.0),
            ));
            let detail = if v.duration.is_empty() {
                v.author.clone()
            } else if v.author.is_empty() {
                v.duration.clone()
            } else {
                format!("{} · {}", v.author, v.duration)
            };
            row.addSubview(&label(
                mtm,
                &detail,
                &font(12.0, Weight::Regular),
                &rgba(1.0, 1.0, 1.0, 0.74),
                rect(58.0, ty + 18.0, w - 66.0, 15.0),
            ));
            let url = v.url.clone();
            let t = self.row_targets.add(mtm, move |_| {
                // Play it where the music already is: the playing browser's
                // tab navigates to the video.
                glue::control(MediaPlayer::Browser, MediaAction::Navigate(url.clone()));
            });
            row.addSubview(&click_overlay(mtm, &t, rect(0.0, 0.0, w, row_h), &format!("Play {}", v.title)));
            set_accessibility_label(&row, &format!("{}, {}", v.title, v.author));
            self.list_doc.addSubview(&row);
        }
        self.list_doc.scrollPoint(NSPoint::new(0.0, 0.0));
    }

    /// The lyrics of the featured track; the tick keeps the playing line lit.
    fn render_lyrics(&self, media: &MediaState) {
        let mtm = self.mtm;
        let lyrics = media.lyrics.clone();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(l) = &lyrics {
            for (ms, line) in &l.lines {
                ms.hash(&mut h);
                line.hash(&mut h);
            }
            l.plain.hash(&mut h);
        }
        let key = h.finish();
        {
            let st = self.state.borrow();
            if st.lyrics_key == key {
                return;
            }
        }
        {
            let mut st = self.state.borrow_mut();
            st.lyrics_key = key;
            st.lyric_rows.clear();
            st.lyric_labels.clear();
            st.lyric_active = -1;
            st.lyrics = lyrics.clone();
        }
        self.row_targets.clear();
        remove_all_subviews(&self.list_doc);
        self.list_message.setHidden(true);
        self.list_action.setHidden(true);
        let lines: &[(i64, String)] = lyrics.as_ref().map(|l| l.lines.as_slice()).unwrap_or(&[]);
        if lines.is_empty() {
            self.list_scroll.setHidden(true);
            set_text(&self.list_message, "No lyrics found for this track.");
            self.list_message.setHidden(false);
            return;
        }
        self.list_scroll.setHidden(false);
        let w = self.list_width;
        let visible_h = self.list_height();
        let row_h = 30.0_f64.min((visible_h / lines.len() as f64).max(20.0));
        let doc_h = (lines.len() as f64 * row_h).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        let dim = rgba(1.0, 1.0, 1.0, 0.45);
        let plain = lyrics.as_ref().is_some_and(|l| l.plain);
        for (i, (ms, line)) in lines.iter().enumerate() {
            let is_gap = line.trim().is_empty() || line.trim().chars().all(|c| matches!(c, '♪' | '♫'));
            let text = if is_gap { "· · ·".to_owned() } else { line.clone() };
            let lbl = label(
                mtm,
                &text,
                &font(if plain { 12.5 } else { 13.0 }, Weight::Medium),
                &dim,
                rect(16.0, i as f64 * row_h, w - 32.0, row_h),
            );
            lbl.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
            lbl.setMaximumNumberOfLines(2);
            self.list_doc.addSubview(&lbl);
            let mut st = self.state.borrow_mut();
            st.lyric_rows.push((*ms, i));
            st.lyric_labels.push(lbl.clone());
        }
        self.list_doc.scrollPoint(NSPoint::new(0.0, 0.0));
    }

    /// Light the lyric line that is playing right now (runs at 1 Hz).
    pub(super) fn tick_lyrics(&self) {
        let (lyrics, rows, active) = {
            let st = self.state.borrow();
            (st.lyrics.clone(), st.lyric_rows.clone(), st.lyric_active)
        };
        let Some(lyrics) = lyrics else { return };
        let Some(n) = self.state.borrow().now.clone() else { return };
        let position_ms = (n.position_at(now_ms()).unwrap_or(0.0) * 1000.0).round() as i64;
        let want = if lyrics.plain { -1 } else { lyrics.line_at(position_ms).map(|i| i as isize).unwrap_or(-1) };
        if want == active {
            return;
        }
        self.state.borrow_mut().lyric_active = want;
        let lit = rgba(1.0, 1.0, 1.0, 1.0);
        let dim = rgba(1.0, 1.0, 1.0, 0.45);
        let row_h = 30.0_f64.min((self.list_height() / rows.len().max(1) as f64).max(20.0));
        for (i, (_at, row_index)) in rows.iter().enumerate() {
            let Some(lbl) = self.state.borrow().lyric_labels.get(*row_index).cloned() else { continue };
            let on = i as isize == want;
            lbl.setTextColor(Some(if on { &lit } else { &dim }));
            if on {
                let y = i as f64 * row_h;
                self.list_doc.scrollPoint(NSPoint::new(0.0, (y - row_h * 2.0).max(0.0)));
            }
        }
    }

    /// The search overlay: a field above the list, results as rows.
    fn render_search(&self) {
        let mtm = self.mtm;
        self.list_message.setHidden(true);
        self.list_action.setHidden(true);
        self.search_field.setHidden(false);
        // The list slides under the field.
        let lh = self.list_height() - 30.0;
        self.list_scroll.setFrame(rect(6.0, 76.0, self.list_width, lh));
        let results = self.state.borrow().search_results.clone();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in &results {
            v.url.hash(&mut h);
        }
        let key = h.finish();
        if self.state.borrow().playlists_key == key {
            return;
        }
        self.state.borrow_mut().playlists_key = key;
        self.row_targets.clear();
        remove_all_subviews(&self.list_doc);
        if results.is_empty() {
            self.list_scroll.setHidden(true);
            set_text(&self.list_message, "Type and press Enter to search YouTube.");
            self.list_message.setHidden(false);
            return;
        }
        self.list_scroll.setHidden(false);
        let w = self.list_width;
        let visible_h = lh;
        let row_h = if results.len() as f64 >= VISIBLE_ROWS {
            visible_h / VISIBLE_ROWS
        } else {
            (visible_h / results.len() as f64).min(MAX_ROW_H)
        };
        let doc_h = (results.len() as f64 * row_h).max(visible_h);
        self.list_doc.setFrame(rect(0.0, 0.0, w, doc_h));
        for (i, v) in results.iter().enumerate() {
            let row = shape_view(self.mtm, rect(0.0, i as f64 * row_h, w, row_h), &rgba(1.0, 1.0, 1.0, 0.0), 13.0);
            let cy = (row_h - 40.0) / 2.0;
            let cover = shape_view(mtm, rect(8.0, cy, 40.0, 40.0), &rgb8(MediaPlayer::Browser.color()), 8.0);
            cover.addSubview(&symbol_view(
                mtm,
                "play.fill",
                15.0,
                &rgba(1.0, 1.0, 1.0, 0.92),
                rect(11.0, 11.0, 18.0, 18.0),
                &v.title,
            ));
            row.addSubview(&cover);
            let ty = (row_h - 33.0) / 2.0;
            row.addSubview(&label(
                mtm,
                &v.title,
                &font(13.0, Weight::Medium),
                &self.p.primary,
                rect(58.0, ty, w - 66.0, 18.0),
            ));
            row.addSubview(&label(
                mtm,
                &v.author,
                &font(11.5, Weight::Regular),
                &rgba(1.0, 1.0, 1.0, 0.74),
                rect(58.0, ty + 18.0, w - 66.0, 15.0),
            ));
            let url = v.url.clone();
            let t = self.row_targets.add(mtm, move |_| {
                glue::control(MediaPlayer::Browser, MediaAction::Navigate(url.clone()));
                crate::app::with_controller(|c| c.media_search_toggled(false));
            });
            row.addSubview(&click_overlay(mtm, &t, rect(0.0, 0.0, w, row_h), &format!("Play {}", v.title)));
            set_accessibility_label(&row, &format!("Play {}, {}", v.title, v.author));
            self.list_doc.addSubview(&row);
        }
        self.list_doc.scrollPoint(NSPoint::new(0.0, 0.0));
    }

    /// Close the search overlay and show the library again.
    pub(super) fn close_search(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.searching = false;
            st.search_results.clear();
            st.playlists_key = 0;
        }
        self.search_field.setHidden(true);
        self.list_scroll.setFrame(rect(6.0, 48.0, self.list_width, self.list_height()));
        crate::app::with_controller(|c| c.media_rerender());
    }
}
