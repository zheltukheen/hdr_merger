//! The left rail: sources, stack, masks, tone, export.
//!
//! Each section is built from `theme` primitives only, so spacing and control
//! order stay identical everywhere.

use eframe::egui;
use egui::RichText;
use hdr_core::io::InputEncoding;

use crate::app::{App, CurveChoice, MethodChoice, Preset, SortBy, Tool, PROXY_CHOICES};
use crate::meta;
use crate::theme::{self, color, SP_M, SP_S, SP_XS};

pub fn left_panel(app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(SP_XS);
            sources(app, ui);
            stack(app, ui);
            masks(app, ui);
            tone(app, ui);
            export(app, ui);
            ui.add_space(theme::SP_XL);
        });
}

fn sources(app: &mut App, ui: &mut egui::Ui) {
    theme::step_header(ui, 1, "Sources", &format!("{} frames", app.frames.len()));
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.recipe_path)
                .hint_text("recipe .hdrm.json")
                .desired_width(f32::INFINITY),
        );
    });
    ui.horizontal(|ui| {
        if theme::ghost_button(ui, "Save recipe").clicked() {
            app.save_recipe();
        }
        if theme::ghost_button(ui, "Load").clicked() {
            app.load_recipe();
        }
        if theme::ghost_button(ui, "Clear all").clicked() {
            app.clear_frames();
        }
    });

    ui.add_space(SP_S);
    theme::control_row(ui, "Sort by", |ui| {
        egui::ComboBox::from_id_salt("sort")
            .selected_text(match app.sort_by {
                SortBy::CaptureTime => "Capture time",
                SortBy::Name => "File name",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.sort_by, SortBy::CaptureTime, "Capture time");
                ui.selectable_value(&mut app.sort_by, SortBy::Name, "File name");
            });
    });

    let total: u64 = app
        .frames
        .iter()
        .filter(|f| f.included)
        .map(|f| f.meta.bytes)
        .sum();
    theme::hint(
        ui,
        format!(
            "{} of {} frames · {} on disk",
            app.included_count(),
            app.frames.len(),
            meta::human_bytes(total)
        ),
    );

    ui.add_space(SP_S);
    let mut remove = None;
    let mut toggle = None;
    egui::ScrollArea::vertical()
        .id_salt("frames")
        .max_height(228.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if app.frames.is_empty() {
                theme::hint(ui, "No frames yet.");
            }
            for index in 0..app.frames.len() {
                let selected = index == app.selected;
                let mut included = app.frames[index].included;
                let masked = app.frames[index].masked();
                let name = app.frames[index].name();
                let detail = app.frames[index].meta.summary();
                let response = ui
                    .horizontal(|ui| {
                        let check = ui.checkbox(&mut included, "");
                        if check.changed() {
                            toggle = Some((index, included));
                        }
                        let mut text = RichText::new(name).size(12.0);
                        if !included {
                            text = text.color(color::TEXT_DIM).strikethrough();
                        } else if selected {
                            text = text.color(color::TEXT);
                        }
                        if ui.selectable_label(selected, text).clicked() {
                            app.selected = index;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, "×", "Remove frame").clicked() {
                                remove = Some(index);
                            }
                            if masked {
                                theme::badge(ui, "mask", color::ACCENT);
                            }
                            ui.label(RichText::new(detail).size(10.0).color(color::TEXT_DIM));
                        });
                    })
                    .response;
                let frame = &app.frames[index];
                let mut tip = frame.path.display().to_string();
                for part in [
                    &frame.meta.camera,
                    &frame.meta.lens,
                    &frame.meta.exposure,
                    &frame.meta.iso,
                    &frame.meta.aperture,
                    &frame.meta.taken,
                ]
                .into_iter()
                .flatten()
                {
                    tip.push('\n');
                    tip.push_str(part);
                }
                response.on_hover_text(tip);
            }
        });
    if let Some((index, value)) = toggle {
        if let Some(frame) = app.frames.get_mut(index) {
            frame.included = value;
        }
    }
    if let Some(index) = remove {
        app.remove_frame(index);
    }
}

fn stack(app: &mut App, ui: &mut egui::Ui) {
    let proxy = if app.proxy == 0 {
        "full".to_owned()
    } else {
        format!("{} px", app.proxy)
    };
    theme::step_header(
        ui,
        2,
        "Stack",
        &format!("{} · {}", app.method.label(), proxy),
    );

    for choice in [
        MethodChoice::Focus,
        MethodChoice::Median,
        MethodChoice::Percentile,
        MethodChoice::TrimmedMax,
        MethodChoice::Mean,
        MethodChoice::Sum,
        MethodChoice::Max,
    ] {
        let selected = app.method == choice;
        let response = ui.add_sized(
            [ui.available_width(), 24.0],
            egui::SelectableLabel::new(selected, RichText::new(choice.label()).size(12.5)),
        );
        if response.clicked() {
            app.method = choice;
        }
    }
    ui.add_space(SP_XS);
    theme::hint(ui, app.method.hint());

    if app.method == MethodChoice::Percentile {
        ui.add_space(SP_XS);
        theme::control_row(ui, "Percentile", |ui| {
            theme::slider(ui, &mut app.percentile, 1.0..=99.0, "p");
        });
    }
    if app.method == MethodChoice::TrimmedMax {
        ui.add_space(SP_XS);
        theme::control_row(ui, "Drop top", |ui| {
            theme::int_slider(ui, &mut app.trimmed, 0..=20, "");
        });
    }

    ui.add_space(SP_S);
    theme::control_row(ui, "Reference", |ui| {
        let label = app
            .frames
            .get(app.reference)
            .map(|f| f.name())
            .unwrap_or_else(|| "first".to_owned());
        egui::ComboBox::from_id_salt("reference")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (index, frame) in app.frames.iter().enumerate().take(64) {
                    ui.selectable_value(&mut app.reference, index, frame.name());
                }
            });
    });
    theme::control_row(ui, "Align", |ui| {
        ui.checkbox(&mut app.align, "translation");
    });
    theme::control_row(ui, "Shift guard", |ui| {
        ui.checkbox(&mut app.shift_guard, "");
        ui.add_enabled_ui(app.shift_guard, |ui| {
            theme::int_slider(ui, &mut app.max_shift, 1..=400, " px");
        });
    });
    theme::control_row(ui, "Crop edges", |ui| {
        ui.checkbox(&mut app.crop, "to intersection");
    });
    theme::control_row(ui, "Preview proxy", |ui| {
        egui::ComboBox::from_id_salt("proxy")
            .selected_text(if app.proxy == 0 {
                "Full".to_owned()
            } else {
                format!("{} px", app.proxy)
            })
            .show_ui(ui, |ui| {
                for choice in PROXY_CHOICES {
                    let label = if choice == 0 {
                        "Full".to_owned()
                    } else {
                        format!("{choice} px")
                    };
                    ui.selectable_value(&mut app.proxy, choice, label);
                }
            });
    });
    theme::control_row(ui, "Input", |ui| {
        egui::ComboBox::from_id_salt("encoding")
            .selected_text(match app.input_encoding {
                InputEncoding::Auto => "Auto",
                InputEncoding::Srgb => "sRGB",
                InputEncoding::Linear => "Linear",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.input_encoding, InputEncoding::Auto, "Auto");
                ui.selectable_value(&mut app.input_encoding, InputEncoding::Srgb, "sRGB");
                ui.selectable_value(&mut app.input_encoding, InputEncoding::Linear, "Linear");
            });
    });
}

fn masks(app: &mut App, ui: &mut egui::Ui) {
    theme::step_header(ui, 3, "Masks", &format!("{} masked", app.masked_frames()));
    ui.horizontal(|ui| {
        for (tool, label) in [
            (Tool::Inspect, "Inspect"),
            (Tool::Brush, "Brush"),
            (Tool::Eraser, "Eraser"),
        ] {
            let selected = app.tool == tool;
            if ui.selectable_label(selected, label).clicked() {
                app.tool = tool;
            }
        }
    });
    theme::hint(
        ui,
        "Paint on the preview to drop that frame from the stack there.",
    );
    theme::control_row(ui, "Brush size", |ui| {
        theme::slider(ui, &mut app.brush, 2.0..=300.0, " px");
    });
    theme::control_row(ui, "Softness", |ui| {
        theme::slider(ui, &mut app.softness, 0.0..=1.0, "");
    });
    theme::control_row(ui, "Overlay", |ui| {
        ui.checkbox(&mut app.overlay, "show excluded pixels");
    });
    ui.add_space(SP_S);
    ui.horizontal(|ui| {
        if theme::ghost_button(ui, "Clear frame").clicked() {
            app.clear_mask(app.selected);
        }
        if theme::ghost_button(ui, "Clear all").clicked() {
            app.clear_all_masks();
        }
    });
    theme::hint(ui, format!("{} frame(s) masked", app.masked_frames()));
    theme::divider(ui);
    theme::hint(ui, "Auto-detect the moving subject on proxy copies.");
    theme::control_row(ui, "Threshold", |ui| {
        theme::slider(ui, &mut app.deghost_threshold, 0.02..=0.6, "");
    });
    if theme::ghost_button(ui, "Detect movers").clicked() {
        app.start_deghost();
    }
}

fn tone(app: &mut App, ui: &mut egui::Ui) {
    let exposure = app
        .preview
        .as_ref()
        .map(|preview| format!("{:.1} EV", preview.exposure_ev))
        .unwrap_or_else(|| "auto".to_owned());
    theme::step_header(
        ui,
        4,
        "Tone map",
        &format!("{} · {exposure}", app.curve.label()),
    );
    theme::hint(ui, "Display only. The EXR master stays linear.");

    theme::control_row(ui, "Preset", |ui| {
        egui::ComboBox::from_id_salt("preset")
            .selected_text(app.active_preset().label())
            .show_ui(ui, |ui| {
                for preset in [
                    Preset::Neutral,
                    Preset::LightPaintInterior,
                    Preset::NightExterior,
                    Preset::BracketHdr,
                ] {
                    if ui
                        .selectable_label(app.active_preset() == preset, preset.label())
                        .clicked()
                    {
                        app.apply_preset(preset);
                    }
                }
            });
    });

    theme::control_row(ui, "Auto exposure", |ui| {
        ui.checkbox(&mut app.auto_exposure, "");
    });
    if app.auto_exposure {
        theme::control_row(ui, "Target", |ui| {
            theme::slider(ui, &mut app.target, 0.2..=0.9, "");
        });
    } else {
        theme::control_row(ui, "Exposure", |ui| {
            theme::slider(ui, &mut app.exposure_ev, -8.0..=8.0, " EV");
        });
    }
    theme::control_row(ui, "Curve", |ui| {
        egui::ComboBox::from_id_salt("curve")
            .selected_text(app.curve.label())
            .show_ui(ui, |ui| {
                for choice in [
                    CurveChoice::Aces,
                    CurveChoice::Reinhard,
                    CurveChoice::Reinhard02,
                    CurveChoice::Hable,
                    CurveChoice::None,
                ] {
                    ui.selectable_value(&mut app.curve, choice, choice.label());
                }
            });
    });

    theme::divider(ui);
    theme::hint(ui, "Levels");
    theme::control_row(ui, "Black point", |ui| {
        theme::slider(ui, &mut app.tone.black_point, 0.0..=0.5, "");
    });
    theme::control_row(ui, "White point", |ui| {
        theme::slider(ui, &mut app.tone.white_point, 0.1..=4.0, "");
    });
    theme::control_row(ui, "Contrast", |ui| {
        theme::slider(ui, &mut app.tone.contrast, -1.0..=1.0, "");
    });
    theme::control_row(ui, "Highlights", |ui| {
        theme::slider(ui, &mut app.tone.highlights, -1.0..=1.0, "");
    });
    theme::control_row(ui, "Shadows", |ui| {
        theme::slider(ui, &mut app.tone.shadows, -1.0..=1.0, "");
    });

    theme::divider(ui);
    theme::hint(ui, "Colour");
    theme::control_row(ui, "Saturation", |ui| {
        theme::slider(ui, &mut app.tone.saturation, -1.0..=1.0, "");
    });
    theme::control_row(ui, "Temperature", |ui| {
        theme::slider(ui, &mut app.tone.temperature, -1.0..=1.0, "");
    });
    theme::control_row(ui, "Tint", |ui| {
        theme::slider(ui, &mut app.tone.tint, -1.0..=1.0, "");
    });

    theme::divider(ui);
    theme::hint(ui, "Detail");
    theme::control_row(ui, "Structure", |ui| {
        theme::slider(ui, &mut app.tone.structure, 0.0..=1.0, "");
    });
    theme::control_row(ui, "Local lift", |ui| {
        theme::slider(ui, &mut app.tone.local, 0.0..=1.0, "");
    });
    theme::control_row(ui, "Denoise", |ui| {
        theme::slider(ui, &mut app.tone.denoise, 0.0..=1.0, "");
    });
    theme::control_row(ui, "Vignette", |ui| {
        theme::slider(ui, &mut app.tone.vignette, -1.0..=1.0, "");
    });

    if let Some(preview) = &app.preview {
        theme::hint(
            ui,
            format!("applied exposure {:.2} EV", preview.exposure_ev),
        );
    }
    ui.add_space(SP_S);
    if theme::ghost_button(ui, "Reset tone").clicked() {
        app.auto_exposure = true;
        app.exposure_ev = 0.0;
        app.curve = CurveChoice::Aces;
        app.target = 0.55;
        app.apply_preset(Preset::Neutral);
    }
}

fn export(app: &mut App, ui: &mut egui::Ui) {
    let format = std::path::Path::new(app.out_path.trim())
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_uppercase())
        .unwrap_or_else(|| "EXR".to_owned());
    theme::step_header(ui, 5, "Export", &format);

    ui.add(
        egui::TextEdit::singleline(&mut app.out_path)
            .hint_text("out.exr / out.tif / out.jpg")
            .desired_width(f32::INFINITY),
    );
    theme::control_row(ui, "JPEG quality", |ui| {
        let mut quality = app.jpeg_quality as f32;
        theme::slider(ui, &mut quality, 1.0..=100.0, "");
        app.jpeg_quality = quality.clamp(1.0, 100.0) as u8;
    });
    theme::control_row(ui, "Resize", |ui| {
        ui.checkbox(&mut app.out_edge_on, "long edge");
        ui.add_enabled_ui(app.out_edge_on, |ui| {
            theme::int_slider(ui, &mut app.out_edge, 200..=12000, " px");
        });
    });

    ui.add_space(SP_M);
    ui.horizontal(|ui| {
        let preview = ui.add_enabled(
            !app.busy(),
            egui::Button::new(
                RichText::new("Preview")
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(color::CARD)
            .stroke(egui::Stroke::new(1.0_f32, color::STROKE))
            .min_size(egui::Vec2::new(0.0, 30.0)),
        );
        if preview.clicked() {
            app.start_preview();
        }
        let export = ui.add_enabled(
            !app.busy(),
            egui::Button::new(
                RichText::new("Export full")
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(color::ACCENT_DIM)
            .stroke(egui::Stroke::new(1.0_f32, color::ACCENT))
            .min_size(egui::Vec2::new(0.0, 30.0)),
        );
        if export.clicked() {
            app.start_export();
        }
    });

    if let Some(preview) = &app.preview {
        ui.add_space(SP_S);
        let megapixels = preview.image.pixel_count() as f32 / 1_000_000.0;
        theme::hint(
            ui,
            format!(
                "preview {}×{} ({:.1} MP) · {} frames used",
                preview.image.width, preview.image.height, megapixels, preview.used
            ),
        );
        theme::hint(
            ui,
            format!(
                "decode {} ms · cache {} ms · stack {} ms",
                preview.timings.decode_ms, preview.timings.cache_ms, preview.timings.stack_ms
            ),
        );
        for warning in preview.warnings.iter().take(3) {
            ui.label(RichText::new(warning).size(10.5).color(color::WARN));
        }
    }
}
