use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use std::{
    env, fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc, OnceLock, RwLock,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlaybackStatus {
    Idle,
    Preparing,
    Playing,
    Paused,
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlaybackSnapshot {
    pub current_track: Option<String>,
    pub status: PlaybackStatus,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            current_track: None,
            status: PlaybackStatus::Idle,
        }
    }
}

enum PlaybackCommand {
    Play { video_id: String, title: String },
    TogglePause,
    Stop,
    Shutdown,
}

struct DownloadResult {
    request_id: u64,
    video_id: String,
    result: Result<Vec<u8>, String>,
}

struct PlaybackSession {
    player: Player,
    _device_sink: MixerDeviceSink,
}

struct ProviderConfig {
    node: PathBuf,
    server_home: PathBuf,
}

static PROVIDER_WARMUP: OnceLock<Result<(), String>> = OnceLock::new();

pub(super) struct PlaybackController {
    command_tx: Sender<PlaybackCommand>,
    snapshot: Arc<RwLock<PlaybackSnapshot>>,
}

impl PlaybackController {
    pub(super) fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let snapshot = Arc::new(RwLock::new(PlaybackSnapshot::default()));
        let worker_snapshot = Arc::clone(&snapshot);
        thread::Builder::new()
            .name("playback-worker".to_owned())
            .spawn(move || playback_worker(command_rx, worker_snapshot))
            .expect("failed to start playback worker");

        Self {
            command_tx,
            snapshot,
        }
    }

    pub(super) fn play(&self, video_id: impl Into<String>, title: impl Into<String>) {
        let _ = self.command_tx.send(PlaybackCommand::Play {
            video_id: video_id.into(),
            title: title.into(),
        });
    }

    pub(super) fn toggle_pause(&self) {
        let _ = self.command_tx.send(PlaybackCommand::TogglePause);
    }

    pub(super) fn stop(&self) {
        let _ = self.command_tx.send(PlaybackCommand::Stop);
    }

    pub(super) fn snapshot(&self) -> PlaybackSnapshot {
        self.snapshot
            .read()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }
}

impl Drop for PlaybackController {
    fn drop(&mut self) {
        let _ = self.command_tx.send(PlaybackCommand::Shutdown);
    }
}

fn playback_worker(command_rx: Receiver<PlaybackCommand>, snapshot: Arc<RwLock<PlaybackSnapshot>>) {
    let (download_tx, download_rx) = mpsc::channel::<DownloadResult>();
    let mut current_request_id = 0_u64;
    let mut session: Option<PlaybackSession> = None;

    loop {
        while let Ok(download) = download_rx.try_recv() {
            if download.request_id != current_request_id {
                continue;
            }
            match download.result.and_then(start_session) {
                Ok(new_session) => {
                    session = Some(new_session);
                    update_status(&snapshot, PlaybackStatus::Playing);
                }
                Err(error) => {
                    session = None;
                    update_status(
                        &snapshot,
                        PlaybackStatus::Error(format!("{error} (video_id: {})", download.video_id)),
                    );
                }
            }
        }

        if session
            .as_ref()
            .is_some_and(|session| session.player.empty())
            && matches!(read_snapshot(&snapshot).status, PlaybackStatus::Playing)
        {
            stop_session(&mut session);
            update_snapshot(&snapshot, PlaybackSnapshot::default());
        }

        match command_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(PlaybackCommand::Play { video_id, title }) => {
                current_request_id = current_request_id.wrapping_add(1);
                stop_session(&mut session);
                update_snapshot(
                    &snapshot,
                    PlaybackSnapshot {
                        current_track: Some(title),
                        status: PlaybackStatus::Preparing,
                    },
                );
                spawn_download(current_request_id, video_id, download_tx.clone());
            }
            Ok(PlaybackCommand::TogglePause) => {
                let status = read_snapshot(&snapshot).status;
                if let Some(active_session) = session.as_ref() {
                    match status {
                        PlaybackStatus::Playing => {
                            active_session.player.pause();
                            update_status(&snapshot, PlaybackStatus::Paused);
                        }
                        PlaybackStatus::Paused => {
                            active_session.player.play();
                            update_status(&snapshot, PlaybackStatus::Playing);
                        }
                        _ => {}
                    }
                }
            }
            Ok(PlaybackCommand::Stop) => {
                current_request_id = current_request_id.wrapping_add(1);
                stop_session(&mut session);
                update_snapshot(&snapshot, PlaybackSnapshot::default());
            }
            Ok(PlaybackCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop_session(&mut session);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
fn spawn_download(request_id: u64, video_id: String, download_tx: Sender<DownloadResult>) {
    thread::spawn(move || {
        let result = download_audio(&video_id);
        let _ = download_tx.send(DownloadResult {
            request_id,
            video_id: video_id.clone(),
            result,
        });
    });
}

fn download_audio(video_id: &str) -> Result<Vec<u8>, String> {
    let provider = provider_config()?;
    PROVIDER_WARMUP
        .get_or_init(|| warm_up_provider(&provider))
        .clone()?;

    let video_url = format!("https://www.youtube.com/watch?v={video_id}");
    let output = run_yt_dlp(&video_url, &provider)?;

    if !output.status.success() {
        return Err(yt_dlp_failure(&output));
    }
    if output.stdout.is_empty() {
        return Err("yt-dlp returned no audio data".to_owned());
    }

    Ok(output.stdout)
}

/// Path to a Netscape-format cookie file exported from the browser.
/// Raw cookies are never passed via argv or headers: they are visible in the
/// process list.
fn cookie_file_path() -> Result<Option<PathBuf>, String> {
    let configured = match env::var_os("YTM_COOKIE_FILE") {
        Some(path) if !path.is_empty() => path,
        _ => return Ok(None),
    };
    let path = PathBuf::from(configured);
    if !path.is_file() {
        return Err(format!(
            "YTM_COOKIE_FILE points to a missing file: {}",
            path.display()
        ));
    }
    Ok(Some(path))
}

fn run_yt_dlp(video_url: &str, provider: &ProviderConfig) -> Result<Output, String> {
    let js_runtime = format!("node:{}", provider.node.display());
    let provider_argument = format!(
        "youtubepot-bgutilscript:server_home={}",
        provider.server_home.display()
    );
    let mut arguments = vec![
        "--no-playlist".to_owned(),
        "--no-progress".to_owned(),
        "--js-runtimes".to_owned(),
        js_runtime,
        "--extractor-args".to_owned(),
        provider_argument,
        "--format".to_owned(),
        // Symphonia (used by rodio) has no Opus decoder, so prefer AAC in MP4;
        // fall back to whatever bestaudio is available otherwise.
        "bestaudio[acodec^=mp4a]/bestaudio".to_owned(),
        "--output".to_owned(),
        "-".to_owned(),
    ];
    if let Some(cookie_file) = cookie_file_path()? {
        arguments.push("--cookies".to_owned());
        arguments.push(cookie_file.display().to_string());
    }
    arguments.push("--".to_owned());
    arguments.push(video_url.to_owned());
    let candidates = yt_dlp_candidates();
    let mut last_not_found = None;

    for executable in candidates {
        match Command::new(&executable).args(&arguments).output() {
            Ok(output) => return Ok(output),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                last_not_found = Some(error);
            }
            Err(error) => {
                return Err(format!(
                    "failed to start yt-dlp at {}: {error}",
                    executable.display()
                ));
            }
        }
    }

    Err(format!(
        "yt-dlp was not found. Set YTM_YT_DLP to yt-dlp.exe{}",
        last_not_found
            .map(|error| format!(" ({error})"))
            .unwrap_or_default()
    ))
}

fn provider_config() -> Result<ProviderConfig, String> {
    let node = env::var_os("YTM_NODE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(default_node_path)
        .unwrap_or_else(|| PathBuf::from("node"));
    let configured_home = env::var_os("YTM_PO_PROVIDER_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let mut server_home = configured_home
        .or_else(|| {
            env::var_os("USERPROFILE").map(|home| {
                PathBuf::from(home)
                    .join("bgutil-ytdlp-pot-provider")
                    .join("server")
            })
        })
        .ok_or_else(|| {
            "bgutil provider home was not found. Set YTM_PO_PROVIDER_HOME to its server directory"
                .to_owned()
        })?;

    if server_home.join("server").is_dir() {
        server_home = server_home.join("server");
    }
    if !server_home.join("build").join("generate_once.js").is_file() {
        return Err(format!(
            "bgutil provider is not built at {}. Set YTM_PO_PROVIDER_HOME to its server directory",
            server_home.display()
        ));
    }

    Ok(ProviderConfig { node, server_home })
}

fn default_node_path() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }

    let node = PathBuf::from(r"C:\Program Files\nodejs\node.exe");
    node.is_file().then_some(node)
}

fn warm_up_provider(provider: &ProviderConfig) -> Result<(), String> {
    let script = provider.server_home.join("build").join("generate_once.js");
    let output = Command::new(&provider.node)
        .arg(script)
        .arg("--version")
        .output()
        .map_err(|error| format!("failed to start Node for bgutil provider: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "bgutil provider warm-up failed with {}: {}",
            output.status,
            redact_urls(&String::from_utf8_lossy(&output.stderr))
        ))
    }
}

fn yt_dlp_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(path) = env::var_os("YTM_YT_DLP").filter(|path| !path.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from("yt-dlp"));

    if cfg!(windows)
        && let Some(local_app_data) = env::var_os("LOCALAPPDATA")
    {
        let winget_packages = PathBuf::from(local_app_data)
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        if let Some(path) = find_winget_yt_dlp(&winget_packages) {
            candidates.push(path);
        }
    }

    candidates
}

fn find_winget_yt_dlp(packages: &Path) -> Option<PathBuf> {
    fs::read_dir(packages)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("yt-dlp.yt-dlp_")
                .then(|| entry.path().join("yt-dlp.exe"))
                .filter(|path| path.is_file())
        })
}

fn yt_dlp_failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let safe_detail = safe_yt_dlp_detail(&stderr);

    if is_provider_failure(&stderr) || safe_detail.contains("403") {
        format!(
            "yt-dlp could not obtain YouTube audio through the PO Token Provider: {safe_detail}"
        )
    } else {
        format!("yt-dlp failed with {}: {safe_detail}", output.status)
    }
}

fn safe_yt_dlp_detail(stderr: &str) -> String {
    let detail = stderr
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("ERROR:"))
        .or_else(|| stderr.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("no error details")
        .trim();
    let redacted = redact_urls(detail);

    redacted.chars().take(500).collect()
}

fn is_provider_failure(stderr: &str) -> bool {
    [
        "Error fetching PO Token",
        "Unable to fetch PO Token",
        "PO Token Providers: none",
        "No PO Token Providers",
        "provider is not available",
    ]
    .iter()
    .any(|marker| stderr.contains(marker))
}

fn redact_urls(message: &str) -> String {
    let mut redacted = message.to_owned();

    while let Some(start) = redacted
        .find("https://")
        .or_else(|| redacted.find("http://"))
    {
        let end = redacted[start..]
            .find(char::is_whitespace)
            .map(|offset| start + offset)
            .unwrap_or(redacted.len());
        redacted.replace_range(start..end, "[redacted URL]");
    }

    redacted
}

fn start_session(bytes: Vec<u8>) -> Result<PlaybackSession, String> {
    let decoder = Decoder::new(Cursor::new(bytes))
        .map_err(|error| format!("failed to decode downloaded audio: {error}"))?;
    let device_sink = DeviceSinkBuilder::open_default_sink()
        .map_err(|error| format!("failed to open audio output: {error}"))?;
    let player = Player::connect_new(device_sink.mixer());
    player.append(decoder);

    Ok(PlaybackSession {
        player,
        _device_sink: device_sink,
    })
}

fn stop_session(session: &mut Option<PlaybackSession>) {
    if let Some(active_session) = session.take() {
        active_session.player.stop();
    }
}

fn read_snapshot(snapshot: &Arc<RwLock<PlaybackSnapshot>>) -> PlaybackSnapshot {
    snapshot
        .read()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

fn update_status(snapshot: &Arc<RwLock<PlaybackSnapshot>>, status: PlaybackStatus) {
    match snapshot.write() {
        Ok(mut snapshot) => snapshot.status = status,
        Err(poisoned) => poisoned.into_inner().status = status,
    }
}

fn update_snapshot(snapshot: &Arc<RwLock<PlaybackSnapshot>>, new_snapshot: PlaybackSnapshot) {
    match snapshot.write() {
        Ok(mut snapshot) => *snapshot = new_snapshot,
        Err(poisoned) => *poisoned.into_inner() = new_snapshot,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_provider_failure, redact_urls, safe_yt_dlp_detail, yt_dlp_candidates};
    use std::path::Path;

    #[test]
    fn redacts_signed_media_url_from_error() {
        let message = "ERROR: HTTP 403 at https://example.test/audio?sig=secret details";

        let redacted = redact_urls(message);

        assert_eq!(redacted, "ERROR: HTTP 403 at [redacted URL] details");
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn always_tries_command_from_path() {
        assert!(
            yt_dlp_candidates()
                .iter()
                .any(|candidate| candidate == Path::new("yt-dlp"))
        );
    }

    #[test]
    fn successful_provider_log_does_not_mask_real_error() {
        let stderr = "[youtube] [pot:bgutil:script-node] Generating a gvs PO Token\nERROR: [youtube] requested format is not available";

        assert!(!is_provider_failure(stderr));
        assert_eq!(
            safe_yt_dlp_detail(stderr),
            "ERROR: [youtube] requested format is not available"
        );
    }

    #[test]
    fn recognizes_provider_failure_and_redacts_url() {
        let stderr = "WARNING: Error fetching PO Token\nERROR: HTTP Error 403 at https://example.test/audio?sig=secret";

        assert!(is_provider_failure(stderr));
        assert_eq!(
            safe_yt_dlp_detail(stderr),
            "ERROR: HTTP Error 403 at [redacted URL]"
        );
    }
}
