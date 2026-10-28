//! The machining simulation panel: shows while a `cam.simulate.show` preview is up. A timeline
//! slider, play and pause, speed, single steps and where the program stands; every change runs
//! `cam.simulate.show`, so the viewport's stock, tool and path follow. Closing runs
//! `cam.simulate.hide`.

use egui::RichText;
use serde_json::json;

use crate::SolveApp;
use crate::theme::Tokens;

/// Playback state, kept in egui's frame data (not part of the design).
#[derive(Clone, Copy, Debug, Default)]
struct Playback {
    playing: bool,
    /// Index into [`SPEEDS`].
    speed: usize,
    /// The slider's value, 0..1 of the program's time.
    progress: f64,
}

const SPEEDS: [f64; 6] = [1.0, 4.0, 16.0, 64.0, 256.0, 1024.0];

fn state_id() -> egui::Id {
    egui::Id::new("sc_cam_sim_playback")
}

pub fn show(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(preview) = app.session.cam.preview.clone() else {
        ctx.data_mut(|d| d.remove::<Playback>(state_id()));
        return;
    };
    let t = Tokens::get();
    let mut pb: Playback = ctx.data(|d| d.get_temp(state_id())).unwrap_or(Playback { progress: preview.progress, ..Default::default() });
    let mut open = true;
    let mut want: Option<serde_json::Value> = None;
    let setup = preview.setup;
    let title = RichText::new(crate::i18n::tr_label("Machining Simulation")).strong().size(t.font_button);
    crate::frame::window(ctx, title, crate::frame::Width::Normal).id(egui::Id::new("sc_cam_sim")).open(&mut open).resizable(false).show(ctx, |ui| {
        let op: String = preview.operation.as_ref().map_or_else(|| crate::i18n::tr_label("no operation").to_string(), |(_, n)| n.clone());
        crate::ui_kit::caption(ui, "Operation", &format!("{op}  ({} / {} moves)", preview.move_index + 1, preview.total_moves));
        crate::ui_kit::caption(ui, "Time", &format!("{:.0} s / {:.0} s", preview.time_s, preview.total_time_s));
        ui.add_space(t.space_8);
        let mut progress = pb.progress;
        let resp =
            crate::ui_kit::slider(ui, &mut progress, 0.0..=1.0, crate::ui_kit::Slider { show_value: false, integer: false, ..Default::default() });
        if resp.dragged() || resp.changed() {
            pb.playing = false;
        }
        if (progress - pb.progress).abs() > 1e-9 {
            pb.progress = progress;
            want = Some(json!({"setup": setup, "progress": progress}));
        }
        ui.add_space(t.space_8);
        ui.horizontal(|ui| {
            let label = if pb.playing { "Pause" } else { "Play" };
            if crate::ui_kit::button(ui, crate::i18n::tr_label(label), crate::ui_kit::Button::Small).clicked() {
                if !pb.playing && pb.progress >= 1.0 - 1e-9 {
                    pb.progress = 0.0;
                    want = Some(json!({"setup": setup, "progress": 0.0}));
                }
                pb.playing = !pb.playing;
            }
            if crate::ui_kit::button(ui, crate::i18n::tr_label("◂ Move"), crate::ui_kit::Button::Small)
                .on_hover_text(crate::i18n::tr_label("Back one move"))
                .clicked()
            {
                pb.playing = false;
                want = Some(json!({"setup": setup, "step": -1}));
            }
            if crate::ui_kit::button(ui, crate::i18n::tr_label("Move ▸"), crate::ui_kit::Button::Small)
                .on_hover_text(crate::i18n::tr_label("On one move"))
                .clicked()
            {
                pb.playing = false;
                want = Some(json!({"setup": setup, "step": 1}));
            }
            ui.separator();
            let labels = ["1×", "4×", "16×", "64×", "256×", "1024×"];
            let width = crate::ui_kit::ContentWidth::button(ui, "1024×") * 6.0;
            ui.allocate_ui(egui::vec2(width, crate::ui_kit::row_height()), |ui| {
                crate::ui_kit::segmented(ui, &mut pb.speed, &labels, Default::default());
            });
        });
        if pb.playing {
            let dt = f64::from(ui.input(|i| i.stable_dt).min(0.1));
            let speed = SPEEDS.get(pb.speed).copied().unwrap_or(1.0);
            let total = preview.total_time_s.max(1e-6);
            let next = (pb.progress + dt * speed / total).min(1.0);
            if next >= 1.0 {
                pb.playing = false;
            }
            pb.progress = next;
            want = Some(json!({"setup": setup, "progress": next}));
            ui.ctx().request_repaint();
        }
    });
    if !open {
        let _ = app.run("cam.simulate.hide", json!({}));
        ctx.data_mut(|d| d.remove::<Playback>(state_id()));
        return;
    }
    if let Some(params) = want {
        // Play and the slider keep their own clock; a single-move step takes where the step
        // landed, so the slider follows it.
        let stepped = params.get("step").is_some();
        match app.run("cam.simulate.show", params) {
            Ok(v) => {
                if stepped && let Some(p) = v.get("progress").and_then(serde_json::Value::as_f64) {
                    pb.progress = p;
                }
            }
            Err(e) => {
                pb.playing = false;
                app.set_status(e, true);
            }
        }
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), pb));
}
