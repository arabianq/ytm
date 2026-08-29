mod auth;
mod data;
mod fonts;
mod home;
mod playback;
mod ui;
mod widgets;

use anyhow::Result;
use egui_async::Bind;
use serde_json::{Value, json};
use std::{borrow::Cow, collections::HashMap};

use ytmapi_rs::{
    YtMusic,
    parse::{
        GetArtist, GetPlaylistDetails, LibraryArtist, LibraryPlaylist, MoodPlaylistCategory,
        ParseFrom, PlaylistItem, ProcessedResult, SearchResultAlbum, TableListSong,
    },
    query::{PostMethod, PostQuery, Query},
};

use auth::{AppAuthToken, AuthBootstrap, AuthState};
use home::parse_home_snapshot;
use ytmapi_rs::common::Thumbnail;

struct ApplicationAuth {
    client_id: Option<String>,
    client_secret: Option<String>,
    cookie: Option<String>,
    cookie_file: Option<String>,
    client_id_input: String,
    client_secret_input: String,
    cookie_input: String,
    cookie_file_input: String,
    current_state: Bind<AuthState, anyhow::Error>,
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
    ArtistProfile,
    UserProfile,
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
    artist_name: Option<String>,
    artist_id: Option<String>,
    user_name: Option<String>,
    user_id: Option<String>,
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
    current_state: Bind<LibrarySnapshot, anyhow::Error>,
    playlist_state: Bind<PlaylistSnapshot, anyhow::Error>,
    artist_state: Bind<GetArtist, anyhow::Error>,
    user_state: Bind<UserProfile, anyhow::Error>,
    selected_tab: LibraryTab,
    selected_playlist_id: Option<String>,
    selected_home_params: Option<String>,
    selected_artist_id: Option<String>,
    selected_user_id: Option<String>,
}

#[derive(Clone, Debug)]
struct GetHomeQuery {
    params: Option<String>,
}

#[derive(Clone, Debug)]
struct UserProfile {
    name: String,
    thumbnails: Vec<Thumbnail>,
    playlists: Vec<UserProfileItem>,
    videos: Vec<UserProfileItem>,
}

#[derive(Clone, Debug)]
struct UserProfileItem {
    title: String,
    subtitle: String,
    thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug)]
struct GetUserProfileQuery {
    user_id: String,
}

impl GetUserProfileQuery {
    fn new(user_id: String) -> Self {
        Self { user_id }
    }
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
        let mut header =
            serde_json::Map::from_iter([("browseId".to_string(), json!("FEmusic_home"))]);
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

impl<A: ytmapi_rs::auth::AuthToken> Query<A> for GetUserProfileQuery {
    type Output = UserProfile;
    type Method = PostMethod;
}

impl PostQuery for GetUserProfileQuery {
    fn header(&self) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::from_iter([("browseId".to_string(), json!(self.user_id))])
    }

    fn params(&self) -> Vec<(&str, Cow<'_, str>)> {
        vec![]
    }

    fn path(&self) -> &str {
        "browse"
    }
}

impl ParseFrom<GetUserProfileQuery> for UserProfile {
    fn parse_from(p: ProcessedResult<GetUserProfileQuery>) -> ytmapi_rs::Result<Self> {
        let root = serde_json::from_str::<Value>(&p.source).unwrap_or(Value::Null);
        let header = root
            .pointer("/header/musicVisualHeaderRenderer")
            .unwrap_or(&Value::Null);
        let name = text_at(header, "/title/runs").unwrap_or_else(|| "YouTube Music profile".into());
        let thumbnails = thumbnails_at(
            header,
            "/foregroundThumbnail/musicThumbnailRenderer/thumbnail/thumbnails",
        );
        let contents = root
            .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut playlists = Vec::new();
        let mut videos = Vec::new();
        for section in contents {
            let Some(carousel) = section.get("musicCarouselShelfRenderer") else {
                continue;
            };
            let title = text_at(
                carousel,
                "/header/musicCarouselShelfBasicHeaderRenderer/title/runs",
            )
            .unwrap_or_default();
            let items = carousel
                .get("contents")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(parse_user_profile_item).collect())
                .unwrap_or_default();
            if title.eq_ignore_ascii_case("playlists") {
                playlists = items;
            } else if title.eq_ignore_ascii_case("videos") {
                videos = items;
            }
        }
        Ok(Self {
            name,
            thumbnails,
            playlists,
            videos,
        })
    }
}

fn parse_user_profile_item(value: &Value) -> Option<UserProfileItem> {
    let item = value.get("musicTwoRowItemRenderer")?;
    Some(UserProfileItem {
        title: text_at(item, "/title/runs")?,
        subtitle: text_at(item, "/subtitle/runs").unwrap_or_default(),
        thumbnails: thumbnails_at(
            item,
            "/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails",
        ),
    })
}

fn text_at(value: &Value, pointer: &str) -> Option<String> {
    let text = value
        .pointer(pointer)?
        .as_array()?
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.trim().is_empty()).then_some(text)
}

fn thumbnails_at(value: &Value, pointer: &str) -> Vec<Thumbnail> {
    value
        .pointer(pointer)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
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
    playback: playback::PlaybackController,
    thumbnail_state: HashMap<String, Bind<Vec<u8>, anyhow::Error>>,
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

pub fn run() -> Result<()> {
    ui::run()
}
