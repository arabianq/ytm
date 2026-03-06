mod auth;

use anyhow::{Error, Result, anyhow};
use rust_i18n::t;
use std::env;

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{CentralPanel, Context, TextEdit, Vec2, ViewportBuilder};
use egui_async::Bind;

use ytmapi_rs::{YtMusic, auth::OAuthToken};

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

pub struct Application {
    auth: ApplicationAuth,
}

impl Application {
    fn new(_ctx: &Context) -> Self {
        let client_id = env::var("CLIENT_ID")
            .map(|s| Some(s.trim().to_string()))
            .map_err(|_| anyhow!(t!("config.client_id_not_found")))
            .unwrap_or(None);

        let client_secret = env::var("CLIENT_SECRET")
            .map(|s| Some(s.trim().to_string()))
            .map_err(|_| anyhow!(t!("config.client_secret_not_found")))
            .unwrap_or(None);

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
        }
    }
}

impl App for Application {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.plugin_or_default::<egui_async::EguiAsyncPlugin>();

        // process_auth if no ytclient is provided
        if self.auth.yt_client.is_none() {
            if self.auth.client_id.is_some() && self.auth.client_secret.is_some() {
                self.process_auth(ctx);
            } else {
                CentralPanel::default().show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("Client ID:");
                        ui.add(TextEdit::singleline(&mut self.auth.client_id_input));

                        ui.label("Client Secret:");
                        ui.add(TextEdit::singleline(&mut self.auth.client_secret_input));

                        if ui.button(t!("auth.retry_button")).clicked() {
                            self.auth.client_id = Some(self.auth.client_id_input.clone());
                            self.auth.client_id_input = String::new();

                            self.auth.client_secret = Some(self.auth.client_secret_input.clone());
                            self.auth.client_secret_input = String::new();
                        }
                    });
                });
            }

            ctx.request_repaint_after_secs(0.1);
            return;
        }

        CentralPanel::default().show(ctx, |ui| {
            ui.heading(t!("auth.success_title"));
            ui.label(t!("auth.welcome"));
        });
    }
}

pub fn run() -> Result<()> {
    let options = NativeOptions {
        vsync: true,
        centered: true,
        hardware_acceleration: HardwareAcceleration::Preferred,

        viewport: ViewportBuilder::default()
            .with_app_id("ytm")
            .with_inner_size(Vec2::new(1200.0, 800.0))
            .with_min_inner_size(Vec2::new(800.0, 600.0)),

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
