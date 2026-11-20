//! Design tokens and the small set of widgets every panel is built from.
//!
//! Rule: a panel may not invent spacing, colours or type sizes. It calls this
//! module. If something is missing here, add it here first.

use eframe::egui::{self, Color32, Response, RichText, Stroke, Vec2};

// ---- spacing scale (multiples of 4) -------------------------------------
pub const SP_XS: f32 = 4.0;
pub const SP_S: f32 = 8.0;
pub const SP_M: f32 = 12.0;
pub const SP_L: f32 = 16.0;
pub const SP_XL: f32 = 24.0;

pub const RADIUS: f32 = 6.0;
pub const LABEL_W: f32 = 104.0;
pub const CONTROL_W: f32 = 132.0;
pub const PANEL_W: f32 = 336.0;

pub mod color {
    use super::Color32;

    pub const BG: Color32 = Color32::from_rgb(16, 17, 19);
    pub const PANEL: Color32 = Color32::from_rgb(22, 23, 26);
    pub const CARD: Color32 = Color32::from_rgb(30, 32, 36);
    pub const CARD_HOVER: Color32 = Color32::from_rgb(38, 40, 46);
    pub const STROKE: Color32 = Color32::from_rgb(48, 51, 58);
    pub const TEXT: Color32 = Color32::from_rgb(230, 231, 235);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 142, 152);
    pub const ACCENT: Color32 = Color32::from_rgb(96, 168, 255);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(36, 64, 102);
    pub const OK: Color32 = Color32::from_rgb(104, 196, 138);
    pub const WARN: Color32 = Color32::from_rgb(232, 178, 84);
    pub const ERROR: Color32 = Color32::from_rgb(230, 108, 108);
}

/// Install the app-wide look. Called once at startup and again on theme flip.
pub fn set_dark(ctx: &egui::Context, dark: bool) {
    let mut style = (*ctx.style()).clone();
    style.visuals = if dark { visuals() } else { light_visuals() };
    ctx.set_style(style);
}

fn light_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    v.panel_fill = Color32::from_rgb(244, 245, 247);
    v.window_fill = Color32::from_rgb(250, 250, 252);
    v.extreme_bg_color = Color32::from_rgb(232, 233, 236);
    v.faint_bg_color = Color32::from_rgb(238, 239, 242);
    v.override_text_color = Some(Color32::from_rgb(32, 33, 36));
    v.selection.bg_fill = Color32::from_rgb(198, 222, 255);
    v.selection.stroke = Stroke::new(1.0_f32, color::ACCENT);
    v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(210, 212, 216));
    v.window_rounding = RADIUS.into();
    for widget in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.rounding = RADIUS.into();
    }
    v.widgets.noninteractive.bg_fill = Color32::from_rgb(244, 245, 247);
    v.widgets.inactive.bg_fill = Color32::from_rgb(236, 237, 240);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(236, 237, 240);
    v.widgets.hovered.bg_fill = Color32::from_rgb(226, 228, 233);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(226, 228, 233);
    v.widgets.active.bg_fill = Color32::from_rgb(198, 222, 255);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(198, 222, 255);
    v
}

pub fn install(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(SP_S, SP_S);
    style.spacing.button_padding = Vec2::new(SP_M, 6.0);
    style.spacing.indent = SP_L;
    style.spacing.slider_width = CONTROL_W;
    style.spacing.interact_size.y = 20.0;
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::proportional(17.0)),
        (egui::TextStyle::Body, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(11.0)),
        (egui::TextStyle::Monospace, egui::FontId::monospace(12.0)),
    ]
    .into();
    style.visuals = visuals();
    ctx.set_style(style);
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = color::PANEL;
    v.window_fill = color::PANEL;
    v.extreme_bg_color = color::BG;
    v.faint_bg_color = color::CARD;
    v.override_text_color = Some(color::TEXT);
    v.selection.bg_fill = color::ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0_f32, color::ACCENT);
    v.window_stroke = Stroke::new(1.0_f32, color::STROKE);
    v.window_rounding = RADIUS.into();

    for widget in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        widget.rounding = RADIUS.into();
    }
    v.widgets.noninteractive.bg_fill = color::PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, color::STROKE);
    v.widgets.inactive.bg_fill = color::CARD;
    v.widgets.inactive.weak_bg_fill = color::CARD;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_fill = color::CARD_HOVER;
    v.widgets.hovered.weak_bg_fill = color::CARD_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, color::STROKE);
    v.widgets.active.bg_fill = color::ACCENT_DIM;
    v.widgets.active.weak_bg_fill = color::ACCENT_DIM;
    v.widgets.open.bg_fill = color::CARD_HOVER;
    v
}

// ---- typography ---------------------------------------------------------

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(SP_L);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title.to_uppercase())
                .size(10.5)
                .strong()
                .color(color::TEXT_DIM),
        );
    });
    ui.add_space(SP_XS);
}

/// Numbered, ruled header for a workflow step: badge, title, right-aligned
/// summary of the current state, hairline under it.
pub fn step_header(ui: &mut egui::Ui, step: u32, title: &str, summary: &str) {
    ui.add_space(SP_L);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(20.0, 20.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 9.0, color::ACCENT_DIM);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            step.to_string(),
            egui::FontId::proportional(10.5),
            color::TEXT,
        );
        ui.add_space(SP_XS);
        ui.label(
            RichText::new(title.to_uppercase())
                .size(11.0)
                .strong()
                .color(color::TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !summary.is_empty() {
                ui.label(RichText::new(summary).size(10.0).color(color::TEXT_DIM));
            }
        });
    });
    ui.add_space(SP_XS);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::STROKE);
    ui.add_space(SP_S);
}

pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).size(11.0).color(color::TEXT_DIM));
}

/// Hairline separator between groups inside a section.
pub fn divider(ui: &mut egui::Ui) {
    ui.add_space(SP_S);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::STROKE);
    ui.add_space(SP_S);
}

// ---- layout primitives --------------------------------------------------

/// One labelled control row. Every setting in every panel goes through this,
/// which is what keeps the label column aligned across sections.
pub fn control_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_W, 18.0],
            egui::Label::new(RichText::new(label).color(color::TEXT_DIM))
                .truncate()
                .halign(egui::Align::LEFT),
        );
        add(ui)
    })
    .inner
}

/// Slider plus a typed value: the roadmap rule "a slider is never the only way".
pub fn slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) {
    ui.spacing_mut().slider_width = CONTROL_W * 0.62;
    ui.add(
        egui::Slider::new(value, range.clone())
            .show_value(false)
            .trailing_fill(true),
    );
    ui.add(
        egui::DragValue::new(value)
            .speed(0.1)
            .range(range)
            .suffix(suffix)
            .max_decimals(2),
    );
}

pub fn int_slider(
    ui: &mut egui::Ui,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    suffix: &str,
) {
    ui.spacing_mut().slider_width = CONTROL_W * 0.62;
    ui.add(egui::Slider::new(value, range.clone()).show_value(false));
    ui.add(
        egui::DragValue::new(value)
            .speed(1.0)
            .range(range)
            .suffix(suffix),
    );
}

// ---- buttons ------------------------------------------------------------

pub fn primary_button(ui: &mut egui::Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).strong().color(Color32::WHITE))
        .fill(color::ACCENT_DIM)
        .stroke(Stroke::new(1.0_f32, color::ACCENT))
        .min_size(Vec2::new(0.0, 28.0));
    ui.add(button)
}

pub fn ghost_button(ui: &mut egui::Ui, text: &str) -> Response {
    let button = egui::Button::new(RichText::new(text).color(color::TEXT))
        .fill(color::CARD)
        .stroke(Stroke::new(1.0_f32, color::STROKE))
        .min_size(Vec2::new(0.0, 28.0));
    ui.add(button)
}

/// Small square icon-ish button used in headers and the filmstrip.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tooltip: &str) -> Response {
    let response = ui.add(
        egui::Button::new(RichText::new(glyph).size(13.0))
            .fill(color::CARD)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(24.0, 22.0)),
    );
    response.on_hover_text(tooltip)
}

pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.18))
        .rounding(RADIUS / 2.0)
        .inner_margin(egui::Margin::symmetric(6.0, 1.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).color(color));
        });
}
