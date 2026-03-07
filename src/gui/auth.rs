use crate::gui::Application;
use crate::misc;

use egui::{Align2, Area, Color32, Frame, Id, RichText, Ui, Vec2};
use egui_async::StateWithData;

use anyhow::{Result, anyhow};
use derivative::Derivative;
use rust_i18n::t;
use std::time::Duration;
use tokio::{fs, time::sleep};

use ytmapi_rs::{
    Client, YtMusic,
    auth::{OAuthToken, oauth::OAuthDeviceCode},
};

#[derive(Derivative, Clone)]
#[derivative(Debug)]
pub enum AuthState {
    Required {
        client: Client,
        #[derivative(Debug = "ignore")]
        code: OAuthDeviceCode,
        url: String,
    },
    LoggedIn(YtMusic<OAuthToken>),
}

async fn begin_auth(client_id: String) -> Result<AuthState> {
    let config_path = misc::get_config_path().await?;
    let token_path = config_path.join("token.json");

    // Attemp to read saved token.json if exists
    if fs::try_exists(&token_path).await.unwrap_or(false) {
        match fs::read(&token_path).await {
            Ok(file_content) => {
                if let Ok(saved_token) = serde_json::from_slice::<OAuthToken>(&file_content) {
                    let yt = YtMusic::from_auth_token(saved_token);
                    return Ok(AuthState::LoggedIn(yt));
                } else {
                    fs::remove_file(&token_path).await?;
                }
            }
            Err(e) => {
                log::error!("Failed to read {path}: {e}", path = token_path.display());
            }
        };
    }

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

    let config_path = misc::get_config_path().await?;
    let token_path = config_path.join("token.json");

    let saved_token_json = serde_json::to_vec(&token)?;
    match fs::write(&token_path, &saved_token_json).await {
        Ok(_) => {
            log::info!(
                "Successfully saved token to {path}",
                path = token_path.display()
            );
        }
        Err(e) => {
            log::error!("Failed to save token path: {e}");
        }
    }

    let yt = YtMusic::from_auth_token(token);

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
                                        let user_code = url
                                            .split("user_code=")
                                            .nth(1)
                                            .unwrap_or("UNKNOWN")
                                            .to_string();

                                        ui.heading(t!("auth.required_title"));
                                        ui.add_space(10.0);
                                        ui.label(t!("auth.required_instruction"));

                                        ui.add_space(10.0);
                                        if ui
                                            .button(RichText::new(&user_code).heading().strong())
                                            .clicked()
                                        {
                                            ui.ctx().copy_text(user_code.clone());
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
                        .request(begin_auth(self.auth.client_id.clone().unwrap()));
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
