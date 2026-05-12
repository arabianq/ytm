mod auth;

use anyhow::{Context as _, Error, Result, anyhow};
use rust_i18n::t;
use serde_json::{Value, json};
use std::{borrow::Cow, collections::HashMap, env, sync::OnceLock};

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{
    Align2, Area, Button, CentralPanel, Color32, Context, Frame, Id, Image, RichText,
    ScrollArea, Sense, Stroke, TextEdit, Ui, Vec2, ViewportBuilder, vec2,
};
use egui_async::{Bind, StateWithData};

use ytmapi_rs::{
    YtMusic,
    common::{PlaylistID, Thumbnail, YoutubeID},
    parse::{
        GetPlaylistDetails, LibraryArtist, LibraryPlaylist, MoodPlaylistCategory, ParseFrom,
        PlaylistItem, ProcessedResult, SearchResultAlbum, TableListSong,
    },
    query::{PostMethod, PostQuery, Query},
};

use auth::{AppAuthToken, AuthBootstrap, AuthState};

struct ApplicationAuth {
    client_id: Option<String>,
    client_secret: Option<String>,
    cookie: Option<String>,
    cookie_file: Option<String>,
    client_id_input: String,
    client_secret_input: String,
    cookie_input: String,
    cookie_file_input: String,

    current_state: Bind<AuthState, Error>,
    previous_state: Option<AuthState>,

    yt_client: Option<YtMusic<AppAuthToken>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryTab {
    Home,
    Overview,
    Playlists,
    Songs,
    Albums,
    Artists,
}

#[derive(Clone, Debug, Default)]
struct HomeChip {
    title: String,
    params: Option<String>,
    selected: bool,
}

#[derive(Clone, Debug, Default)]
struct HomeBanner {
    title: String,
    subtitle: String,
    thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, Default)]
struct HomeItem {
    title: String,
    subtitle: String,
    browse_id: Option<String>,
    page_type: Option<String>,
    thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, Default)]
struct HomeSection {
    title: String,
    items: Vec<HomeItem>,
}

#[derive(Clone, Debug, Default)]
struct HomeSnapshot {
    chips: Vec<HomeChip>,
    sections: Vec<HomeSection>,
    banner: Option<HomeBanner>,
    background: Vec<Thumbnail>,
}

#[derive(Clone)]
struct LibrarySnapshot {
    home: HomeSnapshot,
    playlists: Vec<LibraryPlaylist>,
    songs: Vec<TableListSong>,
    albums: Vec<SearchResultAlbum>,
    artists: Vec<LibraryArtist>,
    recommended_playlists: Vec<MoodPlaylistCategory>,
    warnings: Vec<String>,
}

#[derive(Clone)]
struct PlaylistSnapshot {
    details: GetPlaylistDetails,
    tracks: Vec<PlaylistItem>,
}

struct ApplicationLibrary {
    current_state: Bind<LibrarySnapshot, Error>,
    playlist_state: Bind<PlaylistSnapshot, Error>,
    selected_tab: LibraryTab,
    selected_playlist_id: Option<String>,
    selected_home_params: Option<String>,
}

#[derive(Clone, Debug)]
struct GetHomeQuery {
    params: Option<String>,
}

impl GetHomeQuery {
    fn new(params: Option<String>) -> Self {
        Self { params }
    }
}

impl<A: ytmapi_rs::auth::AuthToken> Query<A> for GetHomeQuery {
    type Output = HomeSnapshot;
    type Method = PostMethod;
}

impl PostQuery for GetHomeQuery {
    fn header(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut header = serde_json::Map::from_iter([("browseId".to_string(), json!("FEmusic_home"))]);
        if let Some(params) = &self.params {
            header.insert("params".to_string(), json!(params));
        }
        header
    }

    fn params(&self) -> Vec<(&str, Cow<'_, str>)> {
        vec![]
    }

    fn path(&self) -> &str {
        "browse"
    }
}

impl ParseFrom<GetHomeQuery> for HomeSnapshot {
    fn parse_from(p: ProcessedResult<GetHomeQuery>) -> ytmapi_rs::Result<Self> {
        let root = serde_json::from_str::<Value>(&p.source).unwrap_or(Value::Null);
        Ok(parse_home_snapshot(&root))
    }
}

enum AsyncView<T> {
    Idle,
    Pending,
    Finished(T),
    Failed(String),
}

pub struct Application {
    auth: ApplicationAuth,
    library: ApplicationLibrary,
    thumbnail_state: HashMap<String, Bind<Vec<u8>, Error>>,
}

impl ApplicationAuth {
    fn bootstrap(&self) -> AuthBootstrap {
        AuthBootstrap {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            cookie: self.cookie.clone(),
            cookie_file: self.cookie_file.clone(),
        }
    }

    fn has_any_auth_source(&self) -> bool {
        self.bootstrap().has_any_auth_source()
    }
}

impl Application {
    fn new(ctx: &Context) -> Self {
        ctx.set_zoom_factor(1.5);
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
                selected_tab: LibraryTab::Home,
                selected_playlist_id: None,
                selected_home_params: None,
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

    fn reload_library(&mut self) {
        self.library.current_state.clear();
        self.library.playlist_state.clear();
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
            ui.label(format!(
                "Home sections: {}",
                snapshot.home.sections.len()
            ));
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
        }
    }

    fn show_home(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        let home = &snapshot.home;

        if !home.background.is_empty() {
            ui.group(|ui| {
                ui.heading("Home");
                ui.add_space(6.0);
                self.show_thumbnail(ui, &home.background, vec2(720.0, 180.0));
            });
            ui.add_space(10.0);
        } else {
            ui.heading("Home");
            ui.add_space(6.0);
        }

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

        for section in &home.sections {
            ui.group(|ui| {
                ui.heading(&section.title);
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
                            let response = self.show_media_card(
                                ui,
                                (
                                    "home-item",
                                    item.browse_id
                                        .as_deref()
                                        .unwrap_or(item.title.as_str()),
                                ),
                                &item.thumbnails,
                                &item.title,
                                &item.subtitle,
                                is_selected,
                                vec2(160.0, 160.0),
                            );

                            if is_playlist && response.clicked() {
                                if let Some(browse_id) = &item.browse_id {
                                    self.select_playlist(browse_id.clone());
                                    self.library.selected_tab = LibraryTab::Playlists;
                                }
                            }

                            ui.add_space(8.0);
                        }
                    });
                });
            });
            ui.add_space(10.0);
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
                    if snapshot.playlists.is_empty() && snapshot.recommended_playlists.is_empty() {
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
                                .id_salt(("recommended-category-scroll", &category.category_name))
                                .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    for playlist in &category.playlists {
                                        let selected = self.library.selected_playlist_id.as_deref()
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
                    AsyncView::Idle => {
                        if self.library.selected_playlist_id.is_some() {
                            ui.label("Preparing playlist request...");
                        } else {
                            ui.label("Select a playlist on the left.");
                        }
                    }
                    AsyncView::Pending => {
                        ui.spinner();
                        ui.label("Loading playlist details...");
                    }
                    AsyncView::Finished(playlist) => {
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

                        if let Some(description) = &playlist.details.description {
                            if !description.trim().is_empty() {
                                ui.add_space(8.0);
                                ui.label(description);
                            }
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
                    AsyncView::Failed(error) => {
                        ui.colored_label(
                            Color32::RED,
                            format!("Failed to load playlist: {error}"),
                        );
                        if ui.button("Retry playlist").clicked() {
                            self.library.playlist_state.clear();
                        }
                    }
                }
            });
        });
    }

    fn show_songs(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Songs");
        ui.add_space(6.0);

        ScrollArea::vertical().id_salt("songs-scroll").show(ui, |ui| {
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

        ScrollArea::vertical().id_salt("albums-scroll").show(ui, |ui| {
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

    fn show_artists(&self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Artists");
        ui.add_space(6.0);

        ScrollArea::vertical().id_salt("artists-scroll").show(ui, |ui| {
            if snapshot.artists.is_empty() {
                ui.label("No artists available.");
            }
            for artist in &snapshot.artists {
                ui.push_id(("artist-row", &artist.artist, &artist.byline), |ui| {
                    ui.label(&artist.artist);
                    ui.small(&artist.byline);
                    ui.add_space(6.0);
                });
            }
        });
    }

    fn show_media_card(
        &mut self,
        ui: &mut Ui,
        id_source: impl std::hash::Hash,
        thumbnails: &[Thumbnail],
        title: &str,
        subtitle: &str,
        selected: bool,
        image_size: Vec2,
    ) -> egui::Response {
        let card_id = ui.make_persistent_id(&id_source);
        let fill = if selected {
            Color32::from_rgb(28, 48, 72)
        } else {
            Color32::from_gray(24)
        };
        let stroke = if selected {
            Stroke::new(1.0, Color32::from_rgb(104, 178, 255))
        } else {
            Stroke::new(1.0, Color32::from_gray(56))
        };

        let frame = ui
            .push_id(card_id.with("scope"), |ui| {
                Frame::group(ui.style())
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(12.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(image_size.x + 16.0);
                        self.show_thumbnail(ui, thumbnails, image_size);
                        ui.add_space(8.0);
                        ui.label(RichText::new(title).strong());
                        if !subtitle.is_empty() {
                            ui.small(subtitle);
                        }
                    })
            })
            .inner;

        frame.response.interact(Sense::click())
    }

    fn show_thumbnail(&mut self, ui: &mut Ui, thumbnails: &[Thumbnail], size: Vec2) {
        let Some(url) = preferred_thumbnail_url(thumbnails).map(ToOwned::to_owned) else {
            show_thumbnail_placeholder(ui, size, "No image");
            return;
        };

        let state = self
            .thumbnail_state
            .entry(url.clone())
            .or_insert_with(|| Bind::new(true));

        if matches!(state.state(), StateWithData::Idle) {
            state.request(download_thumbnail(url.clone()));
        }

        match clone_async_state(state.state()) {
            AsyncView::Idle | AsyncView::Pending => {
                ui.ctx().request_repaint();
                show_thumbnail_placeholder(ui, size, "Loading");
            }
            AsyncView::Finished(bytes) => {
                ui.add(
                    Image::from_bytes(format!("bytes://thumbnail/{url}"), bytes)
                        .fit_to_exact_size(size)
                        .corner_radius(10),
                );
            }
            AsyncView::Failed(error) => {
                log::warn!("Thumbnail load failed for {url}: {error}");
                show_thumbnail_placeholder(ui, size, "Image error");
            }
        }
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
                                        self.auth.cookie_file =
                                            Some(self.auth.cookie_file_input.clone());
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
                                        TextEdit::singleline(&mut self.auth.client_secret_input)
                                            .password(true),
                                    );

                                    ui.vertical_centered(|ui| {
                                        ui.add_space(16.0 - ui.spacing().item_spacing.y);

                                        if ui.button(t!("auth.retry_button")).clicked() {
                                            self.auth.client_id =
                                                Some(self.auth.client_id_input.clone());
                                            self.auth.client_secret =
                                                Some(self.auth.client_secret_input.clone());
                                            if !self.auth.cookie_input.trim().is_empty() {
                                                self.auth.cookie =
                                                    Some(self.auth.cookie_input.clone());
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

async fn load_library_snapshot(
    yt: YtMusic<AppAuthToken>,
    home_params: Option<String>,
) -> Result<LibrarySnapshot> {
    let home_yt = yt.clone();
    let playlists_yt = yt.clone();
    let songs_yt = yt.clone();
    let albums_yt = yt.clone();
    let artists_yt = yt.clone();

    let (home_result, playlists_result, songs_result, albums_result, artists_result) = tokio::join!(
        home_yt.query(GetHomeQuery::new(home_params)),
        playlists_yt.get_library_playlists(),
        songs_yt.get_library_songs(),
        albums_yt.get_library_albums(),
        artists_yt.get_library_artists(),
    );

    let mut warnings = Vec::new();
    let mut loaded_any = false;

    let home = match home_result {
        Ok(home) => {
            if !home.sections.is_empty() || home.banner.is_some() || !home.chips.is_empty() {
                loaded_any = true;
            }
            home
        }
        Err(error) => {
            warnings.push(format!("Home unavailable: {error}"));
            HomeSnapshot::default()
        }
    };

    let playlists = match playlists_result {
        Ok(playlists) => {
            loaded_any = true;
            playlists
        }
        Err(error) => {
            warnings.push(format!("Playlists unavailable: {error}"));
            Vec::new()
        }
    };

    let songs = match songs_result {
        Ok(songs) => {
            loaded_any = true;
            songs
        }
        Err(error) => {
            if !is_empty_section_error(&error) {
                warnings.push(format!("Songs unavailable: {error}"));
            }
            Vec::new()
        }
    };

    let albums = match albums_result {
        Ok(albums) => {
            loaded_any = true;
            albums
        }
        Err(error) => {
            if !is_empty_section_error(&error) {
                warnings.push(format!("Albums unavailable: {error}"));
            }
            Vec::new()
        }
    };

    let artists = match artists_result {
        Ok(artists) => {
            loaded_any = true;
            artists
        }
        Err(error) => {
            warnings.push(format!("Artists unavailable: {error}"));
            Vec::new()
        }
    };

    let library_is_empty =
        playlists.is_empty() && songs.is_empty() && albums.is_empty() && artists.is_empty();

    let recommended_playlists = if library_is_empty {
        match yt.get_mood_categories().await {
            Ok(categories) => match categories
                .first()
                .and_then(|section| section.mood_categories.first())
            {
                Some(first_category) => match yt.get_mood_playlists(first_category.params.clone()).await
                {
                    Ok(playlists) => {
                        if !playlists.is_empty() {
                            loaded_any = true;
                        }
                        playlists
                    }
                    Err(error) => {
                        warnings.push(format!("Recommendations unavailable: {error}"));
                        Vec::new()
                    }
                },
                None => Vec::new(),
            },
            Err(error) => {
                warnings.push(format!("Recommendations unavailable: {error}"));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    if !loaded_any {
        return Err(anyhow!("Failed to load library sections"));
    }

    Ok(LibrarySnapshot {
        home,
        playlists,
        songs,
        albums,
        artists,
        recommended_playlists,
        warnings,
    })
}

async fn load_playlist_snapshot(
    yt: YtMusic<AppAuthToken>,
    playlist_id: String,
) -> Result<PlaylistSnapshot> {
    let playlist_id = PlaylistID::from_raw(playlist_id);
    let details = yt
        .get_playlist_details(playlist_id.clone())
        .await
        .context("failed to load playlist details")?;
    let tracks = yt
        .get_playlist_tracks(playlist_id)
        .await
        .context("failed to load playlist tracks")?;

    Ok(PlaylistSnapshot { details, tracks })
}

fn thumbnail_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
            )
            .build()
            .expect("thumbnail http client")
    })
}

async fn download_thumbnail(url: String) -> Result<Vec<u8>> {
    let response = thumbnail_http_client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to request thumbnail: {url}"))?
        .error_for_status()
        .with_context(|| format!("thumbnail request failed: {url}"))?;

    Ok(response
        .bytes()
        .await
        .with_context(|| format!("failed to read thumbnail bytes: {url}"))?
        .to_vec())
}

fn parse_home_snapshot(root: &Value) -> HomeSnapshot {
    let single_column = root
        .pointer("/contents/singleColumnBrowseResultsRenderer")
        .unwrap_or(&Value::Null);
    let tab = root
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer")
        .unwrap_or(&Value::Null);
    let contents = tab
        .pointer("/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let sections = contents.iter().filter_map(parse_home_section).collect();
    let banner = contents.iter().find_map(parse_home_banner);
    let chips = single_column
        .pointer("/header/chipCloudRenderer/chips")
        .and_then(Value::as_array)
        .map(|chips| chips.iter().filter_map(parse_home_chip).collect())
        .unwrap_or_default();
    let background = thumbnails_at(root, "/background/musicThumbnailRenderer/thumbnail/thumbnails");

    HomeSnapshot {
        chips,
        sections,
        banner,
        background,
    }
}

fn parse_home_section(section: &Value) -> Option<HomeSection> {
    let carousel = section.get("musicCarouselShelfRenderer")?;
    let title = runs_text_at(
        carousel,
        "/header/musicCarouselShelfBasicHeaderRenderer/title/runs",
    )
    .or_else(|| string_at(carousel, "/header/musicCarouselShelfBasicHeaderRenderer/title/text"))
    .unwrap_or_else(|| "Recommended".to_string());

    let items = carousel
        .get("contents")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_home_item).collect::<Vec<_>>())
        .unwrap_or_default();

    if items.is_empty() {
        None
    } else {
        Some(HomeSection { title, items })
    }
}

fn parse_home_banner(section: &Value) -> Option<HomeBanner> {
    let banner = section.get("musicTastebuilderShelfRenderer")?;
    let title = runs_text_at(banner, "/primaryText/runs")?;
    let subtitle = runs_text_at(banner, "/secondaryText/runs").unwrap_or_default();
    let thumbnails = thumbnails_at(
        banner,
        "/thumbnail/musicTastebuilderShelfThumbnailRenderer/thumbnail/thumbnails",
    );

    Some(HomeBanner {
        title,
        subtitle,
        thumbnails,
    })
}

fn parse_home_chip(chip: &Value) -> Option<HomeChip> {
    let chip = chip.get("chipCloudChipRenderer")?;
    let title = runs_text_at(chip, "/text/runs")?;
    let params = string_at(chip, "/navigationEndpoint/browseEndpoint/params");
    let selected = chip
        .get("isSelected")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Some(HomeChip {
        title,
        params,
        selected,
    })
}

fn parse_home_item(item: &Value) -> Option<HomeItem> {
    let item = item.get("musicTwoRowItemRenderer")?;
    let title = runs_text_at(item, "/title/runs")?;
    let subtitle = runs_text_at(item, "/subtitle/runs").unwrap_or_default();
    let browse_id = string_at(item, "/navigationEndpoint/browseEndpoint/browseId");
    let page_type = string_at(
        item,
        "/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType",
    );
    let thumbnails = thumbnails_at(
        item,
        "/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails",
    );

    Some(HomeItem {
        title,
        subtitle,
        browse_id,
        page_type,
        thumbnails,
    })
}

fn show_thumbnail_placeholder(ui: &mut Ui, size: Vec2, label: &str) {
    Frame::group(ui.style())
        .fill(Color32::from_gray(40))
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_min_size(size);
            ui.centered_and_justified(|ui| {
                ui.small(label);
            });
        });
}

fn preferred_thumbnail_url(thumbnails: &[Thumbnail]) -> Option<&str> {
    thumbnails
        .iter()
        .max_by_key(|thumbnail| thumbnail.width.saturating_mul(thumbnail.height))
        .map(|thumbnail| thumbnail.url.as_str())
}

fn runs_text_at(value: &Value, pointer: &str) -> Option<String> {
    let runs = value.pointer(pointer)?.as_array()?;
    let text = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<String>();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn string_at(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn thumbnails_at(value: &Value, pointer: &str) -> Vec<Thumbnail> {
    value.pointer(pointer)
        .cloned()
        .and_then(|thumbs| serde_json::from_value::<Vec<Thumbnail>>(thumbs).ok())
        .unwrap_or_default()
}

fn playlist_item_summary(item: &PlaylistItem) -> (String, String) {
    match item {
        PlaylistItem::Song(song) => (
            song.title.clone(),
            format!(
                "{} | {} | {}",
                song.artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                song.album.name,
                song.duration
            ),
        ),
        PlaylistItem::Video(video) => (
            video.title.clone(),
            format!("{} | {}", video.channel_name, video.duration),
        ),
        PlaylistItem::Episode(episode) => (episode.title.clone(), episode.podcast_name.clone()),
        PlaylistItem::UploadSong(song) => {
            let artists = if song.artists.is_empty() {
                "Uploaded track".to_string()
            } else {
                song.artists
                    .iter()
                    .map(|artist| artist.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let album = song
                .album
                .as_ref()
                .map(|album| album.name.as_str())
                .unwrap_or("No album");

            (song.title.clone(), format!("{} | {}", artists, album))
        }
    }
}

fn playlist_item_thumbnails(item: &PlaylistItem) -> &[Thumbnail] {
    match item {
        PlaylistItem::Song(song) => &song.thumbnails,
        PlaylistItem::Video(video) => &video.thumbnails,
        PlaylistItem::Episode(episode) => &episode.thumbnails,
        PlaylistItem::UploadSong(song) => &song.thumbnails,
    }
}

fn clone_async_state<T: Clone>(state: StateWithData<'_, T, Error>) -> AsyncView<T> {
    match state {
        StateWithData::Idle => AsyncView::Idle,
        StateWithData::Pending => AsyncView::Pending,
        StateWithData::Finished(data) => AsyncView::Finished(data.clone()),
        StateWithData::Failed(error) => AsyncView::Failed(error.to_string()),
    }
}

fn is_empty_section_error(error: &impl std::fmt::Display) -> bool {
    let message = error.to_string();
    message.contains("musicShelfRenderer not found")
        || message.contains("gridRenderer not found")
}

pub fn run() -> Result<()> {
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
        Err(e) => {
            log::error!("{e}");
            Err(anyhow!(format!("{e}")))
        }
    }
}
