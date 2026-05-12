use crate::gui::Application;
use crate::misc;

use egui::{Align2, Area, Color32, Frame, Id, RichText, Ui, Vec2};
use egui_async::StateWithData;

use anyhow::{Result, anyhow};
use derivative::Derivative;
use rust_i18n::t;
use serde_json::Value;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::{fs, time::sleep};

use ytmapi_rs::{
    Client, YtMusic,
    auth::{
        AuthToken, BrowserToken, LoggedIn, OAuthToken, RawResult, oauth::OAuthDeviceCode,
    },
    parse::ProcessedResult,
};

const STABLE_CLIENT_VERSION: &str = "1.20250416.01.00";

#[derive(Clone, Debug, Default)]
pub struct AuthBootstrap {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub cookie: Option<String>,
    pub cookie_file: Option<String>,
}

impl AuthBootstrap {
    pub fn has_any_auth_source(&self) -> bool {
        self.cookie
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || self
                .cookie_file
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            || (self.client_id.is_some() && self.client_secret.is_some())
    }
}

#[derive(Clone, Debug)]
pub struct StableOAuthToken {
    inner: OAuthToken,
}

impl StableOAuthToken {
    pub fn from_oauth_token(inner: OAuthToken) -> Self {
        Self { inner }
    }
}

impl AuthToken for StableOAuthToken {
    fn headers(&self) -> ytmapi_rs::error::Result<impl IntoIterator<Item = (&str, Cow<'_, str>)>> {
        self.inner.headers()
    }

    fn client_version(&self) -> Cow<'_, str> {
        STABLE_CLIENT_VERSION.into()
    }

    fn deserialize_response<'a, Q>(
        raw: RawResult<'a, Q, Self>,
    ) -> ytmapi_rs::error::Result<ProcessedResult<'a, Q>> {
        ProcessedResult::try_from(raw)
    }
}

impl LoggedIn for StableOAuthToken {}

#[derive(Clone, Debug)]
pub enum AppAuthToken {
    OAuth(StableOAuthToken),
    Browser(BrowserToken),
}

impl AppAuthToken {
    fn from_oauth_token(token: OAuthToken) -> Self {
        Self::OAuth(StableOAuthToken::from_oauth_token(token))
    }

    async fn from_cookie_str(cookie: &str) -> Result<Self> {
        let normalized = normalize_cookie_input(cookie)?;
        let client = Client::new()?;
        let token = BrowserToken::from_str(&normalized, &client).await?;
        Ok(Self::Browser(token))
    }

    async fn from_cookie_file(path: impl AsRef<Path>) -> Result<Self> {
        let cookie_file = path.as_ref();
        let raw = fs::read_to_string(cookie_file).await?;
        Self::from_cookie_str(&raw).await
    }
}

impl AuthToken for AppAuthToken {
    fn headers(&self) -> ytmapi_rs::error::Result<impl IntoIterator<Item = (&str, Cow<'_, str>)>> {
        let headers = match self {
            Self::OAuth(token) => token.headers()?.into_iter().collect::<Vec<_>>(),
            Self::Browser(token) => token.headers()?.into_iter().collect::<Vec<_>>(),
        };
        Ok(headers)
    }

    fn client_version(&self) -> Cow<'_, str> {
        match self {
            Self::OAuth(token) => token.client_version(),
            Self::Browser(token) => token.client_version(),
        }
    }

    fn deserialize_response<'a, Q>(
        raw: RawResult<'a, Q, Self>,
    ) -> ytmapi_rs::error::Result<ProcessedResult<'a, Q>> {
        ProcessedResult::try_from(raw)
    }
}

impl LoggedIn for AppAuthToken {}

#[derive(Derivative, Clone)]
#[derivative(Debug)]
pub enum AuthState {
    Required {
        client: Client,
        #[derivative(Debug = "ignore")]
        code: OAuthDeviceCode,
        url: String,
    },
    LoggedIn(YtMusic<AppAuthToken>),
}

async fn save_token(token: &OAuthToken) -> Result<()> {
    let config_path = misc::get_config_path().await?;
    let token_path = config_path.join("token.json");
    let saved_token_json = serde_json::to_vec(token)?;

    fs::write(&token_path, &saved_token_json).await?;
    log::info!(
        "Successfully saved token to {path}",
        path = token_path.display()
    );

    Ok(())
}

fn config_cookie_candidates(config_path: &Path) -> [PathBuf; 2] {
    [config_path.join("cookie.txt"), config_path.join("cookies.txt")]
}

fn normalize_cookie_input(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("Cookie input is empty"));
    }

    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(json) = serde_json::from_str::<Value>(trimmed) {
            if let Some(header) = cookie_header_from_json(&json) {
                return Ok(header);
            }
        }
    }

    if trimmed.contains('\t') || trimmed.contains("# Netscape") {
        let pairs = trimmed
            .lines()
            .filter_map(parse_netscape_cookie_line)
            .collect::<Vec<_>>();
        if !pairs.is_empty() {
            return Ok(pairs.join("; "));
        }
    }

    if trimmed.contains('\n') {
        let pairs = trimmed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.trim_end_matches(';'))
            .collect::<Vec<_>>();
        if !pairs.is_empty() {
            return Ok(pairs.join("; "));
        }
    }

    Ok(trimmed.to_string())
}

fn cookie_header_from_json(json: &Value) -> Option<String> {
    let cookies = match json {
        Value::Array(items) => items,
        Value::Object(map) => map.get("cookies")?.as_array()?,
        _ => return None,
    };

    let pairs = cookies
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.trim();
            let value = item.get("value")?.as_str()?.trim();
            if name.is_empty() {
                None
            } else {
                Some(format!("{name}={value}"))
            }
        })
        .collect::<Vec<_>>();

    if pairs.is_empty() {
        None
    } else {
        Some(pairs.join("; "))
    }
}

fn parse_netscape_cookie_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() < 7 {
        return None;
    }

    let name = parts[5].trim();
    let value = parts[6].trim();
    if name.is_empty() {
        None
    } else {
        Some(format!("{name}={value}"))
    }
}

async fn try_cookie_auth(config: &AuthBootstrap, config_path: &Path) -> Result<Option<AppAuthToken>> {
    let mut errors = Vec::new();

    if let Some(cookie) = config.cookie.as_deref().filter(|value| !value.trim().is_empty()) {
        match AppAuthToken::from_cookie_str(cookie).await {
            Ok(token) => return Ok(Some(token)),
            Err(error) => errors.push(format!("cookie text: {error}")),
        }
    }

    if let Some(cookie_file) = config
        .cookie_file
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match AppAuthToken::from_cookie_file(cookie_file).await {
            Ok(token) => return Ok(Some(token)),
            Err(error) => errors.push(format!("cookie file {cookie_file}: {error}")),
        }
    }

    for cookie_file in config_cookie_candidates(config_path) {
        if !fs::try_exists(&cookie_file).await.unwrap_or(false) {
            continue;
        }

        match AppAuthToken::from_cookie_file(&cookie_file).await {
            Ok(token) => return Ok(Some(token)),
            Err(error) => errors.push(format!("cookie file {}: {error}", cookie_file.display())),
        }
    }

    if errors.is_empty() {
        Ok(None)
    } else if config.client_id.is_some() && config.client_secret.is_some() {
        log::warn!(
            "Cookie auth failed, falling back to OAuth. Errors: {}",
            errors.join(" | ")
        );
        Ok(None)
    } else {
        Err(anyhow!("Cookie auth failed: {}", errors.join(" | ")))
    }
}

async fn begin_auth(config: AuthBootstrap) -> Result<AuthState> {
    let config_path = misc::get_config_path().await?;
    let token_path = config_path.join("token.json");

    if let Some(token) = try_cookie_auth(&config, &config_path).await? {
        return Ok(AuthState::LoggedIn(YtMusic::from_auth_token(token)));
    }

    // Attemp to read saved token.json if exists
    if fs::try_exists(&token_path).await.unwrap_or(false) {
        match fs::read(&token_path).await {
            Ok(file_content) => {
                if let Ok(saved_token) = serde_json::from_slice::<OAuthToken>(&file_content) {
                    let mut yt = YtMusic::from_auth_token(saved_token);
                    match yt.refresh_token().await {
                        Ok(refreshed_token) => {
                            if let Err(e) = save_token(&refreshed_token).await {
                                log::error!("Failed to save refreshed token: {e}");
                            }
                            return Ok(AuthState::LoggedIn(YtMusic::from_auth_token(
                                AppAuthToken::from_oauth_token(refreshed_token),
                            )));
                        }
                        Err(e) => {
                            log::warn!("Saved token refresh failed, removing old token: {e}");
                            fs::remove_file(&token_path).await?;
                        }
                    }
                } else {
                    fs::remove_file(&token_path).await?;
                }
            }
            Err(e) => {
                log::error!("Failed to read {path}: {e}", path = token_path.display());
            }
        };
    }

    let client_id = config
        .client_id
        .ok_or_else(|| anyhow!("Missing CLIENT_ID for OAuth login"))?;
    let client = Client::new()?;

    log::info!("Starting OAuth flow with CLIENT_ID and CLIENT_SECRET");

    match ytmapi_rs::generate_oauth_code_and_url(&client, client_id).await {
        Ok((code, url)) => Ok(AuthState::Required { client, code, url }),
        Err(e) => Err(anyhow!(e)),
    }
}

async fn finish_auth(
    client: Client,
    client_id: String,
    client_secret: String,
    code: OAuthDeviceCode,
) -> Result<AuthState> {
    let token = loop {
        match ytmapi_rs::generate_oauth_token(&client, code.clone(), &client_id, &client_secret)
            .await
        {
            Ok(t) => break t,
            Err(e) => {
                let err_msg = format!("{e}");
                if err_msg.contains("authorization_pending") {
                    sleep(Duration::from_secs(5)).await;
                } else {
                    return Err(anyhow!(t!("auth.auth_error", error = err_msg)));
                }
            }
        }
    };

    if let Err(e) = save_token(&token).await {
        log::error!("Failed to save token path: {e}");
    }

    let yt = YtMusic::from_auth_token(AppAuthToken::from_oauth_token(token));

    Ok(AuthState::LoggedIn(yt))
}

impl Application {
    pub fn process_auth(&mut self, ui: &mut Ui) {
        match self.auth.current_state.state() {
            StateWithData::Pending => match &self.auth.previous_state {
                None => {
                    Area::new(Id::new("auth_checking"))
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(ui.ctx(), |ui| {
                            ui.vertical_centered(|ui| {
                                ui.spinner();
                                ui.label(t!("auth.checking"))
                            });
                        });
                }
                Some(AuthState::Required {
                    client: _,
                    code: _,
                    url,
                }) => {
                    Area::new(Id::new("auth_processing"))
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(ui.ctx(), |ui| {
                            Frame::group(ui.style())
                                .corner_radius(8.0)
                                .inner_margin(16.0)
                                .show(ui, |ui| {
                                    ui.vertical_centered(|ui| {
                                        let user_code =
                                            url.split("user_code=").nth(1).unwrap_or("UNKNOWN");

                                        ui.heading(t!("auth.required_title"));
                                        ui.add_space(10.0);
                                        ui.label(t!("auth.required_instruction"));

                                        ui.add_space(10.0);
                                        if ui
                                            .button(RichText::new(user_code).heading().strong())
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(user_code.to_string());
                                        }
                                        ui.small(t!("auth.copy_prompt"));

                                        ui.add_space(20.0);
                                        ui.hyperlink(url);

                                        ui.add_space(20.0);
                                        ui.spinner();
                                        ui.label(t!("auth.waiting"));
                                    });
                                });
                        });
                }
                Some(AuthState::LoggedIn(_)) => {
                    unreachable!("UNREACHABLE");
                }
            },
            StateWithData::Idle => match &self.auth.previous_state {
                None => {
                    self.auth
                        .current_state
                        .request(begin_auth(self.auth.bootstrap()));
                }
                Some(AuthState::Required {
                    client,
                    code,
                    url: _,
                }) => {
                    self.auth.current_state.request(finish_auth(
                        client.clone(),
                        self.auth.client_id.clone().unwrap(),
                        self.auth.client_secret.clone().unwrap(),
                        code.clone(),
                    ));
                }
                Some(AuthState::LoggedIn(yt)) => {
                    self.auth.yt_client = Some(yt.clone());
                    self.auth.current_state.clear();
                    self.auth.previous_state.take();
                }
            },
            StateWithData::Finished(state) => match state {
                AuthState::Required { client, code, url } => {
                    self.auth.previous_state = Some(AuthState::Required {
                        client: client.clone(),
                        code: code.clone(),
                        url: url.clone(),
                    });
                    self.auth.current_state.clear();
                }
                AuthState::LoggedIn(yt) => {
                    self.auth.previous_state = Some(AuthState::LoggedIn(yt.clone()));
                    self.auth.current_state.clear();
                    ui.heading(t!("auth.success_title"));
                    ui.label(t!("auth.welcome"));
                }
            },
            StateWithData::Failed(e) => {
                ui.colored_label(Color32::RED, format!("{}{}", t!("auth.error_prefix"), e));
            }
        }
    }
}
