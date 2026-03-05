use anyhow::{Result, anyhow};
use rust_i18n::t;

use eframe::{App, HardwareAcceleration, NativeOptions};
use egui::{CentralPanel, Context, Vec2, ViewportBuilder};

pub struct Application {}

impl Application {
    fn new(_ctx: &Context) -> Self {
        Self {}
    }
}

impl App for Application {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| ui.label(t!("internal.todo")));
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
