### TODO

# Authorization
- [x] Basic Google OAuth support
- [ ] Token renewal (is it handled by ytmapi-rs?)

# UI
- [ ] Basic application layout
- [ ] Basic search
- [ ] User's library with playlists and tracks
- [ ] Main page with playlists and tracks
      
# Other
- [x] Audio playback (yt-dlp + PO Token Provider + [rodio](https://crates.io/crates/rodio))

# Building

Requirements: Rust (stable), `yt-dlp` on `PATH` (or set `YTM_YT_DLP`), Node.js (or set `YTM_NODE`), and the [bgutil-ytdlp-pot-provider](https://github.com/Brainicism/bgutil-ytdlp-pot-provider) with the matching yt-dlp plugin installed.

```powershell
git clone https://github.com/arabianq/ytm.git
cd ytm
git checkout feature/playback
cargo build
cargo run
```

Playback looks for the bgutil provider at `%USERPROFILE%\bgutil-ytdlp-pot-provider` (it expects `server\build\generate_once.js` inside). Override with `YTM_PO_PROVIDER_HOME` if it lives elsewhere.

Build the provider once:

```powershell
cd bgutil-ytdlp-pot-provider\server
npm ci
npm run build
```

Install the plugin into `%APPDATA%\yt-dlp\plugins\` (see the provider README). Verify with `yt-dlp --verbose` — it must list `PO Token Providers: bgutil:...`.

# Testing

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual playback test: run the app, click any track in Library Songs, a playlist, or artist top tracks, and use the playback panel at the bottom (Play/Pause, Stop).

# Known issues

- rodio/CPAL audio output on Windows is fragile with some device configurations; if playback silently produces no sound, try another output device.
- The whole track is downloaded to memory before playback starts; the first failure of the provider warm-up is cached until the app is restarted.
