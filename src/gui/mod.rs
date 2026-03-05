mod auth;

use anyhow::{Error, Result, anyhow};
use rust_i18n::t;

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{CentralPanel, Context, Vec2, ViewportBuilder};
use egui_async::Bind;

use ytmapi_rs::{YtMusic, auth::OAuthToken};

use auth::AuthState;

struct ApplicationAuth {
    current_state: Bind<AuthState, Error>,
    previous_state: Option<AuthState>,

    yt_client: Option<YtMusic<OAuthToken>>,
}

pub struct Application {
    auth: ApplicationAuth,
}

impl Application {
    fn new(_ctx: &Context) -> Self {
        Self {
            auth: ApplicationAuth {
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
            self.process_auth(ctx);
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
