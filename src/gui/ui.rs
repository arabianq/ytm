use super::{
    Application, ApplicationAuth, ApplicationLibrary, AsyncView, HomeItem, LibrarySnapshot,
    LibraryTab, PlaylistSnapshot,
    data::{
        clone_async_state, load_artist_profile, load_library_snapshot, load_playlist_snapshot,
        load_user_profile, playlist_item_summary, playlist_item_thumbnails,
    },
    fonts,
};
use anyhow::{Result, anyhow};
use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{
    Align2, Area, Button, CentralPanel, Color32, Context, Frame, Id, RichText, ScrollArea, Stroke,
    TextEdit, Ui, Vec2, ViewportBuilder, vec2,
};
use egui_async::{Bind, StateWithData};
use rust_i18n::t;
use std::{collections::HashMap, env};
use ytmapi_rs::common::YoutubeID;

fn blend_color(from: Color32, to: Color32, progress: f32) -> Color32 {
    let mix = |start: u8, end: u8| {
        (f32::from(start) + (f32::from(end) - f32::from(start)) * progress.clamp(0.0, 1.0)).round()
            as u8
    };
    Color32::from_rgba_premultiplied(
        mix(from.r(), to.r()),
        mix(from.g(), to.g()),
        mix(from.b(), to.b()),
        mix(from.a(), to.a()),
    )
}

impl Application {
    pub(super) fn new(ctx: &Context) -> Self {
        ctx.set_zoom_factor(1.5);
        fonts::configure_fonts(ctx);
        egui_extras::install_image_loaders(ctx);

        let client_id = env::var("CLIENT_ID").ok().map(|s| s.trim().to_string());
        let client_secret = env::var("CLIENT_SECRET").ok().map(|s| s.trim().to_string());
        let cookie = env::var("YTM_COOKIE")
            .ok()
            .or_else(|| env::var("YTM_COOKIES").ok())
            .map(|s| s.trim().to_string());
        let cookie_file = env::var("YTM_COOKIE_FILE")
            .ok()
            .or_else(|| env::var("YTM_COOKIES_FILE").ok())
            .map(|s| s.trim().to_string());

        Self {
            auth: ApplicationAuth {
                client_id,
                client_secret,
                cookie,
                cookie_file,
                client_id_input: String::new(),
                client_secret_input: String::new(),
                cookie_input: String::new(),
                cookie_file_input: String::new(),
                current_state: Bind::new(true),
                previous_state: None,
                yt_client: None,
            },
            library: ApplicationLibrary {
                current_state: Bind::new(true),
                playlist_state: Bind::new(true),
                artist_state: Bind::new(true),
                user_state: Bind::new(true),
                selected_tab: LibraryTab::Home,
                selected_playlist_id: None,
                selected_home_params: None,
                selected_artist_id: None,
                selected_user_id: None,
            },
            thumbnail_state: HashMap::new(),
        }
    }

    fn ensure_library_requested(&mut self) {
        let Some(yt) = self.auth.yt_client.clone() else {
            return;
        };

        if matches!(self.library.current_state.state(), StateWithData::Idle) {
            self.library.current_state.request(load_library_snapshot(
                yt,
                self.library.selected_home_params.clone(),
            ));
        }
    }

    fn ensure_playlist_requested(&mut self) {
        let Some(yt) = self.auth.yt_client.clone() else {
            return;
        };
        let Some(playlist_id) = self.library.selected_playlist_id.clone() else {
            return;
        };

        if matches!(self.library.playlist_state.state(), StateWithData::Idle) {
            self.library
                .playlist_state
                .request(load_playlist_snapshot(yt, playlist_id));
        }
    }

    fn select_playlist(&mut self, playlist_id: String) {
        if self.library.selected_playlist_id.as_deref() == Some(playlist_id.as_str()) {
            return;
        }

        self.library.selected_playlist_id = Some(playlist_id);
        self.library.playlist_state.clear();
    }

    fn select_artist(&mut self, artist_id: String) {
        if self.library.selected_artist_id.as_deref() != Some(artist_id.as_str()) {
            self.library.selected_artist_id = Some(artist_id);
            self.library.artist_state.clear();
        }
        self.library.selected_tab = LibraryTab::ArtistProfile;
    }

    fn ensure_artist_requested(&mut self) {
        let (Some(yt), Some(artist_id)) = (
            self.auth.yt_client.clone(),
            self.library.selected_artist_id.clone(),
        ) else {
            return;
        };
        if matches!(self.library.artist_state.state(), StateWithData::Idle) {
            self.library
                .artist_state
                .request(load_artist_profile(yt, artist_id));
        }
    }

    fn select_user(&mut self, user_id: String) {
        self.library.selected_user_id = Some(user_id);
        self.library.user_state.clear();
        self.library.selected_tab = LibraryTab::UserProfile;
    }
    fn ensure_user_requested(&mut self) {
        let (Some(yt), Some(user_id)) = (
            self.auth.yt_client.clone(),
            self.library.selected_user_id.clone(),
        ) else {
            return;
        };
        if matches!(self.library.user_state.state(), StateWithData::Idle) {
            self.library
                .user_state
                .request(load_user_profile(yt, user_id));
        }
    }

    fn reload_library(&mut self) {
        self.library.current_state.clear();
        self.library.playlist_state.clear();
        self.library.artist_state.clear();
        self.library.user_state.clear();
    }

    fn set_home_filter(&mut self, params: Option<String>) {
        self.library.selected_home_params = params;
        self.reload_library();
        self.library.selected_tab = LibraryTab::Home;
    }

    fn show_library(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        if self.library.selected_playlist_id.is_none() {
            if let Some(first_playlist) = snapshot.playlists.first() {
                self.select_playlist(first_playlist.playlist_id.get_raw().to_owned());
            } else if let Some(first_playlist) = snapshot
                .recommended_playlists
                .iter()
                .flat_map(|category| category.playlists.iter())
                .next()
            {
                self.select_playlist(first_playlist.playlist_id.get_raw().to_owned());
            }
        }

        ui.horizontal(|ui| {
            ui.heading("YouTube Music");
            if ui.button("Reload").clicked() {
                self.reload_library();
            }
        });

        ui.label(format!(
            "Loaded {} playlists, {} songs, {} albums and {} artists.",
            snapshot.playlists.len(),
            snapshot.songs.len(),
            snapshot.albums.len(),
            snapshot.artists.len()
        ));
        if !snapshot.home.sections.is_empty() {
            ui.label(format!("Home sections: {}", snapshot.home.sections.len()));
        }
        for warning in &snapshot.warnings {
            ui.colored_label(Color32::YELLOW, warning);
        }

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (LibraryTab::Home, "Home"),
                (LibraryTab::Overview, "Overview"),
                (LibraryTab::Playlists, "Playlists"),
                (LibraryTab::Songs, "Songs"),
                (LibraryTab::Albums, "Albums"),
                (LibraryTab::Artists, "Artists"),
            ] {
                ui.selectable_value(&mut self.library.selected_tab, tab, label);
            }
        });
        ui.add_space(8.0);

        match self.library.selected_tab {
            LibraryTab::Home => {
                ScrollArea::vertical()
                    .id_salt("home-page-scroll")
                    .show(ui, |ui| self.show_home(ui, snapshot));
            }
            LibraryTab::Overview => {
                ScrollArea::vertical()
                    .id_salt("overview-page-scroll")
                    .show(ui, |ui| self.show_overview(ui, snapshot));
            }
            LibraryTab::Playlists => self.show_playlists(ui, snapshot),
            LibraryTab::Songs => {
                ScrollArea::vertical()
                    .id_salt("songs-page-scroll")
                    .show(ui, |ui| self.show_songs(ui, snapshot));
            }
            LibraryTab::Albums => {
                ScrollArea::vertical()
                    .id_salt("albums-page-scroll")
                    .show(ui, |ui| self.show_albums(ui, snapshot));
            }
            LibraryTab::Artists => {
                ScrollArea::vertical()
                    .id_salt("artists-page-scroll")
                    .show(ui, |ui| self.show_artists(ui, snapshot));
            }
            LibraryTab::ArtistProfile => {
                ScrollArea::vertical()
                    .id_salt("artist-profile-scroll")
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.show_artist_profile(ui);
                    });
            }
            LibraryTab::UserProfile => {
                ScrollArea::vertical()
                    .id_salt("user-profile-scroll")
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        self.show_user_profile(ui);
                    });
            }
        }
    }

    fn show_home(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        let home = &snapshot.home;

        ui.heading("Home");
        ui.add_space(6.0);

        if !home.chips.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for chip in &home.chips {
                    let response = ui.add(Button::new(&chip.title).selected(chip.selected));
                    if response.clicked() {
                        if chip.selected {
                            self.set_home_filter(None);
                        } else {
                            self.set_home_filter(chip.params.clone());
                        }
                    }
                }
            });
            ui.add_space(10.0);
        }

        if let Some(banner) = &home.banner {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    self.show_thumbnail(ui, &banner.thumbnails, vec2(220.0, 96.0));
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&banner.title).strong());
                        if !banner.subtitle.is_empty() {
                            ui.small(&banner.subtitle);
                        }
                    });
                });
            });
            ui.add_space(10.0);
        }

        if home.sections.is_empty() {
            ui.label("No home recommendations available.");
            return;
        }

        for (section_index, section) in home.sections.iter().enumerate() {
            let featured = section_index == 0;
            let frame = if featured {
                Frame::default()
            } else {
                Frame::group(ui.style())
            };
            frame.show(ui, |ui| {
                if featured {
                    ui.label(RichText::new("FOR YOU").small().weak());
                    ui.heading(RichText::new(&section.title).size(28.0));
                } else {
                    ui.heading(&section.title);
                }
                ui.add_space(6.0);

                ScrollArea::horizontal()
                    .id_salt(("home-section-scroll", &section.title))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for item in &section.items {
                                let is_playlist =
                                    item.page_type.as_deref() == Some("MUSIC_PAGE_TYPE_PLAYLIST");
                                let is_selected = is_playlist
                                    && self.library.selected_playlist_id.as_deref()
                                        == item.browse_id.as_deref();
                                let playlist_clicked =
                                    self.show_home_media_card(ui, item, is_selected, is_playlist);

                                if is_playlist
                                    && playlist_clicked
                                    && let Some(browse_id) = &item.browse_id
                                {
                                    self.select_playlist(browse_id.clone());
                                    self.library.selected_tab = LibraryTab::Playlists;
                                }

                                ui.add_space(8.0);
                            }
                        });
                    });
            });
            ui.add_space(10.0);
        }
    }

    fn show_home_media_card(
        &mut self,
        ui: &mut Ui,
        item: &HomeItem,
        selected: bool,
        clickable: bool,
    ) -> bool {
        let card_id = ui.make_persistent_id((
            "home-item",
            item.browse_id.as_deref().unwrap_or(item.title.as_str()),
        ));
        let selection_progress = ui
            .ctx()
            .animate_bool(card_id.with("selected-animation"), selected);
        let fill = blend_color(
            Color32::from_gray(24),
            Color32::from_rgb(28, 48, 72),
            selection_progress,
        );
        let stroke = Stroke::new(
            1.0_f32,
            blend_color(
                Color32::from_gray(56),
                Color32::from_rgb(104, 178, 255),
                selection_progress,
            ),
        );

        let mut playlist_clicked = false;
        let mut artwork_rect = egui::Rect::NOTHING;
        ui.push_id(card_id.with("scope"), |ui| {
            Frame::group(ui.style())
                .fill(fill)
                .stroke(stroke)
                .corner_radius(12.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.set_width(176.0);
                    ui.vertical(|ui| {
                        artwork_rect =
                            egui::Rect::from_min_size(ui.cursor().min, vec2(160.0, 160.0));
                        self.show_thumbnail(ui, &item.thumbnails, vec2(160.0, 160.0));
                        ui.add_space(8.0);
                        if clickable {
                            playlist_clicked = ui
                                .add(
                                    egui::Label::new(RichText::new(&item.title).strong())
                                        .sense(egui::Sense::click()),
                                )
                                .clicked();
                        } else {
                            ui.label(RichText::new(&item.title).strong());
                        }
                        self.show_home_item_subtitle(ui, item);
                    });
                })
        });

        let play_response = ui.interact(
            artwork_rect,
            card_id.with("play-overlay"),
            egui::Sense::click(),
        );
        let hover_progress = ui.ctx().animate_bool_with_time(
            card_id.with("play-overlay-hover"),
            play_response.hovered(),
            0.22,
        );
        if clickable {
            playlist_clicked |= play_response.clicked();
        }
        let center = artwork_rect.center();
        let radius = 16.0 + hover_progress * 9.0;
        ui.painter().circle_filled(
            center,
            radius,
            Color32::from_black_alpha((hover_progress * 204.0) as u8),
        );
        let triangle_size = 0.7 + hover_progress * 0.35;
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(
                    center.x - 6.0 * triangle_size,
                    center.y - 10.0 * triangle_size,
                ),
                egui::pos2(
                    center.x - 6.0 * triangle_size,
                    center.y + 10.0 * triangle_size,
                ),
                egui::pos2(center.x + 11.0 * triangle_size, center.y),
            ],
            Color32::from_white_alpha((hover_progress * 255.0) as u8),
            Stroke::NONE,
        ));

        playlist_clicked
    }

    fn show_home_item_subtitle(&mut self, ui: &mut Ui, item: &HomeItem) {
        if let (Some(user_name), Some(user_id)) = (&item.user_name, &item.user_id) {
            if ui.link(user_name).clicked() {
                self.select_user(user_id.clone());
            }
            return;
        }
        let (Some(artist_name), Some(artist_id)) = (&item.artist_name, &item.artist_id) else {
            ui.small(&item.subtitle);
            return;
        };
        let prefix = item
            .subtitle
            .strip_suffix(artist_name)
            .unwrap_or_default()
            .trim_end();

        ui.horizontal_wrapped(|ui| {
            if !prefix.is_empty() {
                ui.small(prefix);
            }
            if ui.link(artist_name).clicked() {
                self.select_artist(artist_id.clone());
            }
        });
    }

    fn show_user_profile(&mut self, ui: &mut Ui) {
        self.ensure_user_requested();
        if ui.button("← Back").clicked() {
            self.library.selected_tab = LibraryTab::Home;
            return;
        }
        match clone_async_state(self.library.user_state.state()) {
            AsyncView::Idle | AsyncView::Pending => {
                ui.spinner();
                ui.label("Loading user profile...");
            }
            AsyncView::Failed(error) => {
                log::error!("Failed to load user profile: {error:#}");
                ui.colored_label(
                    Color32::RED,
                    format!("Failed to load user profile: {error:#}"),
                );
                if ui.button("Retry user profile").clicked() {
                    self.library.user_state.clear();
                }
            }
            AsyncView::Finished(user) => {
                let profile_appearance = ui.ctx().animate_bool_with_time(
                    Id::new("user-profile-appearance")
                        .with(self.library.selected_user_id.as_deref().unwrap_or_default()),
                    true,
                    0.55,
                );
                ui.horizontal(|ui| {
                    self.show_thumbnail(ui, &user.thumbnails, vec2(144.0, 144.0));
                    ui.vertical(|ui| {
                        ui.heading(RichText::new(&user.name).size(32.0).color(
                            Color32::from_white_alpha((profile_appearance * 255.0) as u8),
                        ));
                        ui.label(RichText::new("YouTube Music profile").color(
                            Color32::from_white_alpha((profile_appearance * 180.0) as u8),
                        ));
                    });
                });
                ui.add_space(16.0);
                ui.heading("Playlists");
                for playlist in user.playlists.iter().take(12) {
                    ui.horizontal(|ui| {
                        self.show_thumbnail(ui, &playlist.thumbnails, vec2(56.0, 56.0));
                        ui.vertical(|ui| {
                            ui.label(&playlist.title);
                            ui.small(&playlist.subtitle);
                        });
                    });
                }
                ui.add_space(12.0);
                ui.heading("Videos");
                for video in user.videos.iter().take(12) {
                    ui.horizontal(|ui| {
                        self.show_thumbnail(ui, &video.thumbnails, vec2(84.0, 56.0));
                        ui.vertical(|ui| {
                            ui.label(&video.title);
                            ui.small(&video.subtitle);
                        });
                    });
                }
            }
        }
    }

    fn show_overview(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.group(|ui| {
            ui.heading("Quick stats");
            ui.label(format!("Playlists: {}", snapshot.playlists.len()));
            ui.label(format!("Songs: {}", snapshot.songs.len()));
            ui.label(format!("Albums: {}", snapshot.albums.len()));
            ui.label(format!("Artists: {}", snapshot.artists.len()));
            ui.label(format!("Home sections: {}", snapshot.home.sections.len()));
        });

        ui.add_space(10.0);
        ui.group(|ui| {
            ui.heading("First playlists");
            if snapshot.playlists.is_empty() {
                ui.label("No playlists found.");
            }

            ScrollArea::horizontal()
                .id_salt("overview-playlists-scroll")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for playlist in snapshot.playlists.iter().take(8) {
                            let selected = self.library.selected_playlist_id.as_deref()
                                == Some(playlist.playlist_id.get_raw());
                            let response = self.show_media_card(
                                ui,
                                ("overview-playlist", playlist.playlist_id.get_raw()),
                                &playlist.thumbnails,
                                &playlist.title,
                                &format!("{} | {}", playlist.author, playlist.tracks),
                                selected,
                                vec2(144.0, 144.0),
                            );
                            if response.clicked() {
                                self.select_playlist(playlist.playlist_id.get_raw().to_owned());
                                self.library.selected_tab = LibraryTab::Playlists;
                            }
                        }
                    });
                });
        });
    }

    fn show_playlists(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        self.ensure_playlist_requested();

        ui.columns(2, |columns| {
            columns[0].group(|ui| {
                ui.heading("Playlists");
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .id_salt("playlists-list-scroll")
                    .show(ui, |ui| {
                        if snapshot.playlists.is_empty()
                            && snapshot.recommended_playlists.is_empty()
                        {
                            ui.label("No playlists available.");
                        }

                        for playlist in &snapshot.playlists {
                            let selected = self.library.selected_playlist_id.as_deref()
                                == Some(playlist.playlist_id.get_raw());
                            let response = self.show_media_card(
                                ui,
                                ("library-playlist", playlist.playlist_id.get_raw()),
                                &playlist.thumbnails,
                                &playlist.title,
                                &format!("{} | {}", playlist.author, playlist.tracks),
                                selected,
                                vec2(112.0, 112.0),
                            );
                            if response.clicked() {
                                self.select_playlist(playlist.playlist_id.get_raw().to_owned());
                            }
                            ui.small(format!("id: {}", playlist.playlist_id.get_raw()));
                            ui.add_space(8.0);
                        }

                        if !snapshot.recommended_playlists.is_empty() {
                            ui.separator();
                            ui.heading("Recommended");
                            ui.add_space(6.0);

                            for category in &snapshot.recommended_playlists {
                                ui.label(RichText::new(&category.category_name).strong());
                                ui.add_space(4.0);
                                ScrollArea::horizontal()
                                    .id_salt((
                                        "recommended-category-scroll",
                                        &category.category_name,
                                    ))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            for playlist in &category.playlists {
                                                let selected =
                                                    self.library.selected_playlist_id.as_deref()
                                                        == Some(playlist.playlist_id.get_raw());
                                                let response = self.show_media_card(
                                                    ui,
                                                    (
                                                        "recommended-playlist",
                                                        playlist.playlist_id.get_raw(),
                                                    ),
                                                    &playlist.thumbnails,
                                                    &playlist.title,
                                                    &playlist.author,
                                                    selected,
                                                    vec2(112.0, 112.0),
                                                );
                                                if response.clicked() {
                                                    self.select_playlist(
                                                        playlist.playlist_id.get_raw().to_owned(),
                                                    );
                                                }
                                                ui.add_space(8.0);
                                            }
                                        });
                                    });
                                ui.add_space(6.0);
                            }
                        }
                    });
            });

            columns[1].group(|ui| {
                ui.heading("Playlist details");
                ui.add_space(6.0);

                match clone_async_state(self.library.playlist_state.state()) {
                    AsyncView::Idle => self.show_idle_playlist_state(ui),
                    AsyncView::Pending => {
                        ui.spinner();
                        ui.label("Loading playlist details...");
                    }
                    AsyncView::Finished(playlist) => self.show_playlist_details(ui, &playlist),
                    AsyncView::Failed(error) => {
                        ui.colored_label(Color32::RED, format!("Failed to load playlist: {error}"));
                        if ui.button("Retry playlist").clicked() {
                            self.library.playlist_state.clear();
                        }
                    }
                }
            });
        });
    }

    fn show_idle_playlist_state(&self, ui: &mut Ui) {
        if self.library.selected_playlist_id.is_some() {
            ui.label("Preparing playlist request...");
        } else {
            ui.label("Select a playlist on the left.");
        }
    }

    fn show_playlist_details(&mut self, ui: &mut Ui, playlist: &PlaylistSnapshot) {
        ui.horizontal(|ui| {
            self.show_thumbnail(ui, &playlist.details.thumbnails, vec2(128.0, 128.0));
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.heading(&playlist.details.title);
                ui.label(format!("Author: {}", playlist.details.author));
                ui.label(format!("Tracks: {}", playlist.details.track_count_text));
                ui.label(format!("Duration: {}", playlist.details.duration));
            });
        });

        if let Some(description) = &playlist.details.description
            && !description.trim().is_empty()
        {
            ui.add_space(8.0);
            ui.label(description);
        }

        ui.add_space(8.0);
        ui.separator();
        ui.label(format!("Items loaded: {}", playlist.tracks.len()));
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt("playlist-details-tracks-scroll")
            .show(ui, |ui| {
                for (index, item) in playlist.tracks.iter().enumerate() {
                    let (title, subtitle) = playlist_item_summary(item);
                    ui.push_id(("playlist-track", index, &title), |ui| {
                        ui.horizontal(|ui| {
                            self.show_thumbnail(
                                ui,
                                playlist_item_thumbnails(item),
                                vec2(52.0, 52.0),
                            );
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.label(&title);
                                ui.small(&subtitle);
                            });
                        });
                        ui.add_space(6.0);
                    });
                }
            });
    }

    fn show_songs(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Songs");
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt("songs-scroll")
            .show(ui, |ui| {
                if snapshot.songs.is_empty() {
                    ui.label("No songs available.");
                }
                for song in snapshot.songs.iter().take(100) {
                    let artists = if song.artists.is_empty() {
                        "Unknown artist".to_string()
                    } else {
                        song.artists
                            .iter()
                            .map(|artist| artist.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };

                    ui.push_id(("song-row", &song.title, &song.video_id), |ui| {
                        ui.horizontal(|ui| {
                            self.show_thumbnail(ui, &song.thumbnails, vec2(56.0, 56.0));
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.label(&song.title);
                                ui.small(format!(
                                    "{} | {} | {}",
                                    artists, song.album.name, song.duration
                                ));
                            });
                        });
                        ui.add_space(6.0);
                    });
                }
            });
    }

    fn show_albums(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Albums");
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt("albums-scroll")
            .show(ui, |ui| {
                if snapshot.albums.is_empty() {
                    ui.label("No albums available.");
                }
                for album in &snapshot.albums {
                    ui.push_id(("album-row", &album.title, &album.album_id), |ui| {
                        ui.horizontal(|ui| {
                            self.show_thumbnail(ui, &album.thumbnails, vec2(68.0, 68.0));
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.label(&album.title);
                                ui.small(format!(
                                    "{} | {} | {:?}",
                                    album.artist, album.year, album.album_type
                                ));
                            });
                        });
                        ui.add_space(6.0);
                    });
                }
            });
    }

    fn show_artists(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Artists");
        ui.add_space(6.0);

        ScrollArea::vertical()
            .id_salt("artists-scroll")
            .show(ui, |ui| {
                if snapshot.artists.is_empty() {
                    ui.label("No artists available.");
                }
                for artist in &snapshot.artists {
                    ui.push_id(("artist-row", &artist.artist, &artist.byline), |ui| {
                        if ui.button(&artist.artist).clicked() {
                            self.select_artist(artist.channel_id.get_raw().to_owned());
                        }
                        ui.small(&artist.byline);
                        ui.add_space(6.0);
                    });
                }
            });
    }

    fn show_artist_profile(&mut self, ui: &mut Ui) {
        self.ensure_artist_requested();
        if ui.button("← Back to artists").clicked() {
            self.library.selected_tab = LibraryTab::Artists;
            return;
        }
        ui.add_space(8.0);

        match clone_async_state(self.library.artist_state.state()) {
            AsyncView::Idle | AsyncView::Pending => {
                ui.spinner();
                ui.label("Loading artist profile...");
            }
            AsyncView::Failed(error) => {
                ui.colored_label(
                    Color32::RED,
                    format!("Failed to load artist profile: {error}"),
                );
                if ui.button("Retry artist profile").clicked() {
                    self.library.artist_state.clear();
                }
            }
            AsyncView::Finished(artist) => {
                let profile_appearance = ui.ctx().animate_bool_with_time(
                    Id::new("artist-profile-appearance").with(
                        self.library
                            .selected_artist_id
                            .as_deref()
                            .unwrap_or_default(),
                    ),
                    true,
                    0.55,
                );
                Frame::group(ui.style())
                    .fill(Color32::from_rgb(35, 29, 22))
                    .corner_radius(16.0)
                    .inner_margin(20.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            self.show_thumbnail(ui, &artist.thumbnails, vec2(160.0, 160.0));
                            ui.add_space(18.0);
                            ui.vertical(|ui| {
                                ui.heading(RichText::new(&artist.name).size(34.0).color(
                                    Color32::from_white_alpha((profile_appearance * 255.0) as u8),
                                ));
                                ui.label(
                                    artist
                                        .subscribers
                                        .as_deref()
                                        .or(artist.views.as_deref())
                                        .unwrap_or("Artist"),
                                );
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    let _ = ui.add_enabled(false, egui::Button::new("Shuffle"));
                                    let _ = ui.add_enabled(false, egui::Button::new("Radio"));
                                    ui.label(if artist.subscribed {
                                        "Subscribed"
                                    } else {
                                        "Not subscribed"
                                    });
                                });
                            });
                        });
                    });
                ui.add_space(18.0);

                if let Some(songs) = &artist.top_releases.songs {
                    ui.heading("Top tracks");
                    for song in songs.results.iter().take(10) {
                        ui.horizontal(|ui| {
                            ui.hyperlink_to(
                                RichText::new(&song.title).strong(),
                                format!(
                                    "https://music.youtube.com/watch?v={}",
                                    song.video_id.get_raw()
                                ),
                            );
                            ui.separator();
                            ui.hyperlink_to(
                                &song.album.name,
                                format!(
                                    "https://music.youtube.com/browse/{}",
                                    song.album.id.get_raw()
                                ),
                            );
                            ui.separator();
                            for (index, contributor) in song.artists.iter().enumerate() {
                                if index > 0 {
                                    ui.label(",");
                                }
                                if let Some(id) = &contributor.id {
                                    ui.hyperlink_to(
                                        &contributor.name,
                                        format!(
                                            "https://music.youtube.com/channel/{}",
                                            id.get_raw()
                                        ),
                                    );
                                } else {
                                    ui.small(&contributor.name);
                                }
                            }
                            ui.separator();
                            ui.small(&song.plays);
                        });
                        ui.separator();
                    }
                } else {
                    ui.label("No top tracks available for this artist.");
                }
            }
        }
    }

    fn show_auth_setup_form(&mut self, ctx: &Context) {
        Area::new(Id::new("auth_form"))
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                Frame::group(ui.style())
                    .corner_radius(8.0)
                    .inner_margin(16.0)
                    .show(ui, |ui| {
                        ui.heading("Authentication");
                        ui.add_space(8.0);
                        ui.label("Cookie string");
                        ui.add(
                            TextEdit::multiline(&mut self.auth.cookie_input)
                                .desired_width(420.0)
                                .desired_rows(4),
                        );

                        ui.label("Cookie file path");
                        ui.add(TextEdit::singleline(&mut self.auth.cookie_file_input));

                        if ui.button("Use cookie").clicked() {
                            self.auth.cookie = Some(self.auth.cookie_input.clone());
                            self.auth.cookie_file = Some(self.auth.cookie_file_input.clone());
                            self.auth.client_id = None;
                            self.auth.client_secret = None;
                            self.auth.current_state.clear();
                            self.auth.previous_state.take();
                        }

                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(12.0);
                        ui.label("Client ID");
                        ui.add(TextEdit::singleline(&mut self.auth.client_id_input));

                        ui.label("Client Secret");
                        ui.add(
                            TextEdit::singleline(&mut self.auth.client_secret_input).password(true),
                        );

                        ui.vertical_centered(|ui| {
                            ui.add_space(16.0 - ui.spacing().item_spacing.y);

                            if ui.button(t!("auth.retry_button")).clicked() {
                                self.auth.client_id = Some(self.auth.client_id_input.clone());
                                self.auth.client_secret =
                                    Some(self.auth.client_secret_input.clone());
                                if !self.auth.cookie_input.trim().is_empty() {
                                    self.auth.cookie = Some(self.auth.cookie_input.clone());
                                }
                                if !self.auth.cookie_file_input.trim().is_empty() {
                                    self.auth.cookie_file =
                                        Some(self.auth.cookie_file_input.clone());
                                }

                                self.auth.client_id_input.clear();
                                self.auth.client_secret_input.clear();
                                self.auth.current_state.clear();
                                self.auth.previous_state.take();
                            }
                        });
                    });
            });
    }
}

impl App for Application {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.plugin_or_default::<egui_async::EguiAsyncPlugin>();

        CentralPanel::default().show(ctx, |ui| {
            if self.auth.yt_client.is_none() {
                if self.auth.has_any_auth_source() {
                    self.process_auth(ui);
                } else {
                    self.show_auth_setup_form(ctx);
                }

                ctx.request_repaint_after_secs(0.1);
                return;
            }

            self.ensure_library_requested();

            match clone_async_state(self.library.current_state.state()) {
                AsyncView::Idle => {
                    ui.spinner();
                    ui.label("Preparing library request...");
                }
                AsyncView::Pending => {
                    Area::new(Id::new("library_loading"))
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.heading(t!("auth.success_title"));
                                ui.label(t!("auth.welcome"));
                                ui.add_space(12.0);
                                ui.spinner();
                                ui.label("Loading your YouTube Music home and library...");
                            });
                        });
                }
                AsyncView::Finished(snapshot) => {
                    self.show_library(ui, &snapshot);
                }
                AsyncView::Failed(error) => {
                    ui.colored_label(
                        Color32::RED,
                        format!("Failed to load library data: {error}"),
                    );
                    if ui.button("Retry library").clicked() {
                        self.reload_library();
                    }
                }
            }
        });
    }
}

pub(super) fn run() -> Result<()> {
    let options = NativeOptions {
        vsync: true,
        centered: true,
        hardware_acceleration: HardwareAcceleration::Preferred,
        viewport: ViewportBuilder::default()
            .with_app_id("ytm")
            .with_inner_size(vec2(1200.0, 800.0))
            .with_min_inner_size(vec2(800.0, 600.0)),
        ..Default::default()
    };

    match eframe::run_native(
        "Youtube Music",
        options,
        Box::new(|cc| Ok(Box::new(Application::new(&cc.egui_ctx)))),
    ) {
        Ok(_) => Ok(()),
        Err(error) => {
            log::error!("{error}");
            Err(anyhow!("{error}"))
        }
    }
}
