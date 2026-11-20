//! Canvas: empty state, image view, mask painting, histogram, filmstrip.

use eframe::egui;
use egui::{Color32, RichText};
use hdr_core::Mask;

use crate::app::{App, StatusLevel, Tool};
use crate::theme::{self, color, SP_L, SP_M, SP_S, SP_XL, SP_XS};

pub fn central(app: &mut App, ui: &mut egui::Ui) {
    if app.frames.is_empty() {
        empty_state(app, ui);
        return;
    }
    let size = ui.available_size();
    let rect = ui
        .allocate_exact_size(size, egui::Sense::click_and_drag())
        .0;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, color::BG);

    let Some(texture) = app.texture.clone() else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Press Preview to stack the frames",
            egui::FontId::proportional(15.0),
            color::TEXT_DIM,
        );
        return;
    };
    let Some(preview) = app.preview.as_ref() else {
        return;
    };
    let image_size = egui::vec2(preview.image.width as f32, preview.image.height as f32);
    let fit = (rect.width() / image_size.x).min(rect.height() / image_size.y);
    let scale = if app.view_fit {
        fit
    } else {
        fit * app.view_zoom
    };
    let display = image_size * scale;
    let image_rect = egui::Rect::from_center_size(rect.center() + app.view_offset, display);

    painter.image(
        texture.id(),
        image_rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );

    if app.overlay {
        if let Some(overlay) = mask_overlay_texture(app, ui.ctx(), image_size) {
            painter.image(
                overlay.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    interact(app, ui, rect, image_rect, scale);
    histogram(app, &painter, rect);
    footer(app, &painter, rect);
}

fn interact(
    app: &mut App,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    image_rect: egui::Rect,
    scale: f32,
) {
    let response = ui.interact(rect, ui.id().with("canvas"), egui::Sense::click_and_drag());
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.0 {
            app.view_zoom = (app.view_zoom * (1.0 + scroll * 0.0015)).clamp(0.1, 8.0);
            app.view_fit = false;
        }
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        app.view_offset += response.drag_delta();
    }
    let space = ui.input(|i| i.key_down(egui::Key::Space));
    if app.tool == Tool::Inspect && response.dragged_by(egui::PointerButton::Primary) && space {
        app.view_offset += response.drag_delta();
    }

    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    if !image_rect.contains(pointer) {
        return;
    }
    if app.tool != Tool::Inspect && response.drag_started() {
        app.begin_mask_edit();
    }
    if app.tool != Tool::Inspect && response.dragged_by(egui::PointerButton::Primary) {
        paint(app, pointer, image_rect, ui.input(|i| i.modifiers.shift));
    }
    if app.tool == Tool::Inspect {
        if let Some(preview) = &app.preview {
            let u = ((pointer.x - image_rect.left()) / image_rect.width()).clamp(0.0, 1.0);
            let v = ((pointer.y - image_rect.top()) / image_rect.height()).clamp(0.0, 1.0);
            let x = (u * preview.image.width as f32) as u32;
            let y = (v * preview.image.height as f32) as u32;
            let width = preview.image.width as usize;
            let index = (y as usize * width + x as usize) * 3;
            if let Some(pixel) = preview.display.get(index..index + 3) {
                let value = [
                    pixel[0] as f32 / 255.0,
                    pixel[1] as f32 / 255.0,
                    pixel[2] as f32 / 255.0,
                ];
                app.inspect = Some((x, y, value));
            }
        }
    } else {
        let radius = (app.brush * scale).clamp(1.0, 400.0);
        ui.painter()
            .circle_stroke(pointer, radius, egui::Stroke::new(1.5_f32, color::ACCENT));
    }
}

fn paint(app: &mut App, pointer: egui::Pos2, image_rect: egui::Rect, force_erase: bool) {
    let u = (pointer.x - image_rect.left()) / image_rect.width();
    let v = (pointer.y - image_rect.top()) / image_rect.height();
    let index = app.selected;
    let brush = app.brush;
    let softness = app.softness;
    let erasing = app.tool == Tool::Eraser || force_erase;
    let Some(mask) = app.ensure_mask(index) else {
        return;
    };
    let cx = u * mask.width as f32;
    let cy = v * mask.height as f32;
    let radius = (brush * (mask.width as f32 / image_rect.width())).max(1.0);
    if erasing {
        erase_circle(mask, cx, cy, radius);
    } else {
        mask.paint_circle(cx, cy, radius, softness, 255);
    }
}

fn erase_circle(mask: &mut Mask, cx: f32, cy: f32, radius: f32) {
    let x0 = (cx - radius).floor().max(0.0) as u32;
    let x1 = (cx + radius).ceil().clamp(0.0, mask.width as f32 - 1.0) as u32;
    let y0 = (cy - radius).floor().max(0.0) as u32;
    let y1 = (cy + radius).ceil().clamp(0.0, mask.height as f32 - 1.0) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= radius * radius {
                let offset = y as usize * mask.width as usize + x as usize;
                mask.data[offset] = 0;
            }
        }
    }
}

fn mask_overlay_texture(
    app: &mut App,
    ctx: &egui::Context,
    image_size: egui::Vec2,
) -> Option<egui::TextureHandle> {
    let frame = app.frames.get(app.selected)?;
    if !frame.masked() {
        return None;
    }
    let key = (
        app.selected,
        frame.mask.as_ref().map(|m| m.excluded_count()).unwrap_or(0),
    );
    if app.mask_overlay_key == Some(key) {
        return app.mask_overlay.clone();
    }

    // Build the tinted overlay before touching `app`: the mask borrow has to
    // end first, and cloning a 60 MP mask just to get out of a borrow is worse.
    let (width, height, pixels) = {
        let mask = frame.mask.as_ref()?;
        let width = image_size.x.max(1.0) as usize;
        let height = image_size.y.max(1.0) as usize;
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let my = ((y as f32 / height as f32) * mask.height as f32) as usize;
            let my = my.min(mask.height.saturating_sub(1) as usize);
            for x in 0..width {
                let mx = ((x as f32 / width as f32) * mask.width as f32) as usize;
                let mx = mx.min(mask.width.saturating_sub(1) as usize);
                let excluded = mask
                    .data
                    .get(my * mask.width as usize + mx)
                    .copied()
                    .unwrap_or(0);
                if excluded > 127 {
                    pixels.extend_from_slice(&[235, 72, 72, 70]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        (width, height, pixels)
    };

    let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels);
    let texture = ctx.load_texture("mask-overlay", image, egui::TextureOptions::NEAREST);
    app.mask_overlay = Some(texture.clone());
    app.mask_overlay_key = Some(key);
    Some(texture)
}

fn histogram(app: &App, painter: &egui::Painter, rect: egui::Rect) {
    let Some(bins) = &app.histogram else { return };
    let area = egui::Rect::from_min_size(
        rect.right_top() + egui::vec2(-(196.0 + SP_M), SP_M),
        egui::vec2(196.0, 84.0),
    );
    painter.rect_filled(area, theme::RADIUS, Color32::from_black_alpha(150));
    painter.rect_stroke(
        area,
        theme::RADIUS,
        egui::Stroke::new(1.0_f32, color::STROKE),
    );
    let peak = bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    let inner = area.shrink2(egui::vec2(SP_S, SP_S));
    let step = inner.width() / bins.len() as f32;
    for (index, value) in bins.iter().enumerate() {
        let height = (inner.height() * (*value as f32 / peak).sqrt()).max(0.5);
        let x = inner.left() + index as f32 * step;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x, inner.bottom() - height),
            egui::pos2(x + step * 0.8, inner.bottom()),
        );
        painter.rect_filled(bar, 0.0, color::TEXT_DIM.gamma_multiply(0.9));
    }
}

fn footer(app: &App, painter: &egui::Painter, rect: egui::Rect) {
    let mut parts = vec![format!("{} frames", app.included_count())];
    parts.push(app.method.label().to_owned());
    if let Some(preview) = &app.preview {
        parts.push(format!("{}×{}", preview.image.width, preview.image.height));
        parts.push(format!("{:.2} EV", preview.exposure_ev));
    }
    if !app.view_fit {
        parts.push(format!("{:.0}%", app.view_zoom * 100.0));
    }
    if let Some((x, y, rgb)) = app.inspect {
        parts.push(format!(
            "x{x} y{y}  rgb {:.2} {:.2} {:.2}",
            rgb[0], rgb[1], rgb[2]
        ));
    }
    let text = parts.join("  ·  ");
    let position = rect.left_bottom() + egui::vec2(SP_M, -SP_M);
    painter.text(
        position,
        egui::Align2::LEFT_BOTTOM,
        text,
        egui::FontId::proportional(11.0),
        color::TEXT_DIM,
    );
}

fn empty_state(app: &mut App, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, color::BG);

    let dropping = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
    let card_size = egui::vec2(rect.width().min(620.0), 336.0);
    let card = egui::Rect::from_center_size(rect.center(), card_size);
    painter.rect_filled(
        card,
        theme::RADIUS * 2.0,
        if dropping {
            color::ACCENT_DIM.gamma_multiply(0.35)
        } else {
            color::PANEL
        },
    );
    painter.rect_stroke(
        card,
        theme::RADIUS * 2.0,
        egui::Stroke::new(
            1.5_f32,
            if dropping {
                color::ACCENT
            } else {
                color::STROKE
            },
        ),
    );

    let inner = card.shrink(SP_XL);
    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(SP_S);
            ui.label(RichText::new("⤓").size(34.0).color(if dropping {
                color::ACCENT
            } else {
                color::TEXT_DIM
            }));
            ui.add_space(SP_S);
            ui.label(
                RichText::new(if dropping {
                    "Release to load"
                } else {
                    "Drop frames here"
                })
                .size(19.0)
                .strong(),
            );
            ui.add_space(SP_XS);
            ui.label(
                RichText::new("a folder, or a batch of files — tiff, png, jpeg, exr")
                    .size(12.5)
                    .color(color::TEXT_DIM),
            );
            ui.add_space(SP_L);

            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut app.add_path)
                        .hint_text("/path/to/frames")
                        .desired_width(360.0),
                );
                if theme::primary_button(ui, "Add folder").clicked() {
                    let path = std::path::PathBuf::from(app.add_path.trim());
                    if path.exists() {
                        app.add_paths(vec![path]);
                    } else {
                        app.set_status(StatusLevel::Error, "that path does not exist");
                    }
                }
            });
            ui.add_space(SP_M);
            ui.label(
                RichText::new("Light-paint set: one exposure, many flash positions, one tripod.")
                    .size(11.0)
                    .color(color::TEXT_DIM),
            );
            ui.label(
                RichText::new("RAW must be converted to 16-bit linear TIFF first (dcraw -4 -T).")
                    .size(11.0)
                    .color(color::TEXT_DIM),
            );
            ui.add_space(SP_M);
        });
    });
}

pub fn filmstrip(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::horizontal()
        .id_salt("filmstrip")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for index in 0..app.frames.len() {
                    let selected = index == app.selected;
                    let masked = app.frames[index].masked();
                    let name = app.frames[index].name();
                    let short: String = name
                        .chars()
                        .rev()
                        .take(9)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let text = RichText::new(format!("{:02}  {short}", index + 1)).size(11.0);
                    let mut button = egui::Button::new(text)
                        .fill(if selected {
                            color::ACCENT_DIM
                        } else {
                            color::CARD
                        })
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if selected {
                                color::ACCENT
                            } else {
                                color::STROKE
                            },
                        ))
                        .min_size(egui::vec2(104.0, 46.0));
                    if masked {
                        button = button.fill(if selected {
                            color::ACCENT_DIM
                        } else {
                            color::CARD_HOVER
                        });
                    }
                    if ui.add(button).clicked() {
                        app.selected = index;
                    }
                    if masked {
                        ui.label(RichText::new("●").size(8.0).color(color::ACCENT));
                    }
                }
                ui.add_space(SP_S);
            });
        });
}
