use crate::gui::{Application, ApplicationPage};

use egui::{
    Align, Align2, Area, Button, Color32, FontFamily, Id, Layout, RichText, Stroke, Ui, Vec2,
};
use egui_material_icons::icons::*;
use rust_i18n::t;

impl Application {
    pub fn draw_header(&mut self, ui: &mut Ui) {
        Area::new(Id::new("header"))
            .anchor(Align2::CENTER_TOP, Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let main_page_button = Button::new(RichText::new(format!(
                        "{} {}",
                        ICON_HOME,
                        t!("app.main_page_title")
                    )))
                    .stroke(Stroke::new(0.0, Color32::TRANSPARENT))
                    .frame(self.page == ApplicationPage::Main)
                    .corner_radius(12.0);

                    let library_page_button = Button::new(RichText::new(format!(
                        "{} {}",
                        ICON_LIBRARY_MUSIC,
                        t!("app.library_page_title")
                    )))
                    .stroke(Stroke::new(0.0, Color32::WHITE))
                    .frame(self.page == ApplicationPage::Library)
                    .corner_radius(12.0);

                    if ui.add(main_page_button).clicked() {
                        self.page = ApplicationPage::Main;

                        if let Some(_yt) = &self.auth.yt_client {}
                    }
                    if ui.add(library_page_button).clicked() {
                        self.page = ApplicationPage::Library;

                        if let Some(yt) = &self.auth.yt_client {}
                    }
                });
            });

        ui.add_space(16.0);
        ui.separator();
    }
}
