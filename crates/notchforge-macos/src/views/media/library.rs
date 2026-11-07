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
        let player = self.list_player(media);
        seg_style(&self.seg_spotify, "Spotify", player == MediaPlayer::Spotify);
        seg_style(&self.seg_music, "Music", player == MediaPlayer::Music);
        seg_style(&self.seg_up_next, "Up next", player == MediaPlayer::Browser);
        self.seg_up_next.setEnabled(!media.up_next.is_empty());

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
}
