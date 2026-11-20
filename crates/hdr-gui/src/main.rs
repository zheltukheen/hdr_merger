//! Desktop UI. The window is a thin shell around `hdr-core`: it owns the frame
//! list and the masks, and every expensive operation runs on a worker thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use eframe::egui;
use egui::{Color32, ColorImage, RichText, TextureHandle, TextureOptions};
use hdr_core::io::InputEncoding;
use hdr_core::{
    merge, Curve, FrameInput, Mask, MergeOptions, MergeOutput, StackMethod, ToneMapOptions,
};

const PREVIEW_EDGE: u32 = 1400;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 860.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "HDR Merger",
        options,
        Box::new(|cc| Ok(Box::new(HdrApp::new(cc)))),
    )
}

struct FrameEntry {
    path: PathBuf,
    mask: Option<Mask>,
    included: bool,
}

impl FrameEntry {
    fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    fn masked(&self) -> bool {
        self.mask.as_ref().is_some_and(|m| !m.is_empty())
    }
}

enum JobMsg {
    Progress(u32, u32),
    Preview(MergeOutput),
    Exported(PathBuf),
    Failed(String),
}

struct HdrApp {
    frames: Vec<FrameEntry>,
    folder_input: String,
    file_input: String,
    selected: usize,

    method: MethodChoice,
    percentile: f32,
    align: bool,
    input_encoding: InputEncoding,

    curve: CurveChoice,
    auto_exposure: bool,
    exposure_ev: f32,
    target_luminance: f32,

    paint: bool,
    brush_radius: f32,

    output_path: String,
    preview: Option<MergeOutput>,
    texture: Option<TextureHandle>,
    preview_size: egui::Vec2,

    worker: Option<Receiver<JobMsg>>,
    busy: bool,
    progress: (u32, u32),
    status: String,
}

#[derive(PartialEq, Clone, Copy)]
enum MethodChoice {
    Median,
    Mean,
    Sum,
    Max,
    Percentile,
    TrimmedMax,
}

#[derive(PartialEq, Clone, Copy)]
enum CurveChoice {
    Aces,
    Reinhard,
    Hable,
    None,
}

impl CurveChoice {
    fn curve(self) -> Curve {
        match self {
            CurveChoice::Aces => Curve::Aces,
            CurveChoice::Reinhard => Curve::Reinhard,
            CurveChoice::Hable => Curve::Hable,
            CurveChoice::None => Curve::None,
        }
    }
}

impl HdrApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        cc.egui_ctx.set_style(style);
        Self {
            frames: Vec::new(),
            folder_input: String::new(),
            file_input: String::new(),
            selected: 0,
            method: MethodChoice::Median,
            percentile: 90.0,
            align: true,
            input_encoding: InputEncoding::Auto,
            curve: CurveChoice::Aces,
            auto_exposure: true,
            exposure_ev: 0.0,
            target_luminance: 0.55,
            paint: false,
            brush_radius: 24.0,
            output_path: String::new(),
            preview: None,
            texture: None,
            preview_size: egui::vec2(0.0, 0.0),
            worker: None,
            busy: false,
            progress: (0, 0),
            status: "Add frames to begin".to_owned(),
        }
    }

    fn stack_method(&self) -> StackMethod {
        match self.method {
            MethodChoice::Median => StackMethod::Median,
            MethodChoice::Mean => StackMethod::Mean,
            MethodChoice::Sum => StackMethod::Sum,
            MethodChoice::Max => StackMethod::Max,
            MethodChoice::TrimmedMax => StackMethod::TrimmedMax(3),
            MethodChoice::Percentile => StackMethod::Percentile(self.percentile.clamp(1.0, 99.0)),
        }
    }

    fn merge_options(&self, max_dim: Option<u32>) -> MergeOptions {
        MergeOptions {
            method: self.stack_method(),
            align: self.align,
            reference: None,
            max_dim,
            input_encoding: self.input_encoding,
        }
    }

    fn tonemap(&self, image: &hdr_core::LinearImage) -> ToneMapOptions {
        let exposure_ev = if self.auto_exposure {
            hdr_core::tonemap::auto_exposure_ev(image, self.target_luminance)
        } else {
            self.exposure_ev
        };
        ToneMapOptions {
            exposure_ev,
            curve: self.curve.curve(),
            srgb: true,
        }
    }

    fn frame_inputs(&self) -> Vec<FrameInput> {
        self.frames
            .iter()
            .filter(|f| f.included)
            .map(|f| FrameInput {
                path: f.path.clone(),
                mask: f.mask.clone(),
            })
            .collect()
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if self.frames.iter().any(|f| f.path == path) {
                continue;
            }
            let native = hdr_core::io::dimensions(&path).unwrap_or((0, 0));
            let mask = if native != (0, 0) {
                Some(Mask::new(native.0, native.1))
            } else {
                None
            };
            self.frames.push(FrameEntry {
                path,
                mask,
                included: true,
            });
        }
        self.status = format!("{} frame(s)", self.frames.len());
    }

    fn add_folder(&mut self) {
        let folder = PathBuf::from(self.folder_input.trim());
        let Ok(entries) = std::fs::read_dir(&folder) else {
            self.status = format!("cannot read {}", folder.display());
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_supported(p))
            .collect();
        paths.sort();
        let count = paths.len();
        self.add_paths(paths);
        if count == 0 {
            self.status = "no tiff/png/jpeg/exr in that folder".to_owned();
        }
    }

    fn start_preview(&mut self) {
        if self.frames.is_empty() || self.busy {
            return;
        }
        let inputs = self.frame_inputs();
        let options = self.merge_options(Some(PREVIEW_EDGE));
        self.spawn(move |tx| {
            let result = merge(&inputs, &options, &|done, total| {
                let _ = tx.send(JobMsg::Progress(done, total));
            });
            match result {
                Ok(output) => {
                    let _ = tx.send(JobMsg::Preview(output));
                }
                Err(error) => {
                    let _ = tx.send(JobMsg::Failed(error.to_string()));
                }
            }
        });
        self.status = "previewing…".to_owned();
    }

    fn start_export(&mut self) {
        if self.frames.is_empty() || self.busy {
            return;
        }
        let output = PathBuf::from(self.output_path.trim());
        if output.as_os_str().is_empty() {
            self.status = "set an output path first".to_owned();
            return;
        }
        let inputs = self.frame_inputs();
        let options = self.merge_options(None);
        let auto = self.auto_exposure;
        let ev = self.exposure_ev;
        let target = self.target_luminance;
        let curve = self.curve.curve();
        self.spawn(move |tx| {
            let result = merge(&inputs, &options, &|done, total| {
                let _ = tx.send(JobMsg::Progress(done, total));
            });
            match result {
                Ok(merged) => {
                    let exposure_ev = if auto {
                        hdr_core::tonemap::auto_exposure_ev(&merged.image, target)
                    } else {
                        ev
                    };
                    let tonemap = ToneMapOptions {
                        exposure_ev,
                        curve,
                        srgb: true,
                    };
                    match hdr_core::io::save(&output, &merged.image, &tonemap) {
                        Ok(()) => {
                            let _ = tx.send(JobMsg::Exported(output));
                        }
                        Err(error) => {
                            let _ = tx.send(JobMsg::Failed(error.to_string()));
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(JobMsg::Failed(error.to_string()));
                }
            }
        });
        self.status = "exporting…".to_owned();
    }

    fn spawn(&mut self, job: impl FnOnce(std::sync::mpsc::Sender<JobMsg>) + Send + 'static) {
        let (tx, rx) = channel();
        self.worker = Some(rx);
        self.busy = true;
        self.progress = (0, 0);
        thread::spawn(move || job(tx));
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let mut messages = Vec::new();
        if let Some(rx) = &self.worker {
            while let Ok(message) = rx.try_recv() {
                messages.push(message);
            }
        }
        if messages.is_empty() && !self.busy {
            return;
        }
        let mut finished = false;
        for message in messages {
            match message {
                JobMsg::Progress(done, total) => self.progress = (done, total),
                JobMsg::Preview(output) => {
                    self.show_preview(&output, ctx);
                    self.preview = Some(output);
                    self.status = "preview ready".to_owned();
                    finished = true;
                }
                JobMsg::Exported(path) => {
                    self.status = format!("saved {}", path.display());
                    finished = true;
                }
                JobMsg::Failed(error) => {
                    self.status = format!("error: {error}");
                    finished = true;
                }
            }
        }
        if finished {
            self.worker = None;
            self.busy = false;
            ctx.request_repaint();
        } else if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    fn show_preview(&mut self, output: &MergeOutput, ctx: &egui::Context) {
        let image = &output.image;
        let tonemap = self.tonemap(image);
        let bytes = hdr_core::tonemap::to_display8(image, &tonemap);
        let color = ColorImage::from_rgb([image.width as usize, image.height as usize], &bytes);
        self.texture = Some(ctx.load_texture("preview", color, TextureOptions::LINEAR));
        self.preview_size = egui::vec2(image.width as f32, image.height as f32);
    }

    fn paint_mask(&mut self, rect: egui::Rect, pointer: egui::Pos2) {
        let Some(frame) = self.frames.get(self.selected) else {
            return;
        };
        let Some(mask) = &frame.mask else { return };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let u = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let v = ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let scale = mask.width as f32 / rect.width();
        let radius = (self.brush_radius * scale).max(1.0);
        let cx = u * mask.width as f32;
        let cy = v * mask.height as f32;
        if let Some(frame) = self.frames.get_mut(self.selected) {
            if let Some(mask) = &mut frame.mask {
                mask.paint_circle(cx, cy, radius, 0.35, 255);
            }
        }
    }

    fn clear_selected_mask(&mut self) {
        if let Some(frame) = self.frames.get_mut(self.selected) {
            if let Some(mask) = &mut frame.mask {
                mask.clear();
            }
        }
    }
}

fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("tif") | Some("tiff") | Some("png") | Some("jpg") | Some("jpeg") | Some("exr")
    )
}

impl eframe::App for HdrApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker(ctx);
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("HDR Merger");
                ui.separator();
                ui.label(RichText::new(&self.status).color(Color32::GRAY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.busy {
                        let (done, total) = self.progress;
                        let fraction = if total > 0 {
                            done as f32 / total as f32
                        } else {
                            0.0
                        };
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(180.0)
                                .show_percentage(),
                        );
                    }
                });
            });
        });

        egui::SidePanel::left("controls")
            .resizable(true)
            .default_width(330.0)
            .show(ctx, |ui| self.controls_ui(ui));

        egui::CentralPanel::default().show(ctx, |ui| self.canvas_ui(ui));
    }
}

impl HdrApp {
    fn controls_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("SOURCE").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.folder_input).hint_text("folder path"));
                if ui.button("Add").clicked() {
                    self.add_folder();
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.file_input).hint_text("file path"));
                if ui.button("Add").clicked() {
                    let path = PathBuf::from(self.file_input.trim());
                    let mut paths = vec![path.clone()];
                    if path.is_dir() {
                        self.folder_input = self.file_input.clone();
                        self.add_folder();
                        paths.clear();
                    }
                    self.add_paths(paths);
                }
            });
            if ui.button("Clear frames").clicked() {
                self.frames.clear();
                self.preview = None;
                self.texture = None;
            }

            ui.add_space(10.0);
            ui.label(RichText::new("STACK").small().color(Color32::GRAY));
            egui::ComboBox::from_label("method")
                .selected_text(method_label(self.method))
                .show_ui(ui, |ui| {
                    for choice in [
                        MethodChoice::Median,
                        MethodChoice::Percentile,
                        MethodChoice::TrimmedMax,
                        MethodChoice::Mean,
                        MethodChoice::Sum,
                        MethodChoice::Max,
                    ] {
                        ui.selectable_value(&mut self.method, choice, method_label(choice));
                    }
                });
            if self.method == MethodChoice::Percentile {
                ui.add(egui::Slider::new(&mut self.percentile, 1.0..=99.0).text("percentile"));
            }
            ui.checkbox(&mut self.align, "Align frames (translation)");
            egui::ComboBox::from_label("input encoding")
                .selected_text(match self.input_encoding {
                    InputEncoding::Auto => "auto",
                    InputEncoding::Srgb => "sRGB",
                    InputEncoding::Linear => "linear",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.input_encoding, InputEncoding::Auto, "auto");
                    ui.selectable_value(&mut self.input_encoding, InputEncoding::Srgb, "sRGB");
                    ui.selectable_value(&mut self.input_encoding, InputEncoding::Linear, "linear");
                });

            ui.add_space(10.0);
            ui.label(RichText::new("DISPLAY").small().color(Color32::GRAY));
            egui::ComboBox::from_label("curve")
                .selected_text(match self.curve {
                    CurveChoice::Aces => "ACES",
                    CurveChoice::Reinhard => "Reinhard",
                    CurveChoice::Hable => "Hable",
                    CurveChoice::None => "none",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.curve, CurveChoice::Aces, "ACES");
                    ui.selectable_value(&mut self.curve, CurveChoice::Reinhard, "Reinhard");
                    ui.selectable_value(&mut self.curve, CurveChoice::Hable, "Hable");
                    ui.selectable_value(&mut self.curve, CurveChoice::None, "none");
                });
            ui.checkbox(&mut self.auto_exposure, "auto exposure");
            if self.auto_exposure {
                ui.add(egui::Slider::new(&mut self.target_luminance, 0.2..=0.9).text("target"));
            } else {
                ui.add(egui::Slider::new(&mut self.exposure_ev, -8.0..=8.0).text("EV"));
            }

            ui.add_space(10.0);
            ui.label(RichText::new("EXPORT").small().color(Color32::GRAY));
            ui.add(
                egui::TextEdit::singleline(&mut self.output_path)
                    .hint_text("out.exr / out.tif / out.jpg"),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Preview"))
                    .clicked()
                {
                    self.start_preview();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Export full"))
                    .clicked()
                {
                    self.start_export();
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.label(RichText::new("FRAMES").small().color(Color32::GRAY));
            let mut remove: Option<usize> = None;
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    for index in 0..self.frames.len() {
                        let selected = index == self.selected;
                        let label = self.frames[index].label();
                        let masked = self.frames[index].masked();
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.frames[index].included, "");
                            let text = if masked {
                                RichText::new(format!("{label}  [mask]")).color(Color32::LIGHT_BLUE)
                            } else {
                                RichText::new(label)
                            };
                            if ui.selectable_label(selected, text).clicked() {
                                self.selected = index;
                            }
                            if ui.small_button("x").clicked() {
                                remove = Some(index);
                            }
                        });
                    }
                });
            if let Some(index) = remove {
                self.frames.remove(index);
                self.selected = self.selected.min(self.frames.len().saturating_sub(1));
            }

            ui.add_space(10.0);
            ui.label(RichText::new("MASK").small().color(Color32::GRAY));
            ui.checkbox(&mut self.paint, "paint mask on selected frame");
            ui.add(egui::Slider::new(&mut self.brush_radius, 4.0..=160.0).text("brush"));
            ui.horizontal(|ui| {
                if ui.button("Clear mask").clicked() {
                    self.clear_selected_mask();
                }
                if let Some(frame) = self.frames.get(self.selected) {
                    if let Some(mask) = &frame.mask {
                        ui.label(
                            RichText::new(format!(
                                "{}x{} · {} px",
                                mask.width,
                                mask.height,
                                mask.excluded_count()
                            ))
                            .small()
                            .color(Color32::GRAY),
                        );
                    }
                }
            });
        });
    }

    fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let rect = ui
            .allocate_exact_size(available, egui::Sense::click_and_drag())
            .0;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_rgb(24, 24, 26));

        let Some(texture) = &self.texture else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "no preview yet",
                egui::FontId::proportional(16.0),
                Color32::GRAY,
            );
            return;
        };

        let size = fit_size(self.preview_size, rect.size());
        let image_rect = egui::Rect::from_center_size(rect.center(), size);
        ui.put(image_rect, egui::Image::new((texture.id(), size)));

        if self.paint {
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                if image_rect.contains(pointer) {
                    let response = ui.allocate_rect(image_rect, egui::Sense::click_and_drag());
                    if response.is_pointer_button_down_on() || response.clicked() {
                        self.paint_mask(image_rect, pointer);
                    }
                    painter.circle_stroke(
                        pointer,
                        self.brush_radius,
                        egui::Stroke::new(1.5_f32, Color32::from_rgb(120, 200, 255)),
                    );
                }
            }
        }
    }
}

fn fit_size(content: egui::Vec2, bounds: egui::Vec2) -> egui::Vec2 {
    if content.x <= 0.0 || content.y <= 0.0 {
        return egui::vec2(0.0, 0.0);
    }
    let scale = (bounds.x / content.x).min(bounds.y / content.y).min(1.0);
    egui::vec2(content.x * scale, content.y * scale)
}

fn method_label(choice: MethodChoice) -> &'static str {
    match choice {
        MethodChoice::Median => "median (removes mover)",
        MethodChoice::Percentile => "percentile",
        MethodChoice::TrimmedMax => "trimmed max (drop 3)",
        MethodChoice::Mean => "mean",
        MethodChoice::Sum => "sum",
        MethodChoice::Max => "max",
    }
}
