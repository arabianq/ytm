use super::{
    Application, AsyncView,
    data::{clone_async_state, download_thumbnail, preferred_thumbnail_url},
};
use egui::{Color32, Direction, Frame, Image, Layout, RichText, Sense, Stroke, Ui, Vec2};
use egui_async::{Bind, StateWithData};
use ytmapi_rs::common::Thumbnail;

impl Application {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_media_card(
        &mut self,
        ui: &mut Ui,
        id_source: impl std::hash::Hash,
        thumbnails: &[Thumbnail],
        title: &str,
        subtitle: &str,
        selected: bool,
        image_size: Vec2,
    ) -> egui::Response {
        let card_id = ui.make_persistent_id(&id_source);
        let fill = if selected {
            Color32::from_rgb(28, 48, 72)
        } else {
            Color32::from_gray(24)
        };
        let stroke = if selected {
            Stroke::new(1.0_f32, Color32::from_rgb(104, 178, 255))
        } else {
            Stroke::new(1.0_f32, Color32::from_gray(56))
        };

        let frame = ui
            .push_id(card_id.with("scope"), |ui| {
                Frame::group(ui.style())
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(12.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(image_size.x + 16.0);
                        ui.vertical(|ui| {
                            self.show_thumbnail(ui, thumbnails, image_size);
                            ui.add_space(8.0);
                            ui.label(RichText::new(title).strong());
                            if !subtitle.is_empty() {
                                ui.small(subtitle);
                            }
                        });
                    })
            })
            .inner;

        frame.response.interact(Sense::click())
    }

    pub(super) fn show_thumbnail(&mut self, ui: &mut Ui, thumbnails: &[Thumbnail], size: Vec2) {
        let Some(url) = preferred_thumbnail_url(thumbnails).map(ToOwned::to_owned) else {
            show_thumbnail_placeholder(ui, size, "No image");
            return;
        };

        let state = self
            .thumbnail_state
            .entry(url.clone())
            .or_insert_with(|| Bind::new(true));
        if matches!(state.state(), StateWithData::Idle) {
            state.request(download_thumbnail(url.clone()));
        }

        match clone_async_state(state.state()) {
            AsyncView::Idle | AsyncView::Pending => {
                ui.ctx().request_repaint();
                show_thumbnail_placeholder(ui, size, "Loading");
            }
            AsyncView::Finished(bytes) => {
                ui.allocate_ui_with_layout(
                    size,
                    Layout::centered_and_justified(Direction::TopDown),
                    |ui| {
                        ui.add(
                            Image::from_bytes(format!("bytes://thumbnail/{url}"), bytes)
                                .fit_to_exact_size(size)
                                .corner_radius(10),
                        );
                    },
                );
            }
            AsyncView::Failed(error) => {
                log::warn!("Thumbnail load failed for {url}: {error}");
                show_thumbnail_placeholder(ui, size, "Image error");
            }
        }
    }
}

fn show_thumbnail_placeholder(ui: &mut Ui, size: Vec2, label: &str) {
    Frame::group(ui.style())
        .fill(Color32::from_gray(40))
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_min_size(size);
            ui.centered_and_justified(|ui| ui.small(label));
        });
}
