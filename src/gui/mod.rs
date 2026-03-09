mod auth;
mod header;

use anyhow::{Error, Result, anyhow};
use rust_i18n::t;
use std::env;

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{
    Align, Align2, Area, CentralPanel, Context, Frame, Id, Layout, TextEdit, Vec2, ViewportBuilder,
    vec2,
};
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

#[derive(PartialEq, Eq)]
enum ApplicationPage {
    Main,
    Library,
}

pub struct Application {
    auth: ApplicationAuth,
    page: ApplicationPage,
}

impl Application {
    fn new(ctx: &Context) -> Self {
        ctx.set_zoom_factor(1.5);

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
            page: ApplicationPage::Main,
        }
    }
}

impl App for Application {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.plugin_or_default::<egui_async::EguiAsyncPlugin>();
        CentralPanel::default().show(ctx, |ui| {
            if self.auth.yt_client.is_none() {
                // process_auth if no ytclient is provided
                if self.auth.client_id.is_some() && self.auth.client_secret.is_some() {
                    self.process_auth(ui);
                } else {
                    // client_id or client_secret is not provided
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
                                                Some(self.auth.client_secret_input.clone());
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

            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                self.draw_header(ui);
            });
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
            .with_inner_size(vec2(1200.0, 800.0))
            .with_min_inner_size(vec2(800.0, 600.0)),

        ..Default::default()
    };

    match eframe::run_native(
        "Youtube Music",
        options,
        Box::new(|cc| {
            egui_material_icons::initialize(&cc.egui_ctx);

            Ok(Box::new(Application::new(&cc.egui_ctx)))
        }),
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            log::error!("{e}");
            Err(anyhow!(format!("{e}")))
        }
    }
}
