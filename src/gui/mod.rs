mod auth;

use anyhow::{Error, Result, anyhow};
use rust_i18n::t;
use std::env;

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{
    Align2, Area, CentralPanel, Context, Frame, Id, ScrollArea, TextEdit, Ui, Vec2,
    ViewportBuilder, vec2,
};
use egui_async::{Bind, StateWithData};

use ytmapi_rs::{
    YtMusic,
    auth::OAuthToken,
    common::{PlaylistID, YoutubeID},
    parse::{
        GetPlaylistDetails, LibraryArtist, LibraryPlaylist, PlaylistItem, SearchResultAlbum,
        TableListSong,
    },
};

use auth::AuthState;

struct ApplicationAuth {
    client_id: Option<String>,
    client_secret: Option<String>,
    client_id_input: String,
    client_secret_input: String,

    current_state: Bind<AuthState, Error>,
    previous_state: Option<AuthState>,

    yt_client: Option<YtMusic<OAuthToken>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibraryTab {
    Overview,
    Playlists,
    Songs,
    Albums,
    Artists,
}

#[derive(Clone)]
struct LibrarySnapshot {
    playlists: Vec<LibraryPlaylist>,
    songs: Vec<TableListSong>,
    albums: Vec<SearchResultAlbum>,
    artists: Vec<LibraryArtist>,
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
}

impl Application {
    fn new(ctx: &Context) -> Self {
        ctx.set_zoom_factor(1.5);

        let client_id = env::var("CLIENT_ID").ok().map(|s| s.trim().to_string());
        let client_secret = env::var("CLIENT_SECRET").ok().map(|s| s.trim().to_string());

        Self {
            auth: ApplicationAuth {
                client_id,
                client_secret,
                client_id_input: String::new(),
                client_secret_input: String::new(),
                current_state: Bind::new(true),
                previous_state: None,
                yt_client: None,
            },
            library: ApplicationLibrary {
                current_state: Bind::new(true),
                playlist_state: Bind::new(true),
                selected_tab: LibraryTab::Overview,
                selected_playlist_id: None,
            },
        }
    }

    fn ensure_library_requested(&mut self) {
        let Some(yt) = self.auth.yt_client.clone() else {
            return;
        };

        if matches!(self.library.current_state.state(), StateWithData::Idle) {
            self.library.current_state.request(load_library_snapshot(yt));
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

    fn show_library(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        if self.library.selected_playlist_id.is_none() {
            if let Some(first_playlist) = snapshot.playlists.first() {
                self.select_playlist(first_playlist.playlist_id.get_raw().to_owned());
            }
        }

        ui.horizontal(|ui| {
            ui.heading("YouTube Music Library");
            if ui.button("Reload").clicked() {
                self.library.current_state.clear();
                self.library.playlist_state.clear();
            }
        });

        ui.label(format!(
            "Loaded {} playlists, {} songs, {} albums and {} artists.",
            snapshot.playlists.len(),
            snapshot.songs.len(),
            snapshot.albums.len(),
            snapshot.artists.len()
        ));

        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
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
            LibraryTab::Overview => self.show_overview(ui, snapshot),
            LibraryTab::Playlists => self.show_playlists(ui, snapshot),
            LibraryTab::Songs => self.show_songs(ui, snapshot),
            LibraryTab::Albums => self.show_albums(ui, snapshot),
            LibraryTab::Artists => self.show_artists(ui, snapshot),
        }
    }

    fn show_overview(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.group(|ui| {
            ui.heading("Quick stats");
            ui.label(format!("Playlists: {}", snapshot.playlists.len()));
            ui.label(format!("Songs: {}", snapshot.songs.len()));
            ui.label(format!("Albums: {}", snapshot.albums.len()));
            ui.label(format!("Artists: {}", snapshot.artists.len()));
        });

        ui.add_space(10.0);
        ui.group(|ui| {
            ui.heading("First playlists");
            for playlist in snapshot.playlists.iter().take(5) {
                let selected = self.library.selected_playlist_id.as_deref()
                    == Some(playlist.playlist_id.get_raw());
                if ui
                    .selectable_label(
                        selected,
                        format!(
                            "{}  |  {}  |  {}",
                            playlist.title, playlist.author, playlist.tracks
                        ),
                    )
                    .clicked()
                {
                    self.select_playlist(playlist.playlist_id.get_raw().to_owned());
                    self.library.selected_tab = LibraryTab::Playlists;
                }
            }
        });
    }

    fn show_playlists(&mut self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        self.ensure_playlist_requested();

        ui.columns(2, |columns| {
            columns[0].group(|ui| {
                ui.heading("Playlists");
                ui.add_space(6.0);
                ScrollArea::vertical().show(ui, |ui| {
                    for playlist in &snapshot.playlists {
                        let selected = self.library.selected_playlist_id.as_deref()
                            == Some(playlist.playlist_id.get_raw());

                        if ui
                            .selectable_label(
                                selected,
                                format!(
                                    "{}\n{} | {}",
                                    playlist.title, playlist.author, playlist.tracks
                                ),
                            )
                            .clicked()
                        {
                            self.select_playlist(playlist.playlist_id.get_raw().to_owned());
                        }

                        ui.small(format!("id: {}", playlist.playlist_id.get_raw()));
                        ui.add_space(6.0);
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
                        ui.heading(&playlist.details.title);
                        ui.label(format!("Author: {}", playlist.details.author));
                        ui.label(format!("Tracks: {}", playlist.details.track_count_text));
                        ui.label(format!("Duration: {}", playlist.details.duration));

                        if let Some(description) = &playlist.details.description {
                            if !description.trim().is_empty() {
                                ui.label(description);
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.label(format!("Items loaded: {}", playlist.tracks.len()));
                        ui.add_space(6.0);

                        ScrollArea::vertical().show(ui, |ui| {
                            for item in &playlist.tracks {
                                let (title, subtitle) = playlist_item_summary(item);
                                ui.label(title);
                                ui.small(subtitle);
                                ui.add_space(6.0);
                            }
                        });
                    }
                    AsyncView::Failed(error) => {
                        ui.colored_label(
                            egui::Color32::RED,
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

    fn show_songs(&self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Songs");
        ui.add_space(6.0);

        ScrollArea::vertical().show(ui, |ui| {
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

                ui.label(&song.title);
                ui.small(format!(
                    "{} | {} | {}",
                    artists, song.album.name, song.duration
                ));
                ui.add_space(6.0);
            }
        });
    }

    fn show_albums(&self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Albums");
        ui.add_space(6.0);

        ScrollArea::vertical().show(ui, |ui| {
            for album in &snapshot.albums {
                ui.label(&album.title);
                ui.small(format!(
                    "{} | {} | {:?}",
                    album.artist, album.year, album.album_type
                ));
                ui.add_space(6.0);
            }
        });
    }

    fn show_artists(&self, ui: &mut Ui, snapshot: &LibrarySnapshot) {
        ui.heading("Artists");
        ui.add_space(6.0);

        ScrollArea::vertical().show(ui, |ui| {
            for artist in &snapshot.artists {
                ui.label(&artist.artist);
                ui.small(&artist.byline);
                ui.add_space(6.0);
            }
        });
    }
}

impl App for Application {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.plugin_or_default::<egui_async::EguiAsyncPlugin>();

        CentralPanel::default().show(ctx, |ui| {
            if self.auth.yt_client.is_none() {
                if self.auth.client_id.is_some() && self.auth.client_secret.is_some() {
                    self.process_auth(ui);
                } else {
                    Area::new(Id::new("auth_form"))
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(ctx, |ui| {
                            Frame::group(ui.style())
                                .corner_radius(8.0)
                                .inner_margin(16.0)
                                .show(ui, |ui| {
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

                                            self.auth.client_id_input.clear();
                                            self.auth.client_secret_input.clear();
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
                                ui.label("Loading your YouTube Music library...");
                            });
                        });
                }
                AsyncView::Finished(snapshot) => {
                    self.show_library(ui, &snapshot);
                }
                AsyncView::Failed(error) => {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!("Failed to load library data: {error}"),
                    );
                    if ui.button("Retry library").clicked() {
                        self.library.current_state.clear();
                        self.library.playlist_state.clear();
                    }
                }
            }
        });
    }
}

async fn load_library_snapshot(yt: YtMusic<OAuthToken>) -> Result<LibrarySnapshot> {
    let playlists = yt.get_library_playlists().await?;
    let songs = yt.get_library_songs().await?;
    let albums = yt.get_library_albums().await?;
    let artists = yt.get_library_artists().await?;

    Ok(LibrarySnapshot {
        playlists,
        songs,
        albums,
        artists,
    })
}

async fn load_playlist_snapshot(
    yt: YtMusic<OAuthToken>,
    playlist_id: String,
) -> Result<PlaylistSnapshot> {
    let playlist_id = PlaylistID::from_raw(playlist_id);
    let details = yt.get_playlist_details(playlist_id.clone()).await?;
    let tracks = yt.get_playlist_tracks(playlist_id).await?;

    Ok(PlaylistSnapshot { details, tracks })
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

fn clone_async_state<T: Clone>(state: StateWithData<'_, T, Error>) -> AsyncView<T> {
    match state {
        StateWithData::Idle => AsyncView::Idle,
        StateWithData::Pending => AsyncView::Pending,
        StateWithData::Finished(data) => AsyncView::Finished(data.clone()),
        StateWithData::Failed(error) => AsyncView::Failed(error.to_string()),
    }
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
