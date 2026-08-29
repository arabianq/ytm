use super::{
    AppAuthToken, AsyncView, GetHomeQuery, GetUserProfileQuery, HomeSnapshot, LibrarySnapshot,
    PlaylistSnapshot, UserProfile,
};
use anyhow::{Context as _, Result, anyhow};
use egui_async::StateWithData;
use std::sync::OnceLock;
use ytmapi_rs::{
    YtMusic,
    common::{ArtistChannelID, PlaylistID, Thumbnail, YoutubeID},
    parse::PlaylistItem,
};

pub(super) async fn load_library_snapshot(
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
                Some(first_category) => {
                    match yt.get_mood_playlists(first_category.params.clone()).await {
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
                    }
                }
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

pub(super) async fn load_playlist_snapshot(
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

pub(super) async fn load_artist_profile(
    yt: YtMusic<AppAuthToken>,
    artist_id: String,
) -> Result<ytmapi_rs::parse::GetArtist> {
    yt.get_artist(ArtistChannelID::from_raw(artist_id))
        .await
        .context("failed to load artist profile")
}

pub(super) async fn load_user_profile(
    yt: YtMusic<AppAuthToken>,
    user_id: String,
) -> Result<UserProfile> {
    yt.query(GetUserProfileQuery::new(user_id))
        .await
        .context("failed to load user profile")
}

pub(super) fn playlist_item_summary(item: &PlaylistItem) -> (String, String) {
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

pub(super) fn playlist_item_thumbnails(item: &PlaylistItem) -> &[Thumbnail] {
    match item {
        PlaylistItem::Song(song) => &song.thumbnails,
        PlaylistItem::Video(video) => &video.thumbnails,
        PlaylistItem::Episode(episode) => &episode.thumbnails,
        PlaylistItem::UploadSong(song) => &song.thumbnails,
    }
}

pub(super) fn playlist_item_video_id(item: &PlaylistItem) -> &str {
    match item {
        PlaylistItem::Song(song) => song.video_id.get_raw(),
        PlaylistItem::Video(video) => video.video_id.get_raw(),
        PlaylistItem::Episode(episode) => episode.episode_id.get_raw(),
        PlaylistItem::UploadSong(song) => song.video_id.get_raw(),
    }
}

pub(super) fn preferred_thumbnail_url(thumbnails: &[Thumbnail]) -> Option<&str> {
    thumbnails
        .iter()
        .max_by_key(|thumbnail| thumbnail.width.saturating_mul(thumbnail.height))
        .map(|thumbnail| thumbnail.url.as_str())
}

pub(super) fn clone_async_state<T: Clone>(
    state: StateWithData<'_, T, anyhow::Error>,
) -> AsyncView<T> {
    match state {
        StateWithData::Idle => AsyncView::Idle,
        StateWithData::Pending => AsyncView::Pending,
        StateWithData::Finished(data) => AsyncView::Finished(data.clone()),
        StateWithData::Failed(error) => AsyncView::Failed(error.to_string()),
    }
}

pub(super) fn is_empty_section_error(error: &impl std::fmt::Display) -> bool {
    let message = error.to_string();
    message.contains("musicShelfRenderer not found") || message.contains("gridRenderer not found")
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

pub(super) async fn download_thumbnail(url: String) -> Result<Vec<u8>> {
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
