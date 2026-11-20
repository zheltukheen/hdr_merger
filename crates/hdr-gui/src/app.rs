//! Application state and every action the UI can trigger.
//!
//! The UI never touches `hdr-core` directly: it mutates this state and calls the
//! methods below, which is what keeps the panels free of logic.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use eframe::egui;
use egui::TextureHandle;
use hdr_core::io::InputEncoding;
use hdr_core::recipe::{ExportSettings, Recipe, RecipeFrame};
use hdr_core::{
    Curve, FrameInput, Mask, MergeOptions, MergeOutput, MergeTimings, StackMethod, ToneMapOptions,
};

use crate::meta::{self, Meta};
use crate::theme;

pub const PREVIEW_EDGE: u32 = 1400;
pub const PROXY_CHOICES: [u32; 4] = [1000, 1400, 2000, 0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    CaptureTime,
    Name,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MethodChoice {
    Median,
    Percentile,
    TrimmedMax,
    Mean,
    Sum,
    Max,
}

impl MethodChoice {
    pub fn label(self) -> &'static str {
        match self {
            MethodChoice::Median => "Median",
            MethodChoice::Percentile => "Percentile",
            MethodChoice::TrimmedMax => "Trimmed max",
            MethodChoice::Mean => "Mean",
            MethodChoice::Sum => "Sum",
            MethodChoice::Max => "Max",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            MethodChoice::Median => "Removes anything that moved. The safe default.",
            MethodChoice::Percentile => "Keeps the strongest stable contribution per pixel.",
            MethodChoice::TrimmedMax => "Brightest value after dropping the top N.",
            MethodChoice::Mean => "Average. Lower noise, movers smear.",
            MethodChoice::Sum => "Total light. Highlights will clip.",
            MethodChoice::Max => "Lighten blend: the brightest frame everywhere.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CurveChoice {
    Aces,
    Reinhard,
    Hable,
    None,
}

impl CurveChoice {
    pub fn curve(self) -> Curve {
        match self {
            CurveChoice::Aces => Curve::Aces,
            CurveChoice::Reinhard => Curve::Reinhard,
            CurveChoice::Hable => Curve::Hable,
            CurveChoice::None => Curve::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CurveChoice::Aces => "ACES",
            CurveChoice::Reinhard => "Reinhard",
            CurveChoice::Hable => "Hable",
            CurveChoice::None => "Linear",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Inspect,
    Brush,
    Eraser,
}

pub struct Frame {
    pub path: PathBuf,
    pub meta: Meta,
    /// Allocated lazily on the first brush stroke: a 60 MP mask is 60 MB and
    /// most frames never get one.
    pub mask: Option<Mask>,
    pub included: bool,
}

impl Frame {
    pub fn name(&self) -> String {
        meta::file_stem(&self.path)
    }

    pub fn masked(&self) -> bool {
        self.mask.as_ref().is_some_and(|m| !m.is_empty())
    }
}

pub struct Preview {
    pub image: hdr_core::LinearImage,
    pub display: Vec<u8>,
    pub tone: Option<ToneMapOptions>,
    pub exposure_ev: f32,
    pub warnings: Vec<String>,
    pub used: usize,
    pub timings: MergeTimings,
}

pub enum JobKind {
    Preview,
    Export,
}

pub enum JobMsg {
    Progress(u32, u32),
    Preview(Box<MergeOutput>),
    Exported(PathBuf),
    Failed(String),
}

struct Job {
    kind: JobKind,
    rx: Receiver<JobMsg>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Busy,
    Ok,
    Error,
}

pub struct App {
    pub frames: Vec<Frame>,
    pub selected: usize,
    pub sort_by: SortBy,

    pub method: MethodChoice,
    pub percentile: f32,
    pub trimmed: u32,
    pub align: bool,
    pub reference: usize,
    pub shift_guard: bool,
    pub max_shift: u32,
    pub crop: bool,
    pub proxy: u32,
    pub input_encoding: InputEncoding,

    pub curve: CurveChoice,
    pub auto_exposure: bool,
    pub exposure_ev: f32,
    pub target: f32,

    pub tool: Tool,
    pub brush: f32,
    pub softness: f32,
    pub overlay: bool,

    pub add_path: String,
    pub out_path: String,
    pub out_edge_on: bool,
    pub out_edge: u32,
    pub recipe_path: String,

    pub preview: Option<Preview>,
    pub texture: Option<TextureHandle>,
    pub histogram: Option<Vec<u32>>,
    pub mask_overlay: Option<TextureHandle>,
    pub mask_overlay_key: Option<(usize, usize)>,

    pub view_zoom: f32,
    pub view_fit: bool,
    pub view_offset: egui::Vec2,

    pub status: String,
    pub status_level: StatusLevel,
    pub progress: (u32, u32),
    job: Option<Job>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_context(&cc.egui_ctx)
    }

    /// Same as [`App::new`] but without the eframe shell, so the layout can be
    /// exercised headlessly in tests.
    pub fn with_context(ctx: &egui::Context) -> Self {
        theme::install(ctx);
        Self {
            frames: Vec::new(),
            selected: 0,
            sort_by: SortBy::CaptureTime,
            method: MethodChoice::Median,
            percentile: 90.0,
            trimmed: 3,
            align: true,
            reference: 0,
            shift_guard: false,
            max_shift: 50,
            crop: true,
            proxy: PREVIEW_EDGE,
            input_encoding: InputEncoding::Auto,
            curve: CurveChoice::Aces,
            auto_exposure: true,
            exposure_ev: 0.0,
            target: 0.55,
            tool: Tool::Inspect,
            brush: 28.0,
            softness: 0.35,
            overlay: true,
            add_path: String::new(),
            out_path: String::new(),
            out_edge_on: false,
            out_edge: 4000,
            recipe_path: String::new(),
            preview: None,
            texture: None,
            histogram: None,
            mask_overlay: None,
            mask_overlay_key: None,
            view_zoom: 1.0,
            view_fit: true,
            view_offset: egui::Vec2::ZERO,
            status: "Drop a folder or add frames to begin".to_owned(),
            status_level: StatusLevel::Info,
            progress: (0, 0),
            job: None,
        }
    }

    // ---- state helpers -------------------------------------------------

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn job_label(&self) -> &'static str {
        match self.job.as_ref().map(|j| &j.kind) {
            Some(JobKind::Preview) => "Preview",
            Some(JobKind::Export) => "Export",
            None => "",
        }
    }

    pub fn included_count(&self) -> usize {
        self.frames.iter().filter(|f| f.included).count()
    }

    pub fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status = text.into();
        self.status_level = level;
    }

    pub fn stack_method(&self) -> StackMethod {
        match self.method {
            MethodChoice::Median => StackMethod::Median,
            MethodChoice::Percentile => StackMethod::Percentile(self.percentile.clamp(1.0, 99.0)),
            MethodChoice::TrimmedMax => StackMethod::TrimmedMax(self.trimmed as usize),
            MethodChoice::Mean => StackMethod::Mean,
            MethodChoice::Sum => StackMethod::Sum,
            MethodChoice::Max => StackMethod::Max,
        }
    }

    pub fn merge_options(&self, max_dim: Option<u32>) -> MergeOptions {
        MergeOptions {
            method: self.stack_method(),
            align: self.align,
            reference: Some(self.reference.min(self.frames.len().saturating_sub(1))),
            max_dim,
            input_encoding: self.input_encoding,
            max_shift: self.shift_guard.then_some(self.max_shift),
            crop_to_intersection: self.crop,
            use_gpu: true,
        }
    }

    pub fn tone_for(&self, image: &hdr_core::LinearImage) -> ToneMapOptions {
        let exposure_ev = if self.auto_exposure {
            hdr_core::tonemap::auto_exposure_ev(image, self.target)
        } else {
            self.exposure_ev
        };
        ToneMapOptions {
            exposure_ev,
            curve: self.curve.curve(),
            srgb: true,
        }
    }

    pub fn frame_inputs(&self) -> Vec<FrameInput> {
        self.frames
            .iter()
            .filter(|f| f.included)
            .map(|f| FrameInput {
                path: f.path.clone(),
                mask: f.mask.clone(),
            })
            .collect()
    }

    // ---- frame list ----------------------------------------------------

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let expanded = meta::expand(paths);
        let before = self.frames.len();
        for path in expanded {
            if self.frames.iter().any(|f| f.path == path) {
                continue;
            }
            let meta = meta::probe(&path);
            self.frames.push(Frame {
                path,
                meta,
                mask: None,
                included: true,
            });
        }
        if self.frames.len() != before {
            self.sort_frames();
            self.reference = self.reference.min(self.frames.len().saturating_sub(1));
            self.invalidate_preview();
            self.set_status(
                StatusLevel::Info,
                format!("{} frame(s) loaded", self.frames.len()),
            );
        } else {
            self.set_status(StatusLevel::Info, "nothing new to add");
        }
    }

    pub fn remove_frame(&mut self, index: usize) {
        if index < self.frames.len() {
            self.frames.remove(index);
            self.selected = self.selected.min(self.frames.len().saturating_sub(1));
            self.reference = self.reference.min(self.frames.len().saturating_sub(1));
            self.invalidate_preview();
        }
    }

    pub fn clear_frames(&mut self) {
        self.frames.clear();
        self.preview = None;
        self.texture = None;
        self.histogram = None;
        self.set_status(StatusLevel::Info, "cleared");
    }

    pub fn sort_frames(&mut self) {
        let current = self.frames.get(self.selected).map(|f| f.path.clone());
        match self.sort_by {
            SortBy::CaptureTime => self.frames.sort_by_key(|f| f.meta.sort_key(&f.path)),
            SortBy::Name => self.frames.sort_by(|a, b| a.path.cmp(&b.path)),
        }
        if let Some(path) = current {
            if let Some(index) = self.frames.iter().position(|f| f.path == path) {
                self.selected = index;
            }
        }
    }

    pub fn invalidate_preview(&mut self) {
        self.preview = None;
        self.texture = None;
        self.histogram = None;
        self.mask_overlay = None;
        self.mask_overlay_key = None;
    }

    // ---- masks ---------------------------------------------------------

    pub fn ensure_mask(&mut self, index: usize) -> Option<&mut Mask> {
        let frame = self.frames.get_mut(index)?;
        if frame.mask.is_none() {
            let (w, h) = (frame.meta.width, frame.meta.height);
            if w == 0 || h == 0 {
                return None;
            }
            frame.mask = Some(Mask::new(w, h));
        }
        frame.mask.as_mut()
    }

    pub fn clear_mask(&mut self, index: usize) {
        if let Some(frame) = self.frames.get_mut(index) {
            if let Some(mask) = &mut frame.mask {
                mask.clear();
            }
        }
    }

    pub fn clear_all_masks(&mut self) {
        for frame in &mut self.frames {
            if let Some(mask) = &mut frame.mask {
                mask.clear();
            }
        }
    }

    pub fn masked_frames(&self) -> usize {
        self.frames.iter().filter(|f| f.masked()).count()
    }

    // ---- jobs ----------------------------------------------------------

    fn spawn(&mut self, kind: JobKind, job: impl FnOnce(Sender<JobMsg>) + Send + 'static) {
        let (tx, rx) = channel();
        self.job = Some(Job { kind, rx });
        self.progress = (0, 0);
        thread::spawn(move || job(tx));
    }

    pub fn start_preview(&mut self) {
        if self.frames.is_empty() || self.busy() {
            return;
        }
        let inputs = self.frame_inputs();
        let options = self.merge_options((self.proxy > 0).then_some(self.proxy));
        self.set_status(StatusLevel::Busy, "stacking preview…");
        self.spawn(JobKind::Preview, move |tx| {
            let result = hdr_core::merge(&inputs, &options, &|done, total| {
                let _ = tx.send(JobMsg::Progress(done, total));
            });
            match result {
                Ok(output) => {
                    let _ = tx.send(JobMsg::Preview(Box::new(output)));
                }
                Err(error) => {
                    let _ = tx.send(JobMsg::Failed(error.to_string()));
                }
            }
        });
    }

    pub fn start_export(&mut self) {
        if self.frames.is_empty() || self.busy() {
            return;
        }
        let out = PathBuf::from(self.out_path.trim());
        if out.as_os_str().is_empty() {
            self.set_status(StatusLevel::Error, "set an output path first");
            return;
        }
        let inputs = self.frame_inputs();
        let options = self.merge_options(None);
        let auto = self.auto_exposure;
        let ev = self.exposure_ev;
        let target = self.target;
        let curve = self.curve.curve();
        let edge = self.out_edge_on.then_some(self.out_edge);
        self.set_status(StatusLevel::Busy, "exporting full resolution…");
        self.spawn(JobKind::Export, move |tx| {
            let result = hdr_core::merge(&inputs, &options, &|done, total| {
                let _ = tx.send(JobMsg::Progress(done, total));
            });
            let merged = match result {
                Ok(merged) => merged,
                Err(error) => {
                    let _ = tx.send(JobMsg::Failed(error.to_string()));
                    return;
                }
            };
            let mut image = merged.image;
            if let Some(edge) = edge {
                image = image.fit_within(edge);
            }
            let exposure_ev = if auto {
                hdr_core::tonemap::auto_exposure_ev(&image, target)
            } else {
                ev
            };
            let tonemap = ToneMapOptions {
                exposure_ev,
                curve,
                srgb: true,
            };
            match hdr_core::io::save(&out, &image, &tonemap) {
                Ok(()) => {
                    let _ = tx.send(JobMsg::Exported(out));
                }
                Err(error) => {
                    let _ = tx.send(JobMsg::Failed(error.to_string()));
                }
            }
        });
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut messages = Vec::new();
        if let Some(job) = &self.job {
            while let Ok(message) = job.rx.try_recv() {
                messages.push(message);
            }
        }
        let mut finished = false;
        for message in messages {
            match message {
                JobMsg::Progress(done, total) => self.progress = (done, total),
                JobMsg::Preview(output) => {
                    let used = output.shifts.len();
                    let timings = output.timings;
                    self.preview = Some(Preview {
                        image: output.image,
                        display: Vec::new(),
                        tone: None,
                        exposure_ev: 0.0,
                        warnings: output.warnings,
                        used,
                        timings,
                    });
                    self.set_status(
                        StatusLevel::Ok,
                        format!("preview ready · {} ms", timings.total_ms),
                    );
                    finished = true;
                }
                JobMsg::Exported(path) => {
                    self.set_status(StatusLevel::Ok, format!("saved {}", path.display()));
                    finished = true;
                }
                JobMsg::Failed(error) => {
                    self.set_status(StatusLevel::Error, error);
                    finished = true;
                }
            }
        }
        if finished {
            self.job = None;
            ctx.request_repaint();
        } else if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Rebuild the preview texture when the tone settings changed.
    pub fn ensure_texture(&mut self, ctx: &egui::Context) {
        let Some(tone) = self.preview.as_ref().map(|p| self.tone_for(&p.image)) else {
            return;
        };
        let refresh = self
            .preview
            .as_ref()
            .map(|p| p.tone.as_ref() != Some(&tone) || self.texture.is_none())
            .unwrap_or(false);
        if !refresh {
            return;
        }
        let Some(preview) = self.preview.as_mut() else {
            return;
        };
        preview.display = hdr_core::tonemap::to_display8(&preview.image, &tone);
        preview.tone = Some(tone.clone());
        preview.exposure_ev = tone.exposure_ev;
        let size = [preview.image.width as usize, preview.image.height as usize];
        let color = egui::ColorImage::from_rgb(size, &preview.display);
        self.texture = Some(ctx.load_texture("preview", color, egui::TextureOptions::LINEAR));
        self.histogram = Some(histogram(&preview.display));
    }

    // ---- recipes -------------------------------------------------------

    pub fn save_recipe(&mut self) {
        let path = PathBuf::from(self.recipe_path.trim());
        if path.as_os_str().is_empty() {
            self.set_status(StatusLevel::Error, "set a recipe path first");
            return;
        }
        let recipe = Recipe {
            schema: hdr_core::recipe::RECIPE_SCHEMA,
            frames: self
                .frames
                .iter()
                .map(|frame| RecipeFrame {
                    path: frame.path.clone(),
                    included: frame.included,
                    mask: frame.mask.clone(),
                })
                .collect(),
            stack: self.merge_options(None),
            tone: ToneMapOptions {
                exposure_ev: self.exposure_ev,
                curve: self.curve.curve(),
                srgb: true,
            },
            export: ExportSettings {
                path: Some(PathBuf::from(self.out_path.trim()))
                    .filter(|p| !p.as_os_str().is_empty()),
                max_edge: self.out_edge_on.then_some(self.out_edge),
            },
        };
        match recipe.save(&path) {
            Ok(()) => self.set_status(StatusLevel::Ok, format!("recipe saved {}", path.display())),
            Err(error) => self.set_status(StatusLevel::Error, error.to_string()),
        }
    }

    pub fn load_recipe(&mut self) {
        let path = PathBuf::from(self.recipe_path.trim());
        if path.as_os_str().is_empty() {
            self.set_status(StatusLevel::Error, "set a recipe path first");
            return;
        }
        match Recipe::load(&path) {
            Ok(recipe) => self.apply_recipe(recipe),
            Err(error) => self.set_status(StatusLevel::Error, error.to_string()),
        }
    }

    fn apply_recipe(&mut self, recipe: Recipe) {
        self.frames = recipe
            .frames
            .into_iter()
            .map(|frame| {
                let meta = meta::probe(&frame.path);
                Frame {
                    path: frame.path,
                    meta,
                    mask: frame.mask,
                    included: frame.included,
                }
            })
            .collect();
        self.selected = 0;
        self.reference = recipe.stack.reference.unwrap_or(0);
        self.align = recipe.stack.align;
        self.crop = recipe.stack.crop_to_intersection;
        self.shift_guard = recipe.stack.max_shift.is_some();
        self.max_shift = recipe.stack.max_shift.unwrap_or(self.max_shift);
        self.input_encoding = recipe.stack.input_encoding;
        self.method = match recipe.stack.method {
            StackMethod::Median => MethodChoice::Median,
            StackMethod::Percentile(_) => MethodChoice::Percentile,
            StackMethod::TrimmedMax(_) => MethodChoice::TrimmedMax,
            StackMethod::Mean => MethodChoice::Mean,
            StackMethod::Sum => MethodChoice::Sum,
            StackMethod::Max => MethodChoice::Max,
        };
        if let StackMethod::Percentile(p) = recipe.stack.method {
            self.percentile = p;
        }
        if let StackMethod::TrimmedMax(n) = recipe.stack.method {
            self.trimmed = n as u32;
        }
        self.exposure_ev = recipe.tone.exposure_ev;
        self.curve = match recipe.tone.curve {
            Curve::Aces => CurveChoice::Aces,
            Curve::Reinhard => CurveChoice::Reinhard,
            Curve::Hable => CurveChoice::Hable,
            Curve::None => CurveChoice::None,
        };
        if let Some(path) = recipe.export.path {
            self.out_path = path.display().to_string();
        }
        self.out_edge_on = recipe.export.max_edge.is_some();
        if let Some(edge) = recipe.export.max_edge {
            self.out_edge = edge;
        }
        self.invalidate_preview();
        self.set_status(
            StatusLevel::Ok,
            format!("recipe loaded: {} frame(s)", self.frames.len()),
        );
    }

    // ---- drag and drop -------------------------------------------------

    pub fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped);
        }
    }
}

/// 64-bin luminance histogram of a display-encoded RGB buffer.
pub fn histogram(rgb: &[u8]) -> Vec<u32> {
    let mut bins = vec![0u32; 64];
    for pixel in rgb.chunks_exact(3) {
        let luma = 0.2126 * pixel[0] as f32 + 0.7152 * pixel[1] as f32 + 0.0722 * pixel[2] as f32;
        let index = ((luma / 255.0) * 63.0).round().clamp(0.0, 63.0) as usize;
        bins[index] += 1;
    }
    bins
}
