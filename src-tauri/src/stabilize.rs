//! Video stabilization: FFmpeg two-pass vid.stab (libvidstab is in the bundled BtbN GPL build).
//!
//! Pass 1 `vidstabdetect` writes camera motion to a .trf file in a scratch folder; pass 2
//! `vidstabtransform` smooths it, zooms just enough to hide moving borders (optzoom=1),
//! resamples bicubically and sharpens lightly. Audio is kept. The result is a new file in
//! the app's local data folder (%LOCALAPPDATA%\app.cutdeck.editor\stabilized) that the editor
//! swaps in for the clip, so trims, speed and captions keep working on the same timeline.

use crate::tools::{cmd, probe, Tools};
use serde::Serialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct Stabilizer {
    job: Arc<Mutex<Option<Child>>>,
    cancel: Arc<AtomicBool>,
}

impl Stabilizer {
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(c) = self.job.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
}

/// (shakiness, smoothing frames) per strength
pub fn params(strength: &str) -> (u32, u32) {
    match strength {
        "low" => (5, 15),
        "high" => (10, 60),
        _ => (8, 30),
    }
}

pub fn detect_filter(strength: &str) -> String {
    let (shake, _) = params(strength);
    format!("vidstabdetect=shakiness={shake}:accuracy=15:stepsize=6:mincontrast=0.25:result=motion.trf")
}

pub fn transform_filter(strength: &str) -> String {
    let (_, smooth) = params(strength);
    format!("vidstabtransform=input=motion.trf:smoothing={smooth}:optalgo=gauss:zoom=0:optzoom=1:zoomspeed=0.25:interpol=bicubic:crop=black,unsharp=5:5:0.8:3:3:0.4,format=yuv420p")
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    path: String,
    phase: String,
    fraction: f64,
    eta: f64,
}

fn out_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("stabilized");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn key(path: &str, strength: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    strength.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        m.modified().ok().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// Run one FFmpeg pass in `work`, reporting progress mapped into [lo, hi].
#[allow(clippy::too_many_arguments)]
fn pass(st: &Stabilizer, t: &Tools, work: &Path, args: &[String], dur: f64, mut on: impl FnMut(f64)) -> Result<(), String> {
    let mut child = cmd(&t.ffmpeg)
        .current_dir(work)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let mut err = child.stderr.take().unwrap();
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    });
    let out = child.stdout.take().unwrap();
    *st.job.lock().unwrap() = Some(child);
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
            if let Ok(us) = v.trim().parse::<f64>() {
                on((us / 1e6 / dur.max(0.01)).clamp(0.0, 1.0));
            }
        }
    }
    let status = st.job.lock().unwrap().take().map(|mut c| c.wait());
    let tail = err_t.join().unwrap_or_default();
    if st.cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    match status {
        Some(Ok(s)) if s.success() => Ok(()),
        _ => Err(if tail.trim().is_empty() { "FFmpeg stopped unexpectedly.".into() } else { tail }),
    }
}

/// Stabilize a whole source file; returns the path of the stabilized copy (cached per strength).
pub fn run(app: &AppHandle, t: &Tools, work: &Path, path: &str, strength: &str) -> Result<String, String> {
    let st = app.state::<Stabilizer>();
    st.cancel.store(false, Ordering::SeqCst);
    let info = probe(t, path)?;
    if !info.has_video {
        return Err("This file has no video to stabilize.".into());
    }
    let dir = out_dir(app)?;
    let out = dir.join(format!("{}-{strength}.mp4", key(path, strength)));
    if out.is_file() {
        return Ok(out.display().to_string());
    }
    let part = dir.join(format!("{}-{strength}.part.mp4", key(path, strength)));
    let dur = info.duration.max(0.01);
    let started = Instant::now();
    let emit = |phase: &str, f: f64| {
        let el = started.elapsed().as_secs_f64();
        let _ = app.emit("stabilize-progress", Progress { path: path.into(), phase: phase.into(), fraction: f, eta: if f > 0.02 { el / f * (1.0 - f) } else { -1.0 } });
    };
    emit("analyze", 0.0);
    let common = ["-hide_banner", "-y", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1"];
    // pass 1: motion analysis (35% of the time)
    let mut a: Vec<String> = common.iter().map(|s| s.to_string()).collect();
    a.extend(["-i".into(), path.into(), "-map".into(), "0:v:0".into(), "-vf".into(), detect_filter(strength), "-f".into(), "null".into(), "-".into()]);
    pass(&st, t, work, &a, dur, |f| emit("analyze", 0.35 * f)).map_err(|e| if e == "cancelled" { e } else { format!("Motion analysis failed: {e}") })?;
    if !work.join("motion.trf").is_file() {
        return Err("Motion analysis wrote no data.".into());
    }
    // pass 2: smooth + render, high quality H.264, audio kept
    let mut b: Vec<String> = common.iter().map(|s| s.to_string()).collect();
    b.extend(["-i".into(), path.into(), "-map".into(), "0:v:0".into()]);
    if info.has_audio {
        b.extend(["-map".into(), "0:a:0".into(), "-c:a".into(), "aac".into(), "-b:a".into(), "256k".into()]);
    }
    b.extend([
        "-vf".into(), transform_filter(strength),
        "-c:v".into(), "libx264".into(), "-preset".into(), "medium".into(), "-crf".into(), "17".into(),
        "-movflags".into(), "+faststart".into(), part.display().to_string(),
    ]);
    let r = pass(&st, t, work, &b, dur, |f| emit("render", 0.35 + 0.65 * f));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(if e == "cancelled" { e } else { format!("Stabilizing failed: {e}") });
    }
    std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
    emit("done", 1.0);
    Ok(out.display().to_string())
}
