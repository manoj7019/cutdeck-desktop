//! Auto captions: offline speech-to-text with whisper.cpp.
//!
//! The official `whisper-cli` build ships next to the app (resources/whisper). Models are
//! downloaded on first use from Hugging Face into the app's local data folder
//! (%LOCALAPPDATA%\app.cutdeck.editor\whisper on Windows), so the installer stays small.
//! FFmpeg cuts the used part of each clip to 16 kHz mono WAV, Silero VAD skips silence,
//! and whisper returns word-level timestamps that the editor groups into caption lines.

use crate::tools::{cmd, Tools};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

const HF_MODELS: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";
const HF_VAD: &str = "https://huggingface.co/ggml-org/whisper-vad/resolve/main/";

/// (quality key, file, exact size in bytes, label)
pub const MODELS: &[(&str, &str, u64, &str)] = &[
    ("fast", "ggml-small-q5_1.bin", 190_085_487, "Fast (Whisper small)"),
    ("accurate", "ggml-large-v3-turbo-q5_0.bin", 574_041_195, "Accurate (Whisper large-v3 turbo)"),
];
const VAD_FILE: &str = "ggml-silero-v5.1.2.bin";
const VAD_SIZE: u64 = 885_098;

/// Mixed-script prompt: nudges Whisper to keep English words in Latin script inside Hindi speech.
const HI_PROMPT: &str = "नमस्ते दोस्तों, आज हम video editing और office meeting के बारे में बात करेंगे।";

pub struct Captions {
    engine_dir: PathBuf,
    job: Arc<Mutex<Option<Child>>>,
    cancel: Arc<AtomicBool>,
}

impl Captions {
    pub fn new(engine_dir: PathBuf) -> Self {
        Captions { engine_dir, job: Arc::new(Mutex::new(None)), cancel: Arc::new(AtomicBool::new(false)) }
    }
    fn cli(&self) -> PathBuf {
        self.engine_dir.join(if cfg!(windows) { "whisper-cli.exe" } else { "whisper-cli" })
    }
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(c) = self.job.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
}

fn model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().app_local_data_dir().map_err(|e| e.to_string())?.join("whisper");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn have(p: &Path, size: u64) -> bool {
    std::fs::metadata(p).map(|m| m.len() == size).unwrap_or(false)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    engine: bool,
    engine_path: String,
    models: Vec<serde_json::Value>,
    folder: String,
}

pub fn status(app: &AppHandle) -> Result<Status, String> {
    let cap = app.state::<Captions>();
    let dir = model_dir(app)?;
    let vad = have(&dir.join(VAD_FILE), VAD_SIZE);
    let models = MODELS
        .iter()
        .map(|(k, f, size, label)| {
            serde_json::json!({"key": k, "label": label, "size": size, "ready": vad && have(&dir.join(f), *size)})
        })
        .collect();
    Ok(Status { engine: cap.cli().is_file(), engine_path: cap.cli().display().to_string(), models, folder: dir.display().to_string() })
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    phase: String,
    detail: String,
    fraction: f64,
    eta: f64,
    text: String,
}

fn emit(app: &AppHandle, phase: &str, detail: String, fraction: f64, eta: f64, text: String) {
    let _ = app.emit("captions-progress", Progress { phase: phase.into(), detail, fraction, eta, text });
}

/// Download one file with resume and retries; verifies the exact size.
fn fetch(app: &AppHandle, cancel: &AtomicBool, url: &str, dest: &Path, size: u64, label: &str, base: u64, all: u64, started: Instant) -> Result<(), String> {
    if have(dest, size) {
        return Ok(());
    }
    let part = dest.with_extension("part");
    let mut last_err = String::new();
    for _attempt in 0..8 {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let mut done = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if done > size {
            let _ = std::fs::remove_file(&part);
            done = 0;
        }
        if done == size {
            break;
        }
        let mut req = ureq::get(url).set("User-Agent", "Cutdeck/0.1");
        if done > 0 {
            req = req.set("Range", &format!("bytes={done}-"));
        }
        let resp = match req.call() {
            Ok(r) => r,
            Err(e) => {
                last_err = e.to_string();
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
        };
        if resp.status() == 200 {
            done = 0; // server ignored the range: start over
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(done > 0)
            .truncate(done == 0)
            .open(&part)
            .map_err(|e| e.to_string())?;
        let mut r = resp.into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut tick = Instant::now();
        let mut ok = true;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    f.write_all(&buf[..n]).map_err(|e| format!("Could not write the model: {e}"))?;
                    done += n as u64;
                    if tick.elapsed().as_millis() > 250 {
                        tick = Instant::now();
                        let got = base + done;
                        let frac = got as f64 / all.max(1) as f64;
                        let el = started.elapsed().as_secs_f64();
                        let rate = got as f64 / el.max(0.1);
                        emit(app, "download", format!("{label} · {:.0} of {:.0} MB · {:.1} MB/s", got as f64 / 1e6, all as f64 / 1e6, rate / 1e6), frac, if rate > 0.0 { (all - got.min(all)) as f64 / rate } else { -1.0 }, String::new());
                    }
                }
                Err(e) => {
                    last_err = e.to_string();
                    ok = false;
                    break;
                }
            }
        }
        drop(f);
        if ok && done >= size {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if got != size {
        return Err(format!("Download of {label} did not finish ({:.0} of {:.0} MB). Check your internet connection and try again. {last_err}", got as f64 / 1e6, size as f64 / 1e6));
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

pub fn download(app: &AppHandle, quality: &str) -> Result<(), String> {
    let cap = app.state::<Captions>();
    cap.cancel.store(false, Ordering::SeqCst);
    let (_, file, size, label) = MODELS.iter().find(|m| m.0 == quality).ok_or("Unknown caption quality")?;
    let dir = model_dir(app)?;
    let need_vad = !have(&dir.join(VAD_FILE), VAD_SIZE);
    let need_model = !have(&dir.join(file), *size);
    let all = if need_vad { VAD_SIZE } else { 0 } + if need_model { *size } else { 0 };
    let started = Instant::now();
    if need_vad {
        fetch(app, &cap.cancel, &format!("{HF_VAD}{VAD_FILE}"), &dir.join(VAD_FILE), VAD_SIZE, "Voice detector", 0, all, started)?;
    }
    if need_model {
        fetch(app, &cap.cancel, &format!("{HF_MODELS}{file}"), &dir.join(file), *size, label, if need_vad { VAD_SIZE } else { 0 }, all, started)?;
    }
    emit(app, "download", "Model ready".into(), 1.0, 0.0, String::new());
    Ok(())
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub path: String,
    /// source range to transcribe, in seconds
    pub start: f64,
    pub end: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Word {
    /// source-media seconds
    pub t0: f64,
    pub t1: f64,
    pub w: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobResult {
    path: String,
    language: String,
    words: Vec<Word>,
}

fn threads() -> usize {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (n / 2).clamp(2, 8) // physical cores; hyper-threads don't help whisper
}

/// whisper-cli JSON (-oj with -ml 1 -sow): one entry per word, offsets in ms.
pub fn parse_words(json: &[u8], shift: f64) -> (String, Vec<Word>) {
    let text = String::from_utf8_lossy(json);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let lang = v["result"]["language"].as_str().unwrap_or("").to_string();
    let mut words = vec![];
    for s in v["transcription"].as_array().cloned().unwrap_or_default() {
        let w = s["text"].as_str().unwrap_or("").trim().to_string();
        // whisper's non-speech markers like [Music] or (applause)
        if w.is_empty() || (w.starts_with('[') && w.ends_with(']')) || (w.starts_with('(') && w.ends_with(')')) {
            continue;
        }
        let t0 = s["offsets"]["from"].as_f64().unwrap_or(0.0) / 1000.0 + shift;
        let t1 = s["offsets"]["to"].as_f64().unwrap_or(0.0) / 1000.0 + shift;
        words.push(Word { t0, t1: t1.max(t0 + 0.01), w });
    }
    (lang, words)
}

fn run_logged(cap: &Captions, mut c: std::process::Command, mut on_line: impl FnMut(&str)) -> Result<String, String> {
    let mut child = c.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let err = child.stderr.take().unwrap();
    let out = child.stdout.take().unwrap();
    *cap.job.lock().unwrap() = Some(child);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx2 = tx.clone();
    let t1 = std::thread::spawn(move || {
        for l in BufReader::new(err).split(b'\n').map_while(Result::ok) {
            let _ = tx.send(String::from_utf8_lossy(&l).to_string());
        }
    });
    let t2 = std::thread::spawn(move || {
        for l in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let _ = tx2.send(String::from_utf8_lossy(&l).to_string());
        }
    });
    let mut tail: Vec<String> = vec![];
    for line in rx {
        on_line(&line);
        tail.push(line);
        if tail.len() > 15 {
            tail.remove(0);
        }
    }
    let _ = t1.join();
    let _ = t2.join();
    let status = cap.job.lock().unwrap().take().map(|mut c| c.wait());
    if cap.cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    match status {
        Some(Ok(s)) if s.success() => Ok(tail.join("\n")),
        _ => Err(tail.join("\n")),
    }
}

pub fn run(app: &AppHandle, t: &Tools, work: &Path, jobs: Vec<Job>, lang: &str, quality: &str) -> Result<Vec<JobResult>, String> {
    let cap = app.state::<Captions>();
    cap.cancel.store(false, Ordering::SeqCst);
    if !cap.cli().is_file() {
        return Err(format!("The caption engine is missing ({}). Reinstall Cutdeck.", cap.cli().display()));
    }
    let (_, file, size, _) = MODELS.iter().find(|m| m.0 == quality).ok_or("Unknown caption quality")?;
    let dir = model_dir(app)?;
    let model = dir.join(file);
    if !have(&model, *size) || !have(&dir.join(VAD_FILE), VAD_SIZE) {
        return Err("The caption model isn't downloaded yet.".into());
    }
    let wl = match lang {
        "en" => "en",
        "hi" | "hinglish" => "hi",
        _ => "auto",
    };
    let total: f64 = jobs.iter().map(|j| (j.end - j.start).max(0.0)).sum::<f64>().max(0.01);
    let started = Instant::now();
    let mut done_sec = 0.0;
    let mut out = vec![];
    for (i, j) in jobs.iter().enumerate() {
        let len = (j.end - j.start).max(0.0);
        if len < 0.3 {
            out.push(JobResult { path: j.path.clone(), language: String::new(), words: vec![] });
            continue;
        }
        let detail = if jobs.len() > 1 { format!("part {} of {}", i + 1, jobs.len()) } else { String::new() };
        emit(app, "audio", detail.clone(), done_sec / total, -1.0, String::new());
        let wav = work.join(format!("cap{i}.wav"));
        let st = cmd(&t.ffmpeg)
            .args(["-hide_banner", "-y", "-nostdin", "-loglevel", "error", "-ss", &format!("{:.3}", j.start), "-t", &format!("{:.3}", len), "-i"])
            .arg(&j.path)
            .args(["-vn", "-map", "0:a:0", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
            .arg(&wav)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;
        if !st.status.success() {
            let e = String::from_utf8_lossy(&st.stderr);
            if e.contains("matches no streams") {
                out.push(JobResult { path: j.path.clone(), language: String::new(), words: vec![] }); // clip without sound
                done_sec += len;
                continue;
            }
            return Err(format!("Could not read the sound of this clip: {}", e.trim()));
        }
        // Auto: detect first so Hindi gets the mixed-script prompt too.
        let mut use_lang = wl.to_string();
        if wl == "auto" {
            let mut c = cmd(&cap.cli());
            c.arg("-m").arg(&model).arg("-f").arg(&wav).args(["-dl", "-l", "auto", "-t", &threads().to_string(), "-np"]);
            emit(app, "detect", detail.clone(), done_sec / total, -1.0, String::new());
            let log = run_logged(&cap, c, |_| {}).unwrap_or_else(|e| e);
            if cap.cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            if let Some(p) = log.find("auto-detected language: ") {
                use_lang = log[p + 24..].chars().take_while(|c| c.is_ascii_alphabetic()).collect();
            }
            if use_lang.is_empty() {
                use_lang = "auto".into();
            }
        }
        let base = work.join(format!("cap{i}"));
        let mut c = cmd(&cap.cli());
        c.current_dir(&cap.engine_dir)
            .arg("-m").arg(&model)
            .arg("-f").arg(&wav)
            .args(["-l", &use_lang, "-t", &threads().to_string()])
            .args(["-bs", "1", "-bo", "1"]) // greedy: ~2x faster than beam search, same accuracy in tests
            .args(["-ml", "1", "-sow", "-sns", "-pp", "-oj"])
            .arg("-of").arg(&base)
            .arg("--vad").arg("-vm").arg(dir.join(VAD_FILE));
        if use_lang == "hi" {
            c.args(["--prompt", HI_PROMPT]);
        }
        let mut last_text = String::new();
        let res = run_logged(&cap, c, |line| {
            if let Some(p) = line.find("progress = ") {
                let pct: f64 = line[p + 11..].trim().trim_end_matches('%').trim().parse().unwrap_or(0.0);
                let f = (done_sec + len * pct / 100.0) / total;
                let el = started.elapsed().as_secs_f64();
                emit(app, "transcribe", detail.clone(), f, if f > 0.02 { el / f * (1.0 - f) } else { -1.0 }, last_text.clone());
            } else if line.starts_with('[') && line.contains("-->") {
                if let Some(p) = line.find(']') {
                    let w = line[p + 1..].trim();
                    if !w.is_empty() {
                        last_text.push(' ');
                        last_text.push_str(w);
                        let n = last_text.chars().count();
                        if n > 60 {
                            last_text = last_text.chars().skip(n - 60).collect();
                        }
                        emit(app, "words", String::new(), 0.0, 0.0, last_text.trim().to_string());
                    }
                }
            }
        });
        if let Err(e) = res {
            if e == "cancelled" {
                return Err(e);
            }
            return Err(format!("Transcription failed: {}", e.lines().rev().take(4).collect::<Vec<_>>().join(" | ")));
        }
        let json = std::fs::read(base.with_extension("json")).map_err(|e| format!("No transcript was written: {e}"))?;
        let (mut detected, words) = parse_words(&json, j.start);
        if detected.is_empty() {
            detected = use_lang.clone();
        }
        out.push(JobResult { path: j.path.clone(), language: detected, words });
        done_sec += len;
        let _ = std::fs::remove_file(&wav);
    }
    emit(app, "done", String::new(), 1.0, 0.0, String::new());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_word_json() {
        let j = r#"{"result":{"language":"hi"},"transcription":[
            {"offsets":{"from":0,"to":420},"text":" नमस्ते"},
            {"offsets":{"from":420,"to":900},"text":" दोस्तों,"},
            {"offsets":{"from":900,"to":1000},"text":" [Music]"},
            {"offsets":{"from":1000,"to":1500},"text":" video"}]}"#;
        let (l, w) = parse_words(j.as_bytes(), 10.0);
        assert_eq!(l, "hi");
        assert_eq!(w.len(), 3);
        assert_eq!(w[1].w, "दोस्तों,");
        assert!((w[2].t0 - 11.0).abs() < 1e-9);
    }
}
