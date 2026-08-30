use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::{
    env, fs,
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
pub(super) struct QueuedTrack {
    pub video_id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlaybackCover {
    pub uri: String,
    pub bytes: Arc<[u8]>,
    /// Tiny (64x64) PNG recompression of the cover: stretching it over the
    /// big-player background gives a cheap blur, like YouTube Music.
    pub blur_uri: String,
    pub blur_bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PlaybackSnapshot {
    pub current_track: Option<String>,
    pub status: PlaybackStatus,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub volume: f32,
    pub cover: Option<Arc<PlaybackCover>>,
    pub shuffle: bool,
    pub queue: Arc<Vec<QueuedTrack>>,
    pub queue_index: usize,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            current_track: None,
            status: PlaybackStatus::Idle,
            position: Duration::ZERO,
            duration: None,
            volume: 1.0,
            cover: None,
            shuffle: false,
            queue: Arc::new(Vec::new()),
            queue_index: 0,
        }
    }
}

enum PlaybackCommand {
    Play {
        video_id: String,
        title: String,
    },
    PlayQueue {
        tracks: Vec<QueuedTrack>,
        start_index: usize,
    },
    TogglePause,
    Stop,
    SetVolume(f32),
    Seek(Duration),
    Next,
    Previous,
    JumpTo(usize),
    ToggleShuffle,
    Shutdown,
}

struct DownloadResult {
    request_id: u64,
    video_id: String,
    result: Result<Arc<StreamState>, String>,
}

/// Cancellation for an in-flight download: the worker cancels it whenever the
/// request is superseded (Stop, track switch, shutdown) so the spawned yt-dlp
/// process does not keep downloading into the void.
struct JobControl {
    child: Mutex<Option<Child>>,
    cancelled: AtomicBool,
}

impl JobControl {
    fn new() -> Self {
        Self {
            child: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        kill_child(self);
    }
}

/// Kills and reaps the spawned yt-dlp process if it has not finished yet.
fn kill_child(control: &JobControl) {
    if let Some(mut child) = control.child.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Cancels the in-flight download of the superseded track and bumps the
/// request id so late results from it are ignored.
fn supersede_download(active: &mut Option<Arc<JobControl>>, request_id: &mut u64) {
    if let Some(control) = active.take() {
        control.cancel();
    }
    *request_id = request_id.wrapping_add(1);
}

/// Growable buffer fed by the yt-dlp download thread and read by the audio
/// decoder: playback starts once the first chunk is buffered while the rest
/// of the track keeps downloading.
struct StreamState {
    inner: Mutex<StreamInner>,
    updated: Condvar,
}

struct StreamInner {
    data: Vec<u8>,
    done: bool,
    error: Option<String>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(StreamInner {
                data: Vec::new(),
                done: false,
                error: None,
            }),
            updated: Condvar::new(),
        }
    }

    fn append(&self, chunk: &[u8]) {
        let mut inner = self.inner.lock().unwrap();
        inner.data.extend_from_slice(chunk);
        drop(inner);
        self.updated.notify_all();
    }

    fn finish(&self, error: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.done = true;
        inner.error = error;
        drop(inner);
        self.updated.notify_all();
    }

    fn error(&self) -> Option<String> {
        self.inner.lock().unwrap().error.clone()
    }
}

struct StreamSource {
    state: Arc<StreamState>,
    pos: u64,
}

impl Read for StreamSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut inner = self.state.inner.lock().unwrap();
        loop {
            let buffered = inner.data.len() as u64;
            if self.pos < buffered {
                let start = self.pos as usize;
                let n = (buffered as usize - start).min(out.len());
                out[..n].copy_from_slice(&inner.data[start..start + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(error) = &inner.error {
                return Err(io::Error::other(error.clone()));
            }
            if inner.done {
                return Ok(0);
            }
            inner = self
                .state
                .updated
                .wait_timeout(inner, Duration::from_secs(1))
                .unwrap()
                .0;
        }
    }
}

impl Seek for StreamSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut inner = self.state.inner.lock().unwrap();
        let target = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => {
                // The stream has no fixed end until the download finishes.
                while !inner.done && inner.error.is_none() {
                    inner = self
                        .state
                        .updated
                        .wait_timeout(inner, Duration::from_secs(1))
                        .unwrap()
                        .0;
                }
                inner.data.len() as i64 + d
            }
        };
        if target < 0 {
            return Err(io::Error::other("seek before start"));
        }
        let target = target as u64;
        while (target as usize) > inner.data.len() {
            if let Some(error) = &inner.error {
                return Err(io::Error::other(error.clone()));
            }
            if inner.done {
                break;
            }
            inner = self
                .state
                .updated
                .wait_timeout(inner, Duration::from_secs(1))
                .unwrap()
                .0;
        }
        self.pos = target.min(inner.data.len() as u64);
        Ok(self.pos)
    }
}

struct PlaybackSession {
    player: Player,
    _device_sink: MixerDeviceSink,
}

struct ProviderConfig {
    node: PathBuf,
    server_home: PathBuf,
}

/// Provider warm-up cache: a success lives for the whole process, a failure is
/// retried after [`PROVIDER_WARMUP_RETRY`] so one transient Node failure does
/// not require an app restart.
static PROVIDER_WARMUP: Mutex<Option<(Instant, Result<(), String>)>> = Mutex::new(None);

/// How long a failed provider warm-up is remembered before it is retried.
const PROVIDER_WARMUP_RETRY: Duration = Duration::from_secs(30);

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

    pub(super) fn set_volume(&self, volume: f32) {
        let _ = self
            .command_tx
            .send(PlaybackCommand::SetVolume(volume.clamp(0.0, 1.0)));
    }

    pub(super) fn seek(&self, position: Duration) {
        let _ = self.command_tx.send(PlaybackCommand::Seek(position));
    }

    pub(super) fn play_queue(&self, tracks: Vec<QueuedTrack>, start_index: usize) {
        let _ = self.command_tx.send(PlaybackCommand::PlayQueue {
            tracks,
            start_index,
        });
    }

    pub(super) fn next(&self) {
        let _ = self.command_tx.send(PlaybackCommand::Next);
    }

    pub(super) fn previous(&self) {
        let _ = self.command_tx.send(PlaybackCommand::Previous);
    }

    pub(super) fn jump_to(&self, index: usize) {
        let _ = self.command_tx.send(PlaybackCommand::JumpTo(index));
    }

    pub(super) fn toggle_shuffle(&self) {
        let _ = self.command_tx.send(PlaybackCommand::ToggleShuffle);
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
    let (cover_tx, cover_rx) = mpsc::channel::<(u64, Option<Arc<PlaybackCover>>)>();
    let mut current_request_id = 0_u64;
    let mut active_download: Option<Arc<JobControl>> = None;
    let mut session: Option<PlaybackSession> = None;
    let mut queue: Vec<QueuedTrack> = Vec::new();
    let mut queue_index: usize = 0;
    let mut shuffle = false;
    let mut current_stream: Option<Arc<StreamState>> = None;
    // Position to jump to after a track is (re)started; used to recover from
    // the rodio/symphonia bug where a backward seek kills the source.
    let mut resume_at: Option<Duration> = None;
    let stored_volume = stored_volume();
    mutate_snapshot(&snapshot, |snapshot| snapshot.volume = stored_volume);

    loop {
        while let Ok(download) = download_rx.try_recv() {
            if download.request_id != current_request_id {
                continue;
            }
            let volume = read_snapshot(&snapshot).volume;
            let video_id = download.video_id.clone();
            current_stream = download.result.clone().ok();
            match download
                .result
                .and_then(|source| start_session(source, volume))
            {
                Ok((new_session, duration)) => {
                    let resume = resume_at.take();
                    session = Some(new_session);
                    mutate_snapshot(&snapshot, |snapshot| {
                        snapshot.status = PlaybackStatus::Playing;
                        snapshot.position = resume.unwrap_or(Duration::ZERO);
                        snapshot.duration = duration;
                    });
                    if let Some(at) = resume
                        && let Some(active) = session.as_ref()
                    {
                        let _ = active.player.try_seek(at);
                    }
                }
                Err(error) => {
                    session = None;
                    update_status(
                        &snapshot,
                        PlaybackStatus::Error(format!("{error} (video_id: {video_id})")),
                    );
                }
            }
        }

        while let Ok((request_id, cover)) = cover_rx.try_recv() {
            if request_id != current_request_id {
                continue;
            }
            if let Some(cover) = cover {
                mutate_snapshot(&snapshot, |snapshot| snapshot.cover = Some(cover));
            }
        }

        if session
            .as_ref()
            .is_some_and(|session| session.player.empty())
            && matches!(read_snapshot(&snapshot).status, PlaybackStatus::Playing)
        {
            let state = read_snapshot(&snapshot);
            let natural_end = match state.duration {
                Some(duration) => {
                    state.position >= duration.saturating_sub(Duration::from_millis(500))
                }
                None => true,
            };
            stop_session(&mut session);
            if natural_end {
                if let Some(next) = pick_neighbour_index(queue.len(), queue_index, shuffle, true) {
                    queue_index = next;
                    resume_at = None;
                    supersede_download(&mut active_download, &mut current_request_id);
                    begin_track(
                        &snapshot,
                        current_request_id,
                        &queue,
                        queue_index,
                        shuffle,
                        &download_tx,
                        &cover_tx,
                        &mut active_download,
                    );
                } else {
                    reset_snapshot(&snapshot);
                }
            } else if let Some(error) = current_stream.as_ref().and_then(|stream| stream.error()) {
                // The download was interrupted; restarting would fail again.
                stop_session(&mut session);
                update_status(
                    &snapshot,
                    PlaybackStatus::Error(format!(
                        "download interrupted: {error} (video_id: {})",
                        queue[queue_index].video_id
                    )),
                );
            } else {
                // The source died long before the end of the track; this
                // happens after some backward seeks in rodio/symphonia.
                // Restart the current track and jump back to where it died.
                resume_at = Some(state.position);
                supersede_download(&mut active_download, &mut current_request_id);
                begin_track(
                    &snapshot,
                    current_request_id,
                    &queue,
                    queue_index,
                    shuffle,
                    &download_tx,
                    &cover_tx,
                    &mut active_download,
                );
            }
        }

        if let Some(active_session) = session.as_ref() {
            let status = read_snapshot(&snapshot).status;
            if matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused) {
                let position = active_session.player.get_pos();
                mutate_snapshot(&snapshot, |snapshot| {
                    if snapshot.position != position {
                        snapshot.position = position;
                    }
                });
            }
        }

        match command_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(PlaybackCommand::Play { video_id, title }) => {
                queue = vec![QueuedTrack { video_id, title }];
                queue_index = 0;
                resume_at = None;
                supersede_download(&mut active_download, &mut current_request_id);
                stop_session(&mut session);
                begin_track(
                    &snapshot,
                    current_request_id,
                    &queue,
                    queue_index,
                    shuffle,
                    &download_tx,
                    &cover_tx,
                    &mut active_download,
                );
            }
            Ok(PlaybackCommand::PlayQueue {
                tracks,
                start_index,
            }) => {
                if tracks.is_empty() {
                    continue;
                }
                queue = tracks;
                queue_index = start_index.min(queue.len() - 1);
                resume_at = None;
                supersede_download(&mut active_download, &mut current_request_id);
                stop_session(&mut session);
                begin_track(
                    &snapshot,
                    current_request_id,
                    &queue,
                    queue_index,
                    shuffle,
                    &download_tx,
                    &cover_tx,
                    &mut active_download,
                );
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
                supersede_download(&mut active_download, &mut current_request_id);
                resume_at = None;
                current_stream = None;
                stop_session(&mut session);
                reset_snapshot(&snapshot);
            }
            Ok(PlaybackCommand::Next) => {
                if let Some(next) = pick_neighbour_index(queue.len(), queue_index, shuffle, true) {
                    queue_index = next;
                    supersede_download(&mut active_download, &mut current_request_id);
                    stop_session(&mut session);
                    begin_track(
                        &snapshot,
                        current_request_id,
                        &queue,
                        queue_index,
                        shuffle,
                        &download_tx,
                        &cover_tx,
                        &mut active_download,
                    );
                }
            }
            Ok(PlaybackCommand::Previous) => {
                if let Some(prev) = pick_neighbour_index(queue.len(), queue_index, shuffle, false) {
                    queue_index = prev;
                    supersede_download(&mut active_download, &mut current_request_id);
                    stop_session(&mut session);
                    begin_track(
                        &snapshot,
                        current_request_id,
                        &queue,
                        queue_index,
                        shuffle,
                        &download_tx,
                        &cover_tx,
                        &mut active_download,
                    );
                }
            }
            Ok(PlaybackCommand::JumpTo(index)) => {
                if index < queue.len() {
                    queue_index = index;
                    supersede_download(&mut active_download, &mut current_request_id);
                    stop_session(&mut session);
                    begin_track(
                        &snapshot,
                        current_request_id,
                        &queue,
                        queue_index,
                        shuffle,
                        &download_tx,
                        &cover_tx,
                        &mut active_download,
                    );
                }
            }
            Ok(PlaybackCommand::ToggleShuffle) => {
                shuffle = !shuffle;
                mutate_snapshot(&snapshot, |snapshot| {
                    snapshot.shuffle = shuffle;
                    snapshot.queue = Arc::new(queue.clone());
                    snapshot.queue_index = queue_index;
                });
            }
            Ok(PlaybackCommand::SetVolume(value)) => {
                if let Some(active_session) = session.as_ref() {
                    active_session.player.set_volume(value);
                }
                mutate_snapshot(&snapshot, |snapshot| snapshot.volume = value);
                store_volume(value);
            }
            Ok(PlaybackCommand::Seek(position)) => {
                if let Some(active_session) = session.as_ref() {
                    let _ = active_session.player.try_seek(position);
                    mutate_snapshot(&snapshot, |snapshot| snapshot.position = position);
                }
            }
            Ok(PlaybackCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                supersede_download(&mut active_download, &mut current_request_id);
                stop_session(&mut session);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn begin_track(
    snapshot: &Arc<RwLock<PlaybackSnapshot>>,
    request_id: u64,
    queue: &[QueuedTrack],
    index: usize,
    shuffle: bool,
    download_tx: &Sender<DownloadResult>,
    cover_tx: &Sender<(u64, Option<Arc<PlaybackCover>>)>,
    active: &mut Option<Arc<JobControl>>,
) {
    let Some(track) = queue.get(index) else {
        return;
    };
    let volume = read_snapshot(snapshot).volume;
    update_snapshot(
        snapshot,
        PlaybackSnapshot {
            current_track: Some(track.title.clone()),
            status: PlaybackStatus::Preparing,
            position: Duration::ZERO,
            duration: None,
            volume,
            cover: None,
            shuffle,
            queue: Arc::new(queue.to_vec()),
            queue_index: index,
        },
    );
    *active = Some(spawn_download(
        request_id,
        track.video_id.clone(),
        download_tx.clone(),
        cover_tx.clone(),
    ));
}

/// Picks the next (or previous) queue index. Shuffle picks a random other
/// track; sequential mode stops at the edges (no wrap-around).
fn pick_neighbour_index(len: usize, current: usize, shuffle: bool, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if shuffle {
        if len == 1 {
            return Some(current);
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let mut candidate = (nanos as usize) % len;
        if candidate == current {
            candidate = (candidate + 1) % len;
        }
        return Some(candidate);
    }
    if forward {
        (current + 1 < len).then_some(current + 1)
    } else {
        current.checked_sub(1)
    }
}

fn spawn_download(
    request_id: u64,
    video_id: String,
    download_tx: Sender<DownloadResult>,
    cover_tx: Sender<(u64, Option<Arc<PlaybackCover>>)>,
) -> Arc<JobControl> {
    let control = Arc::new(JobControl::new());
    {
        let control = Arc::clone(&control);
        let video_id = video_id.clone();
        thread::spawn(move || {
            let result = stream_download(&video_id, &control, |source| {
                download_tx
                    .send(DownloadResult {
                        request_id,
                        video_id: video_id.clone(),
                        result: Ok(source),
                    })
                    .is_ok()
            });
            if let Some(error) = result {
                let _ = download_tx.send(DownloadResult {
                    request_id,
                    video_id,
                    result: Err(error),
                });
            }
        });
    }
    thread::spawn(move || {
        let cover = fetch_cover(&video_id).map(|(bytes, blur)| {
            Arc::new(PlaybackCover {
                uri: format!("bytes://cover/{video_id}"),
                bytes: bytes.into(),
                blur_uri: format!("bytes://cover-blur/{video_id}"),
                blur_bytes: blur.into(),
            })
        });
        let _ = cover_tx.send((request_id, cover));
    });
    control
}

/// Fetches the largest available thumbnail for a video.
/// `maxresdefault` is often absent, `hqdefault` falls back to a small gray
/// placeholder, hence the size check.
fn fetch_cover(video_id: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build thumbnail HTTP client")
    });

    for quality in ["maxresdefault", "hqdefault", "mqdefault"] {
        let url = format!("https://i.ytimg.com/vi/{video_id}/{quality}.jpg");
        let response = match client.get(url).send() {
            Ok(response) if response.status().is_success() => response,
            _ => continue,
        };
        let bytes = match response.bytes() {
            Ok(bytes) if bytes.len() > 2_000 => bytes,
            _ => continue,
        };
        return Some((bytes.to_vec(), blur_cover(&bytes)));
    }
    None
}

/// Downscales the cover to 64x64 PNG; the UI stretches it back up with
/// linear filtering, which reads as a blur.
fn blur_cover(bytes: &[u8]) -> Vec<u8> {
    let Ok(img) = image::load_from_memory(bytes) else {
        return bytes.to_vec();
    };
    let small = img.resize_exact(64, 64, image::imageops::FilterType::Triangle);
    let mut png = Vec::new();
    if small
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .is_err()
    {
        return bytes.to_vec();
    }
    png
}

/// Streams yt-dlp stdout into `state`; calls `ready` once enough audio is
/// buffered for immediate playback. `ready` must return whether the worker
/// accepted the stream: `sent` only suppresses the "no usable audio" error
/// when the stream was actually handed over. Returns an error string if the
/// download never became usable, or `None` if the download was cancelled.
fn stream_download(
    video_id: &str,
    control: &JobControl,
    mut ready: impl FnMut(Arc<StreamState>) -> bool,
) -> Option<String> {
    let state = Arc::new(StreamState::new());
    if control.is_cancelled() {
        return None;
    }
    let provider = match provider_config() {
        Ok(provider) => provider,
        Err(error) => {
            state.finish(Some(error.clone()));
            return Some(error);
        }
    };
    if let Some(error) = ensure_provider_warm(&provider) {
        state.finish(Some(error.clone()));
        return Some(error);
    }

    let video_url = format!("https://www.youtube.com/watch?v={video_id}");
    let mut child = match run_yt_dlp(&video_url, &provider) {
        Ok(child) => child,
        Err(error) => {
            state.finish(Some(error.clone()));
            return Some(error);
        }
    };
    let mut stdout = child.stdout.take().expect("yt-dlp stdout was piped");
    let mut stderr = child.stderr.take().expect("yt-dlp stderr was piped");
    *control.child.lock().unwrap() = Some(child);
    if control.is_cancelled() {
        kill_child(control);
        state.finish(None);
        return None;
    }

    let stderr_worker = thread::spawn(move || {
        let mut collected = String::new();
        let _ = io::Read::read_to_string(&mut stderr, &mut collected);
        collected
    });

    let mut sent = false;
    let mut chunk = [0_u8; 64 * 1024];
    let mut failure: Option<String> = None;
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                state.append(&chunk[..n]);
                if control.is_cancelled() {
                    break;
                }
                if !sent && state.inner.lock().unwrap().data.len() >= STREAM_START_THRESHOLD {
                    sent = ready(Arc::clone(&state));
                }
            }
            Err(error) => {
                failure = Some(format!("failed to read yt-dlp output: {error}"));
                break;
            }
        }
    }

    if control.is_cancelled() {
        kill_child(control);
        state.finish(None);
        return None;
    }
    let Some(mut child) = control.child.lock().unwrap().take() else {
        // cancel() raced in between the flag check and this take and already
        // killed and reaped the process.
        state.finish(None);
        return None;
    };
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let message = format!("failed to wait for yt-dlp: {error}");
            state.finish(Some(message.clone()));
            return sent.then_some(message);
        }
    };
    let stderr_text = stderr_worker.join().unwrap_or_default();

    if !status.success() {
        failure = Some(yt_dlp_failure(status, &stderr_text));
    } else if state.inner.lock().unwrap().data.is_empty() {
        failure = Some("yt-dlp returned no audio data".to_owned());
    }

    state.finish(failure.clone());
    if !sent {
        return failure.or_else(|| Some("yt-dlp produced no usable audio".to_owned()));
    }
    // Playback already started; a late failure ends the stream early and is
    // reported through StreamState.
    failure
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

fn run_yt_dlp(video_url: &str, provider: &ProviderConfig) -> Result<std::process::Child, String> {
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
        match Command::new(&executable)
            .args(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => return Ok(child),
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

/// Runs the bgutil provider warm-up on demand; returns its error if playback
/// cannot proceed. Successful warm-ups are cached forever, failures are
/// retried at most once per [`PROVIDER_WARMUP_RETRY`].
fn ensure_provider_warm(provider: &ProviderConfig) -> Option<String> {
    let mut cached = PROVIDER_WARMUP.lock().unwrap();
    if let Some((at, result)) = cached.as_ref() {
        match result {
            Ok(()) => return None,
            Err(error) if at.elapsed() < PROVIDER_WARMUP_RETRY => {
                return Some(error.clone());
            }
            Err(_) => {}
        }
    }
    let result = warm_up_provider(provider);
    *cached = Some((Instant::now(), result.clone()));
    result.err()
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

/// Buffered bytes required before playback starts (~15s of 128kbps audio).
const STREAM_START_THRESHOLD: usize = 256 * 1024;

fn yt_dlp_failure(status: std::process::ExitStatus, stderr: &str) -> String {
    let safe_detail = safe_yt_dlp_detail(stderr);

    if is_provider_failure(stderr) || safe_detail.contains("403") {
        format!(
            "yt-dlp could not obtain YouTube audio through the PO Token Provider: {safe_detail}"
        )
    } else {
        format!("yt-dlp failed with {status}: {safe_detail}")
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

fn start_session(
    state: Arc<StreamState>,
    volume: f32,
) -> Result<(PlaybackSession, Option<Duration>), String> {
    let source = StreamSource { state, pos: 0 };
    let decoder = Decoder::new(source)
        .map_err(|error| format!("failed to decode downloaded audio: {error}"))?;
    let duration = decoder.total_duration();
    let device_sink = DeviceSinkBuilder::open_default_sink()
        .map_err(|error| format!("failed to open audio output: {error}"))?;
    let player = Player::connect_new(device_sink.mixer());
    player.set_volume(volume);
    player.append(decoder);

    Ok((
        PlaybackSession {
            player,
            _device_sink: device_sink,
        },
        duration,
    ))
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

/// Resets playback state while keeping the user's volume.
fn reset_snapshot(snapshot: &Arc<RwLock<PlaybackSnapshot>>) {
    mutate_snapshot(snapshot, |snapshot| {
        snapshot.current_track = None;
        snapshot.status = PlaybackStatus::Idle;
        snapshot.position = Duration::ZERO;
        snapshot.duration = None;
        snapshot.cover = None;
        snapshot.shuffle = false;
        snapshot.queue = Arc::new(Vec::new());
        snapshot.queue_index = 0;
    });
}

fn volume_storage_path() -> Option<PathBuf> {
    let dir = dirs::config_dir()?.join("ytm");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("player-volume"))
}

fn stored_volume() -> f32 {
    volume_storage_path()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|text| text.trim().parse::<f32>().ok())
        .map(|volume| volume.clamp(0.0, 1.0))
        .unwrap_or(1.0)
}

fn store_volume(volume: f32) {
    if let Some(path) = volume_storage_path() {
        let _ = fs::write(path, format!("{volume}"));
    }
}

fn mutate_snapshot(
    snapshot: &Arc<RwLock<PlaybackSnapshot>>,
    mutate: impl FnOnce(&mut PlaybackSnapshot),
) {
    match snapshot.write() {
        Ok(mut guard) => mutate(&mut guard),
        Err(poisoned) => mutate(&mut poisoned.into_inner()),
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
    use super::{
        JobControl, is_provider_failure, redact_urls, safe_yt_dlp_detail, yt_dlp_candidates,
    };
    use std::path::Path;

    #[test]
    fn cancel_without_child_only_sets_flag() {
        let control = JobControl::new();

        assert!(!control.is_cancelled());
        control.cancel();
        assert!(control.is_cancelled());
    }

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
