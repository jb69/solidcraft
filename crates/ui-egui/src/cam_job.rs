//! Toolpath generation off the window thread. The Generate buttons and menus start a job: the
//! window thread prepares it (geometry resolution reads the session), a worker computes the
//! toolpaths operation by operation, and each frame the finished ones are taken into the
//! session while the browser animates the operation being computed. On the web, which has no
//! threads, the job runs at once.

use serde_json::Value;
use solidcraft_engine::cmd::cam::{GenerateEvent, accept_generated, prepare_generate};

use crate::SolveApp;

/// A generation in progress.
pub struct CamJob {
    rx: std::sync::mpsc::Receiver<GenerateEvent>,
    /// The operation being computed now.
    pub current: Option<u64>,
    /// The operations still to compute, in order.
    pub queued: Vec<u64>,
    /// When it started (ms).
    pub started: f64,
}

/// An operation's part in the running job, if any: `generating` or `queued`.
pub fn state(app: &SolveApp, op: u64) -> Option<&'static str> {
    let job = app.cam_job.as_ref()?;
    if job.current == Some(op) {
        Some("generating")
    } else if job.queued.contains(&op) {
        Some("queued")
    } else {
        None
    }
}

/// Starts generating (`cam.generate`'s parameters). A second start while one runs is refused.
pub fn start(app: &mut SolveApp, params: Value) {
    if app.cam_job.is_some() {
        app.set_status("toolpaths are already being generated", true);
        return;
    }
    let job = match prepare_generate(&app.session, &params) {
        Ok(job) => job,
        Err(e) => {
            app.set_status(e.to_string(), true);
            return;
        }
    };
    let queued = job.operations.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    // Nothing to compute (every toolpath is current): report at once, here.
    if queued.is_empty() {
        job.run(|e| drop(tx.send(e)));
        app.cam_job = Some(CamJob { rx, current: None, queued, started: crate::now_ms() });
        drain(app);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let spawned = std::thread::Builder::new().name("toolpaths".into()).spawn(move || {
            let tx2 = tx.clone();
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || job.run(|e| drop(tx.send(e)))));
            if ran.is_err() {
                let _ = tx2.send(GenerateEvent::Finished(serde_json::json!({"status": "error", "error": "toolpath generation stopped unexpectedly"})));
            }
        });
        if let Err(e) = spawned {
            app.set_status(format!("could not start toolpath generation: {e}"), true);
            return;
        }
    }
    #[cfg(target_arch = "wasm32")]
    job.run(|e| drop(tx.send(e)));
    app.cam_job = Some(CamJob { rx, current: None, queued, started: crate::now_ms() });
}

/// Takes in what the job finished since the last frame; keeps frames coming while it runs.
pub fn poll(app: &mut SolveApp, ctx: &egui::Context) {
    if app.cam_job.is_some() && !drain(app) {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// Takes in the job's progress; true when it has finished.
fn drain(app: &mut SolveApp) -> bool {
    let Some(job) = app.cam_job.as_mut() else { return true };
    let mut finished: Option<Value> = None;
    let mut done = Vec::new();
    loop {
        match job.rx.try_recv() {
            Ok(GenerateEvent::Started(id)) => {
                job.current = Some(id);
                job.queued.retain(|q| *q != id);
            }
            Ok(GenerateEvent::Done(id, g)) => {
                if job.current == Some(id) {
                    job.current = None;
                }
                done.push((id, g));
            }
            Ok(GenerateEvent::Finished(v)) => {
                finished = Some(v);
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                finished = Some(serde_json::json!({"status": "error", "error": "toolpath generation stopped unexpectedly"}));
                break;
            }
        }
    }
    for (id, g) in done {
        accept_generated(&mut app.session, id, g);
    }
    match finished {
        Some(v) => {
            app.cam_job = None;
            let regenerated = v["regenerated"].as_u64().unwrap_or(0);
            let reused = v["reused"].as_u64().unwrap_or(0);
            let status = v["status"].as_str().unwrap_or("ok");
            let text = match v["error"].as_str() {
                Some(e) => e.to_string(),
                None => format!("toolpaths: {regenerated} generated, {reused} unchanged ({status})"),
            };
            app.set_status(text, status == "error");
            true
        }
        None => false,
    }
}

/// A spinning arc in `rect`, `time` seconds into the animation.
pub fn paint_spinner(painter: &egui::Painter, center: egui::Pos2, radius: f32, time: f64, color: egui::Color32) {
    let start = (time * 4.0) as f32;
    let n = 16;
    let points: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = start + i as f32 / n as f32 * std::f32::consts::PI * 1.4;
            center + egui::vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.8, color)));
}
