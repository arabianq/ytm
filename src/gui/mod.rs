use anyhow::{Result, anyhow};
use rust_i18n::t;

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{CentralPanel, Context, Vec2, ViewportBuilder, RichText};
use std::sync::mpsc::{Receiver, channel};
use ytmapi_rs::{auth::OAuthToken, YtMusic};

use crate::auth::{self, AuthEvent};

/// Состояние интерфейса
enum AppState {
    Startup,
    Checking,
    AuthRequired { user_code: String, url: String },
    LoggedIn(YtMusic<OAuthToken>),
    Error(String),
}

pub struct Application {
    state: AppState,
    auth_rx: Receiver<AuthEvent>,
}

impl Application {
    fn new(ctx: &Context) -> Self {
        let (tx, rx) = channel();
        auth::start_auth_flow(tx);
        ctx.request_repaint();
        Self {
            state: AppState::Startup,
            auth_rx: rx,
        }
    }
}

impl App for Application {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(event) = self.auth_rx.try_recv() {
            match event { 
                AuthEvent::Checking => self.state = AppState::Checking,
                AuthEvent::CodeRequired { user_code, verification_url } => {
                    self.state = AppState::AuthRequired { user_code, url: verification_url };
                },
                AuthEvent::Authenticated(yt) => {
                    self.state = AppState::LoggedIn(yt);
                },
                AuthEvent::Error(msg) => self.state = AppState::Error(msg),
            }
        }

        CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                match &self.state {
                    AppState::Startup | AppState::Checking => {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.label(t!("auth.checking"));
                        });
                    },
                    AppState::AuthRequired { user_code, url } => {
                        ui.vertical_centered(|ui| {
                            ui.heading(t!("auth.required_title"));
                            ui.add_space(10.0);
                            ui.label(t!("auth.required_instruction"));
                            
                            ui.add_space(10.0);
                            if ui.button(RichText::new(user_code).heading().strong()).clicked() {
                                ui.ctx().copy_text(user_code.clone());
                            }
                            ui.small(t!("auth.copy_prompt"));

                            ui.add_space(20.0);
                            ui.hyperlink(url);
                            
                            ui.add_space(20.0);
                            ui.spinner();
                            ui.label(t!("auth.waiting"));
                        });
                    },
                    AppState::LoggedIn(_yt) => {
                        ui.heading(t!("auth.success_title"));
                        ui.label(t!("auth.welcome"));
                    },
                    AppState::Error(e) => {
                        ui.colored_label(egui::Color32::RED, format!("{}{}", t!("auth.error_prefix"), e));
                    }
                }
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
