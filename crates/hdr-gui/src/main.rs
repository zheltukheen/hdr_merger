//! Window shell: one top bar, one left rail, one canvas, one filmstrip.
//!
//! Everything visual lives in `theme`; everything stateful lives in `app`;
//! panels and canvas only read and write `App`.

mod app;
mod canvas;
mod meta;
mod panels;
mod theme;

use eframe::egui;
use egui::RichText;

use app::{App, StatusLevel, Tool};
use theme::color;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 880.0])
            .with_min_inner_size([1040.0, 640.0])
            .with_title("HDR Merger"),
        ..Default::default()
    };
    eframe::run_native(
        "HDR Merger",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl App {
    /// The whole window. Kept free of `eframe::Frame` so it can be driven from
    /// a headless `egui::Context` in tests.
    pub fn ui(&mut self, ctx: &egui::Context) {
        self.take_dropped(ctx);
        self.poll(ctx);
        self.ensure_texture(ctx);
        self.shortcuts(ctx);

        top_bar(self, ctx);

        if !self.frames.is_empty() {
            egui::TopBottomPanel::bottom("filmstrip")
                .exact_height(62.0)
                .frame(
                    egui::Frame::none()
                        .fill(color::PANEL)
                        .stroke(egui::Stroke::new(1.0_f32, color::STROKE))
                        .inner_margin(egui::Margin::symmetric(theme::SP_M, theme::SP_S)),
                )
                .show(ctx, |ui| canvas::filmstrip(self, ui));
        }

        egui::SidePanel::left("rail")
            .resizable(true)
            .default_width(theme::PANEL_W)
            .width_range(288.0..=440.0)
            .frame(
                egui::Frame::none()
                    .fill(color::PANEL)
                    .stroke(egui::Stroke::new(1.0_f32, color::STROKE))
                    .inner_margin(egui::Margin::symmetric(theme::SP_M, theme::SP_S)),
            )
            .show(ctx, |ui| panels::left_panel(self, ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(color::BG))
            .show(ctx, |ui| canvas::central(self, ui));
    }
}

fn top_bar(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("topbar")
        .exact_height(46.0)
        .frame(
            egui::Frame::none()
                .fill(color::PANEL)
                .stroke(egui::Stroke::new(1.0_f32, color::STROKE))
                .inner_margin(egui::Margin::symmetric(theme::SP_L, theme::SP_S)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("HDR Merger").strong().size(14.0));
                ui.add_space(theme::SP_S);
                theme::badge(ui, env!("CARGO_PKG_VERSION"), color::TEXT_DIM);
                ui.add_space(theme::SP_M);

                if !app.frames.is_empty() {
                    theme::badge(ui, &format!("{} frames", app.frames.len()), color::TEXT_DIM);
                }
                if app.masked_frames() > 0 {
                    theme::badge(
                        ui,
                        &format!("{} masked", app.masked_frames()),
                        color::ACCENT,
                    );
                }

                ui.add_space(theme::SP_M);
                let status_color = match app.status_level {
                    StatusLevel::Info => color::TEXT_DIM,
                    StatusLevel::Busy => color::ACCENT,
                    StatusLevel::Ok => color::OK,
                    StatusLevel::Error => color::ERROR,
                };
                ui.label(RichText::new(&app.status).size(12.0).color(status_color));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if app.busy() {
                        let (done, total) = app.progress;
                        let fraction = if total > 0 {
                            done as f32 / total as f32
                        } else {
                            0.0
                        };
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(170.0)
                                .text(app.job_label()),
                        );
                    } else if app.preview.is_some()
                        && theme::ghost_button(ui, "Re-preview").clicked()
                    {
                        app.start_preview();
                    }
                });
            });
        });
}

impl App {
    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (keys, modifiers) = ctx.input(|i| (i.keys_down.clone(), i.modifiers));
        let pressed = |key: egui::Key| keys.contains(&key);
        if pressed(egui::Key::B) {
            self.tool = Tool::Brush;
        }
        if pressed(egui::Key::E) {
            self.tool = Tool::Eraser;
        }
        if pressed(egui::Key::I) {
            self.tool = Tool::Inspect;
        }
        if pressed(egui::Key::OpenBracket) {
            self.brush = (self.brush - 4.0).max(2.0);
        }
        if pressed(egui::Key::CloseBracket) {
            self.brush = (self.brush + 4.0).min(300.0);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
            self.view_fit = true;
            self.view_zoom = 1.0;
            self.view_offset = egui::Vec2::ZERO;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
            self.view_fit = false;
            self.view_zoom = 1.0;
        }
        if modifiers.command && ctx.input(|i| i.key_pressed(egui::Key::E)) {
            self.start_export();
        }
        if modifiers.command && ctx.input(|i| i.key_pressed(egui::Key::S)) {
            self.save_recipe();
        }
        if modifiers.command && ctx.input(|i| i.key_pressed(egui::Key::O)) {
            self.load_recipe();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && self.preview.is_none() {
            self.start_preview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        ctx
    }

    fn render(ctx: &egui::Context, app: &mut App) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.ui(ctx));
    }

    fn sample_tiff(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let mut image = hdr_core::LinearImage::filled(32, 24, [0.3, 0.35, 0.4]);
        image.set_pixel(4, 4, [1.0, 1.0, 1.0]);
        let path = dir.join(name);
        let linear = hdr_core::ToneMapOptions {
            exposure_ev: 0.0,
            curve: hdr_core::Curve::None,
            srgb: false,
        };
        hdr_core::io::save(&path, &image, &linear).expect("write fixture");
        path
    }

    #[test]
    fn empty_state_renders_without_frames() {
        let ctx = test_ctx();
        let mut app = App::with_context(&ctx);
        render(&ctx, &mut app);
        render(&ctx, &mut app);
        assert!(app.frames.is_empty());
        assert_eq!(app.included_count(), 0);
    }

    #[test]
    fn adding_frames_renders_the_shell_with_canvas_and_filmstrip() {
        let dir = std::env::temp_dir().join("hdr-merger-gui-add");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = sample_tiff(&dir, "a.tif");

        let ctx = test_ctx();
        let mut app = App::with_context(&ctx);
        app.add_paths(vec![path.clone()]);
        assert_eq!(app.frames.len(), 1);
        assert_eq!(app.frames[0].meta.width, 32);

        render(&ctx, &mut app);
        assert!(app.status_level != StatusLevel::Error, "{}", app.status);
    }

    #[test]
    fn mask_is_allocated_lazily_and_cleared() {
        let dir = std::env::temp_dir().join("hdr-merger-gui-mask");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = sample_tiff(&dir, "b.tif");

        let ctx = test_ctx();
        let mut app = App::with_context(&ctx);
        app.add_paths(vec![path]);
        assert!(
            app.frames[0].mask.is_none(),
            "mask must not be allocated up front"
        );

        {
            let mask = app.ensure_mask(0).expect("mask");
            mask.paint_circle(8.0, 8.0, 4.0, 0.0, 255);
        }
        assert!(app.frames[0].masked());
        assert_eq!(app.masked_frames(), 1);

        app.clear_mask(0);
        assert!(!app.frames[0].masked());
    }

    #[test]
    fn histogram_covers_every_pixel() {
        let pixels = vec![10u8, 20, 30, 200, 210, 220];
        let bins = app::histogram(&pixels);
        assert_eq!(bins.len(), 64);
        assert_eq!(bins.iter().sum::<u32>(), 2);
        let occupied = bins.iter().filter(|count| **count > 0).count();
        assert_eq!(
            occupied, 2,
            "dark and bright pixels must land in separate bins"
        );
        assert!(
            bins[4] + bins[5] + bins[6] > 0,
            "dark pixel missing: {bins:?}"
        );
    }
}
