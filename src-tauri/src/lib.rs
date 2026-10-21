mod captions;
mod cutout;
mod graph;
mod media;
mod stabilize;
mod tools;

use base64::Engine as _;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tools::{cmd, MediaInfo, Tools};

// ---------------------------------------------------------------- state

#[derive(Default)]
struct AppState {
    tools: Mutex<Option<Tools>>,
    version: Mutex<String>,
    encoder: Mutex<Option<String>>,
    job: Arc<Mutex<Option<Child>>>,
    cancel: Arc<AtomicBool>,
    /// Paths the user picked in a native dialog; only these can be written or read as text.
    allowed: Mutex<HashSet<PathBuf>>,
    counter: AtomicU64,
}

impl AppState {
    fn tools(&self) -> Result<Tools, String> {
        let mut t = self.tools.lock().unwrap();
        if t.is_none() {
            let found = tools::locate()?;
            *self.version.lock().unwrap() = tools::version(&found);
            *t = Some(found);
        }
        Ok(t.clone().unwrap())
    }
    fn encoder(&self, t: &Tools) -> String {
        let mut e = self.encoder.lock().unwrap();
        if e.is_none() {
            *e = Some(tools::detect_encoder(t));
        }
        e.clone().unwrap()
    }
    fn next_id(&self) -> u64 {
        self.counter.fetch_add(1, Ordering::SeqCst) + 1
    }
}

fn tmp_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("work");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- environment

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnvStatus {
    ok: bool,
    message: String,
    ffmpeg: String,
    version: String,
    encoder: String,
    hardware: bool,
    cutout: bool,
    cutout_message: String,
}

#[tauri::command]
async fn env_status(app: AppHandle) -> Result<EnvStatus, String> {
    blocking(move || {
        let st = app.state::<AppState>();
        match st.tools() {
            Ok(t) => {
                let enc = st.encoder(&t);
                let cut = app.state::<cutout::Cutout>().available();
                Ok(EnvStatus {
                    cutout: cut.is_ok(),
                    cutout_message: cut.err().unwrap_or_default(),
                    ok: true,
                    message: String::new(),
                    ffmpeg: t.ffmpeg.display().to_string(),
                    version: st.version.lock().unwrap().clone(),
                    hardware: enc != "libx264",
                    encoder: enc,
                })
            }
            Err(e) => Ok(EnvStatus {
                cutout: false,
                cutout_message: String::new(),
                ok: false,
                message: e,
                ffmpeg: String::new(),
                version: String::new(),
                encoder: String::new(),
                hardware: false,
            }),
        }
    })
    .await
}

#[tauri::command]
async fn probe_media(app: AppHandle, path: String) -> Result<MediaInfo, String> {
    blocking(move || {
        let t = app.state::<AppState>().tools()?;
        tools::probe(&t, &path)
    })
    .await
}

// ---------------------------------------------------------------- dialogs + files

const VIDEO_EXT: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "m2ts", "3gp", "wmv", "flv", "hevc",
];
const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp", "avif", "heic"];
const AUDIO_EXT: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma"];

fn picked(list: Option<Vec<tauri_plugin_dialog::FilePath>>) -> Vec<String> {
    list.unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
        .collect()
}

#[tauri::command]
async fn pick_files(app: AppHandle, kind: String) -> Result<Vec<String>, String> {
    let d = app.dialog().file();
    let r = match kind.as_str() {
        "audio" => {
            let mut all: Vec<&str> = AUDIO_EXT.to_vec();
            all.extend(VIDEO_EXT);
            d.set_title("Choose music").add_filter("Audio", &all).blocking_pick_file().map(|f| vec![f])
        }
        "image" => d.set_title("Choose an image").add_filter("Images", IMAGE_EXT).blocking_pick_file().map(|f| vec![f]),
        "project" => {
            let r = d.set_title("Open project").add_filter("Cutdeck project", &["cutdeck", "json"]).blocking_pick_file().map(|f| vec![f]);
            let paths = picked(r);
            let st = app.state::<AppState>();
            for p in &paths {
                st.allowed.lock().unwrap().insert(PathBuf::from(p));
            }
            return Ok(paths);
        }
        _ => {
            let mut all: Vec<&str> = VIDEO_EXT.to_vec();
            all.extend(IMAGE_EXT);
            d.set_title("Import videos and photos")
                .add_filter("Videos and photos", &all)
                .add_filter("Videos", VIDEO_EXT)
                .add_filter("Photos", IMAGE_EXT)
                .blocking_pick_files()
        }
    };
    Ok(picked(r))
}

#[tauri::command]
async fn pick_save(app: AppHandle, name: String, ext: String) -> Result<Option<String>, String> {
    // Debug builds only: automated UI tests set this to bypass the native dialog.
    #[cfg(debug_assertions)]
    if let Ok(dir) = std::env::var("CUTDECK_TEST_SAVE_DIR") {
        let p = PathBuf::from(dir).join(&name);
        app.state::<AppState>().allowed.lock().unwrap().insert(p.clone());
        return Ok(Some(p.display().to_string()));
    }
    let label = match ext.as_str() {
        "mp4" => "MP4 video",
        "png" => "PNG image",
        "cutdeck" => "Cutdeck project",
        "srt" => "SubRip subtitles",
        _ => "File",
    };
    let mut d = app.dialog().file().set_file_name(&name).add_filter(label, &[ext.as_str()]);
    if ext == "mp4" {
        if let Ok(v) = app.path().video_dir() {
            d = d.set_directory(v);
        }
    }
    let r = d.blocking_save_file().and_then(|f| f.into_path().ok());
    if let Some(p) = &r {
        app.state::<AppState>().allowed.lock().unwrap().insert(p.clone());
    }
    Ok(r.map(|p| p.display().to_string()))
}

fn check_allowed(st: &AppState, path: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(path);
    if st.allowed.lock().unwrap().contains(&p) {
        Ok(p)
    } else {
        Err("That file was not chosen in a file dialog.".into())
    }
}

#[tauri::command]
async fn write_text(state: State<'_, AppState>, path: String, text: String) -> Result<(), String> {
    let p = check_allowed(&state, &path)?;
    std::fs::write(p, text).map_err(|e| e.to_string())
}

#[tauri::command]
async fn read_text(state: State<'_, AppState>, path: String) -> Result<String, String> {
    let p = check_allowed(&state, &path)?;
    std::fs::read_to_string(p).map_err(|e| e.to_string())
}

/// Raw body = file bytes, header x-path = destination chosen in pick_save.
#[tauri::command]
async fn write_bytes(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let path = request
        .headers()
        .get("x-path")
        .and_then(|v| v.to_str().ok())
        .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string())
        .ok_or("missing x-path")?;
    let p = check_allowed(&state, &path)?;
    match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => std::fs::write(p, b).map_err(|e| e.to_string()),
        _ => Err("expected raw bytes".into()),
    }
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

/// Base URL the editor uses to load local files: http://127.0.0.1:<port>/<token>/
#[tauri::command]
fn media_base(server: State<'_, media::MediaServer>) -> String {
    format!("http://127.0.0.1:{}/{}/", server.port, server.token)
}

#[tauri::command]
fn path_exists(path: String) -> bool {
    Path::new(&path).exists()
}

// ---------------------------------------------------------------- preview proxies

/// Make a 720p H.264 copy for previewing formats the webview can't decode (e.g. HEVC on
/// some PCs). Export always uses the original file.
#[tauri::command]
async fn make_proxy(app: AppHandle, path: String) -> Result<String, String> {
    blocking(move || {
        let st = app.state::<AppState>();
        let t = st.tools()?;
        let info = tools::probe(&t, &path)?;
        let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut h);
            meta.len().hash(&mut h);
            meta.modified().ok().hash(&mut h);
            h.finish()
        };
        let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("proxies");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let out = dir.join(format!("{key:016x}.mp4"));
        if out.exists() {
            return Ok(out.display().to_string());
        }
        let part = dir.join(format!("{key:016x}.part.mp4"));
        let (w, h) = (info.width.max(2), info.height.max(2));
        let vf = if w >= h { "scale=-2:'min(720,ih)'" } else { "scale='min(720,iw)':-2" };
        let mut child = cmd(&t.ffmpeg)
            .args(["-hide_banner", "-y", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-i"])
            .arg(&path)
            .args(["-vf", vf, "-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-g", "15", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"])
            .arg(&part)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let err = drain(child.stderr.take().unwrap());
        let reader = BufReader::new(child.stdout.take().unwrap());
        for line in reader.lines().map_while(Result::ok) {
            if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
                if let Ok(us) = v.trim().parse::<f64>() {
                    let f = (us / 1e6 / info.duration.max(0.01)).clamp(0.0, 1.0);
                    let _ = app.emit("proxy-progress", serde_json::json!({"path": path, "fraction": f}));
                }
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if !status.success() {
            let _ = std::fs::remove_file(&part);
            return Err(format!("Could not convert this video: {}", err.lock().unwrap().trim()));
        }
        std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
        Ok(out.display().to_string())
    })
    .await
}

fn drain(mut r: impl Read + Send + 'static) -> Arc<Mutex<String>> {
    let out = Arc::new(Mutex::new(String::new()));
    let o = out.clone();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = r.read_to_string(&mut buf);
        // keep the tail; that's where FFmpeg puts the real error
        let tail: String = buf.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        *o.lock().unwrap() = tail;
    });
    out
}

// ---------------------------------------------------------------- AI cutout

/// Raw body = RGBA pixels, headers x-w / x-h = size. Returns a w×h 8-bit mask.
#[tauri::command]
async fn cutout_mask(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<tauri::ipc::Response, String> {
    let hv = |k: &str| -> Result<usize, String> {
        request.headers().get(k).and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).ok_or(format!("missing {k}"))
    };
    let (w, h) = (hv("x-w")?, hv("x-h")?);
    let quality = request.headers().get("x-q").and_then(|v| v.to_str().ok()).unwrap_or("fast").to_string();
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => b.clone(),
        _ => return Err("expected raw RGBA bytes".into()),
    };
    if bytes.len() != w * h * 4 {
        return Err(format!("got {} bytes for {w}x{h}", bytes.len()));
    }
    blocking(move || {
        let cut = app.state::<cutout::Cutout>();
        let session = cut.session(&quality)?;
        let m = cutout::infer(&session, &bytes, w, h, 4)?;
        Ok(tauri::ipc::Response::new(cutout::upscale(&m, w, h)))
    })
    .await
}

/// Raw body = file bytes (e.g. a photo re-encoded by the editor), header x-name.
/// Stored in the app's scratch folder; returns its path.
#[tauri::command]
async fn write_temp(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let name = request.headers().get("x-name").and_then(|v| v.to_str().ok()).unwrap_or("upload.bin").to_string();
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_').collect();
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => b.clone(),
        _ => return Err("expected raw bytes".into()),
    };
    let st = app.state::<AppState>();
    let dir = tmp_dir(&app)?.join("uploads");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = dir.join(format!("{}-{}", st.next_id(), safe));
    std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
    Ok(p.display().to_string())
}

/// Decode a clip with FFmpeg at 320×320, run the model on every frame, and encode the
/// masks losslessly. Frames are retimed exactly like the picture in the filtergraph.
fn build_mask(
    t: &Tools,
    cut: &cutout::Cutout,
    c: &graph::Clip,
    fps: u32,
    turbo: bool,
    out: &Path,
    cancel: &AtomicBool,
    job: &Arc<Mutex<Option<Child>>>,
    mut on_frame: impl FnMut(u64),
) -> Result<(), String> {
    let session = cut.session(&c.cut_quality)?;
    let image = c.kind == "image";
    let frames = if image { 1 } else { graph::clip_frames(c, fps) };
    let sz = cutout::SIZE;
    let mut dec_args: Vec<String> = vec!["-hide_banner".into(), "-nostdin".into(), "-loglevel".into(), "error".into()];
    if image {
        dec_args.extend(["-i".into(), c.path.clone(), "-vf".into(), format!("scale={sz}:{sz}:flags=area,format=rgb24")]);
    } else {
        dec_args.extend(["-ss".into(), format!("{}", c.in_p), "-t".into(), format!("{}", (c.out_p - c.in_p).max(0.01))]);
        if turbo && c.speed >= graph::TURBO_SPEED {
            dec_args.extend(["-skip_frame".into(), "nokey".into()]);
        }
        dec_args.extend([
            "-i".into(), c.path.clone(), "-vf".into(),
            format!("setpts=(PTS-STARTPTS)/{},fps={fps},scale={sz}:{sz}:flags=area,format=rgb24", c.speed.clamp(0.1, 100.0)),
        ]);
    }
    dec_args.extend(["-frames:v".into(), frames.to_string(), "-f".into(), "rawvideo".into(), "-".into()]);
    let mut dec = cmd(&t.ffmpeg).args(&dec_args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let dec_err = drain(dec.stderr.take().unwrap());
    let mut enc_args: Vec<String> = vec![
        "-hide_banner".into(), "-y".into(), "-loglevel".into(), "error".into(), "-f".into(), "rawvideo".into(),
        "-pix_fmt".into(), "gray".into(), "-s".into(), format!("{sz}x{sz}"), "-r".into(), fps.to_string(), "-i".into(), "-".into(),
    ];
    if image {
        enc_args.extend(["-frames:v".into(), "1".into()]);
    } else {
        enc_args.extend(["-c:v".into(), "ffv1".into()]);
    }
    enc_args.push(out.display().to_string());
    let mut enc = cmd(&t.ffmpeg).args(&enc_args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let enc_err = drain(enc.stderr.take().unwrap());
    let mut src = dec.stdout.take().unwrap();
    let mut sink = enc.stdin.take().unwrap();
    *job.lock().unwrap() = Some(dec);
    let mut buf = vec![0u8; sz * sz * 3];
    let mut n = 0u64;
    let res = (|| -> Result<(), String> {
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            match src.read_exact(&mut buf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.to_string()),
            }
            let m = cutout::infer(&session, &buf, sz, sz, 3)?;
            sink.write_all(&m).map_err(|e| format!("mask encoder stopped: {e}"))?;
            n += 1;
            on_frame(n);
        }
        Ok(())
    })();
    drop(sink);
    let dec_status = job.lock().unwrap().take().map(|mut d| d.wait());
    let enc_status = enc.wait().map_err(|e| e.to_string())?;
    res?;
    if n == 0 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        return Err(format!("Could not read frames for the cutout: {}", dec_err.lock().unwrap().trim()));
    }
    if !matches!(dec_status, Some(Ok(s)) if s.success()) && n < frames {
        return Err(format!("Decoding stopped early: {}", dec_err.lock().unwrap().trim()));
    }
    if !enc_status.success() {
        std::thread::sleep(std::time::Duration::from_millis(100));
        return Err(format!("Could not write the cutout mask: {}", enc_err.lock().unwrap().trim()));
    }
    Ok(())
}

// ---------------------------------------------------------------- export

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    phase: String,
    detail: String,
    fraction: f64,
    speed: String,
    eta: f64,
    encoder: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportResult {
    path: String,
    seconds: f64,
    encoder: String,
    size: u64,
}

#[tauri::command]
async fn export_run(app: AppHandle, project: graph::Project, out: String) -> Result<ExportResult, String> {
    blocking(move || {
        let st = app.state::<AppState>();
        check_allowed(&st, &out)?;
        st.cancel.store(false, Ordering::SeqCst);
        let t = st.tools()?;
        let mut enc = if project.encoder == "software" { "libx264".to_string() } else { st.encoder(&t) };
        let slash = graph::supports_slash_opt(&st.version.lock().unwrap());

        // probe every source once
        let mut info: HashMap<String, MediaInfo> = HashMap::new();
        for c in &project.clips {
            if !info.contains_key(&c.path) && c.kind != "rendered_only" {
                let mi = tools::probe(&t, &c.path)?;
                info.insert(c.path.clone(), mi);
            }
        }

        // text PNGs -> temp files
        let work = tmp_dir(&app)?.join(format!("export{}", st.next_id()));
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let mut project = project;
        for (i, tx) in project.texts.iter_mut().enumerate() {
            if !tx.png.is_empty() {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(tx.png.as_bytes())
                    .map_err(|e| format!("bad text image: {e}"))?;
                let p = work.join(format!("text{i}.png"));
                std::fs::write(&p, bytes).map_err(|e| e.to_string())?;
                tx.png_path = p.display().to_string();
                tx.png.clear();
            }
        }
        // auto captions: ASS script + the caption fonts next to it for libass
        if !project.captions_ass.is_empty() {
            std::fs::write(work.join("captions.ass"), &project.captions_ass).map_err(|e| e.to_string())?;
            let fonts = work.join("fonts");
            std::fs::create_dir_all(&fonts).map_err(|e| e.to_string())?;
            for (name, bytes) in CAPTION_FONTS {
                std::fs::write(fonts.join(name), bytes).map_err(|e| e.to_string())?;
            }
            project.subs_file = Some("captions.ass".into());
        }
        let started = Instant::now();
        // AI cutout masks (native ONNX Runtime), weighted into the overall progress
        let person: Vec<usize> = project.clips.iter().enumerate().filter(|(_, c)| c.bg_mode == "person").map(|(i, _)| i).collect();
        let mask_frames: u64 = person.iter().map(|&i| { let c = &project.clips[i]; if c.kind == "image" { 1 } else { graph::clip_frames(c, project.fps) } }).sum();
        let render_frames: u64 = project.clips.iter().map(|c| graph::clip_frames(c, project.fps)).sum();
        let base = if mask_frames > 0 { (mask_frames as f64 * 4.0 / (mask_frames as f64 * 4.0 + render_frames as f64)).clamp(0.15, 0.9) } else { 0.0 };
        if !person.is_empty() {
            let cut = app.state::<cutout::Cutout>();
            if let Err(e) = cut.available() {
                let _ = std::fs::remove_dir_all(&work);
                return Err(format!("Background removal isn't available: {e}"));
            }
            let mut done = 0u64;
            for (k, &i) in person.iter().enumerate() {
                let c = project.clips[i].clone();
                let out_mask = work.join(if c.kind == "image" { format!("mask{i}.png") } else { format!("mask{i}.mkv") });
                let r = build_mask(&t, &cut, &c, project.fps, project.turbo, &out_mask, &st.cancel, &st.job, |n| {
                    if n % 2 == 0 || n == 1 {
                        let f = (done + n) as f64 / mask_frames.max(1) as f64;
                        let el = started.elapsed().as_secs_f64();
                        let _ = app.emit("export-progress", Progress {
                            phase: "cutout".into(),
                            detail: format!("clip {} of {}", k + 1, person.len()),
                            fraction: base * f,
                            speed: format!("{:.1} fps", (done + n) as f64 / el.max(0.01)),
                            eta: if f > 0.0 { el / f * (1.0 - f) } else { -1.0 },
                            encoder: enc.clone(),
                        });
                    }
                });
                if let Err(e) = r {
                    let _ = std::fs::remove_dir_all(&work);
                    return Err(e);
                }
                done += if c.kind == "image" { 1 } else { graph::clip_frames(&c, project.fps) };
                project.clips[i].mask_path = Some(out_mask.display().to_string());
            }
        }
        let script = work.join("graph.txt");
        let result = (|| -> Result<(), String> {
            loop {
                let plan = graph::build(&project, &info, &enc, &out, slash)?;
                std::fs::write(&script, &plan.filter).map_err(|e| e.to_string())?;
                let args: Vec<String> = plan
                    .args
                    .iter()
                    .map(|a| if a == graph::FILTER_SCRIPT { script.display().to_string() } else { a.clone() })
                    .collect();
                match run_ffmpeg(&app, &st, &t, &args, plan.total, &enc, base, &work) {
                    Ok(()) => return Ok(()),
                    Err(e) if e == "cancelled" => return Err(e),
                    Err(e) if enc != "libx264" => {
                        // hardware encoder failed mid-way (driver, size limits): redo on CPU
                        eprintln!("{enc} failed, falling back to libx264: {e}");
                        enc = "libx264".into();
                        *st.encoder.lock().unwrap() = Some(enc.clone());
                    }
                    Err(e) => return Err(e),
                }
            }
        })();
        let _ = std::fs::remove_dir_all(&work);
        for c in &project.clips {
            if let Some(r) = &c.rendered_path {
                let _ = std::fs::remove_file(r);
            }
        }
        match result {
            Ok(()) => {
                let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                Ok(ExportResult { path: out, seconds: started.elapsed().as_secs_f64(), encoder: enc, size })
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out);
                Err(e)
            }
        }
    })
    .await
}

#[allow(clippy::too_many_arguments)]
fn run_ffmpeg(app: &AppHandle, st: &AppState, t: &Tools, args: &[String], total: f64, enc: &str, base: f64, cwd: &Path) -> Result<(), String> {
    let mut child = cmd(&t.ffmpeg)
        .current_dir(cwd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let err = drain(child.stderr.take().unwrap());
    let stdout = child.stdout.take().unwrap();
    *st.job.lock().unwrap() = Some(child);
    let started = Instant::now();
    let mut speed = String::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(v) = line.strip_prefix("speed=") {
            speed = v.trim().to_string();
        }
        if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
            if let Ok(us) = v.trim().parse::<f64>() {
                let f = (us / 1e6 / total.max(0.01)).clamp(0.0, 1.0);
                let el = started.elapsed().as_secs_f64();
                let eta = if f > 0.01 { el / f * (1.0 - f) } else { -1.0 };
                let overall = base + (1.0 - base) * f;
                let _ = app.emit("export-progress", Progress { phase: "render".into(), detail: String::new(), fraction: overall, speed: speed.clone(), eta, encoder: enc.into() });
            }
        }
    }
    let status = st.job.lock().unwrap().take().map(|mut c| c.wait());
    if st.cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    match status {
        Some(Ok(s)) if s.success() => Ok(()),
        _ => {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let e = err.lock().unwrap().clone();
            Err(if e.trim().is_empty() { "FFmpeg stopped unexpectedly.".into() } else { e })
        }
    }
}

#[tauri::command]
fn export_cancel(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
    if let Some(c) = state.job.lock().unwrap().as_mut() {
        let _ = c.kill();
    }
}

// ---------------------------------------------------------------- auto captions

/// Caption fonts (Latin + Devanagari), embedded so libass can burn in exactly what the preview shows.
const CAPTION_FONTS: &[(&str, &[u8])] = &[
    ("CutdeckCaptions.ttf", include_bytes!("../../src/fonts/CutdeckCaptions.ttf")),
    ("CutdeckPoppins.ttf", include_bytes!("../../src/fonts/CutdeckPoppins.ttf")),
];

#[tauri::command]
async fn captions_status(app: AppHandle) -> Result<serde_json::Value, String> {
    blocking(move || captions::status(&app).map(|s| serde_json::to_value(s).unwrap_or_default())).await
}

#[tauri::command]
async fn captions_download(app: AppHandle, quality: String) -> Result<(), String> {
    blocking(move || captions::download(&app, &quality)).await
}

#[tauri::command]
async fn captions_run(app: AppHandle, jobs: Vec<captions::Job>, lang: String, quality: String) -> Result<serde_json::Value, String> {
    blocking(move || {
        let t = app.state::<AppState>().tools()?;
        let work = tmp_dir(&app)?.join(format!("captions{}", app.state::<AppState>().next_id()));
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let r = captions::run(&app, &t, &work, jobs, &lang, &quality);
        let _ = std::fs::remove_dir_all(&work);
        r.map(|v| serde_json::to_value(v).unwrap_or_default())
    })
    .await
}

#[tauri::command]
fn captions_cancel(app: AppHandle) {
    app.state::<captions::Captions>().stop();
}

// ---------------------------------------------------------------- stabilization

#[tauri::command]
async fn stabilize_run(app: AppHandle, path: String, strength: String) -> Result<String, String> {
    blocking(move || {
        let t = app.state::<AppState>().tools()?;
        let work = tmp_dir(&app)?.join(format!("stabilize{}", app.state::<AppState>().next_id()));
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let r = stabilize::run(&app, &t, &work, &path, &strength);
        let _ = std::fs::remove_dir_all(&work); // removes motion.trf
        r
    })
    .await
}

#[tauri::command]
fn stabilize_cancel(app: AppHandle) {
    app.state::<stabilize::Stabilizer>().stop();
}

// ---------------------------------------------------------------- drafts

/// Autosaved projects live in <app data>/drafts as <id>.cutdeck plus <id>.json metadata.
fn drafts_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().app_data_dir().map_err(|e| e.to_string())?.join("drafts");
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn draft_path(app: &AppHandle, id: &str, ext: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Invalid draft id".into());
    }
    Ok(drafts_dir(app)?.join(format!("{id}.{ext}")))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[tauri::command]
async fn list_drafts(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(drafts_dir(&app)?).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") || !p.with_extension("cutdeck").is_file() {
            continue;
        }
        if let Some(v) = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            out.push(v);
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v["updated"].as_u64().unwrap_or(0)));
    Ok(out)
}

/// Writes the project JSON and its metadata; returns the saved time (ms since epoch).
#[tauri::command]
async fn save_draft(app: AppHandle, id: String, json: String, mut meta: serde_json::Value) -> Result<u64, String> {
    let p = draft_path(&app, &id, "cutdeck")?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    let t = now_ms();
    if !meta.is_object() {
        meta = serde_json::json!({});
    }
    meta["id"] = id.clone().into();
    meta["updated"] = t.into();
    std::fs::write(draft_path(&app, &id, "json")?, meta.to_string()).map_err(|e| e.to_string())?;
    Ok(t)
}

#[tauri::command]
async fn load_draft(app: AppHandle, id: String) -> Result<String, String> {
    std::fs::read_to_string(draft_path(&app, &id, "cutdeck")?).map_err(|e| e.to_string())
}

#[tauri::command]
async fn rename_draft(app: AppHandle, id: String, name: String) -> Result<(), String> {
    let p = draft_path(&app, &id, "json")?;
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    v["name"] = name.into();
    std::fs::write(p, v.to_string()).map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_draft(app: AppHandle, id: String) -> Result<(), String> {
    let _ = std::fs::remove_file(draft_path(&app, &id, "json")?);
    let _ = std::fs::remove_file(draft_path(&app, &id, "cutdeck")?);
    Ok(())
}

// ---------------------------------------------------------------- app

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .manage(stabilize::Stabilizer::default())
        .setup(|app| {
            let server = media::start().map_err(|e| format!("Could not start the media server: {e}"))?;
            app.manage(server);
            let res = app.path().resource_dir().unwrap_or_default();
            let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
            let mut model_dir = res.join("models");
            if !model_dir.join("u2netp.onnx").is_file() && exe_dir.join("models/u2netp.onnx").is_file() {
                model_dir = exe_dir.join("models");
            }
            let search = vec![res.join("onnxruntime"), exe_dir.join("onnxruntime"), exe_dir.clone(), res.clone()];
            app.manage(cutout::Cutout::new(model_dir, search));
            let mut whisper = res.join("whisper");
            if !whisper.join("whisper-cli.exe").is_file() && !whisper.join("whisper-cli").is_file() {
                whisper = exe_dir.join("whisper");
            }
            app.manage(captions::Captions::new(whisper));
            // warm up ONNX Runtime + the fast model in the background
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                if let Err(e) = handle.state::<cutout::Cutout>().session("fast") {
                    eprintln!("cutout unavailable: {e}");
                }
            });
            let _ = drafts_dir(app.handle());
            // clear leftovers from a previous crash
            if let Ok(d) = app.path().app_cache_dir() {
                let _ = std::fs::remove_dir_all(d.join("work"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            env_status, probe_media, pick_files, pick_save, write_text, read_text, write_bytes,
            reveal, path_exists, media_base, make_proxy, cutout_mask, write_temp,
            export_run, export_cancel, list_drafts, save_draft, load_draft, rename_draft, delete_draft,
            captions_status, captions_download, captions_run, captions_cancel,
            stabilize_run, stabilize_cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running Cutdeck");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full native cutout export: FFmpeg decode -> ONNX mask per frame -> alphamerge onto magenta.
    #[test]
    fn exports_ai_cutout_clip() {
        let Ok(t) = tools::locate() else { return };
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
        let cut = cutout::Cutout::new(root.join("models"), vec![root.join("onnxruntime")]);
        if cut.available().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join("cutdeck-cutout-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("subject.mp4").display().to_string();
        // a warm disc moving over a flat grey wall, 4 s
        let st = cmd(&t.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                "color=c=0x7a7d82:s=640x360:r=30:d=4[bg];color=c=0xe0503c:s=180x180:r=30:d=4,format=rgba,geq=r=224:g=80:b=60:a='if(lt(hypot(X-90,Y-90),90),255,0)'[d];[bg][d]overlay=x='100+t*60':y=90",
                "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", &src])
            .status().unwrap();
        assert!(st.success());
        let clip: graph::Clip = serde_json::from_value(serde_json::json!({
            "kind":"video","path":src,"inP":0.0,"outP":4.0,"speed":2.0,"vol":0.0,"fit":"fill",
            "bgMode":"person","bgReplace":"color","bgColor":"#ff00ff","cutQuality":"fast"
        })).unwrap();
        let mask = dir.join("mask.mkv");
        let cancel = AtomicBool::new(false);
        let job = Arc::new(Mutex::new(None));
        let mut frames = 0;
        let t0 = Instant::now();
        build_mask(&t, &cut, &clip, 30, true, &mask, &cancel, &job, |n| frames = n).unwrap();
        eprintln!("masked {frames} frames in {:?}", t0.elapsed());
        assert_eq!(frames, 60); // 4 s at 2x = 2 s = 60 frames

        let mut clip = clip;
        clip.mask_path = Some(mask.display().to_string());
        let proj = graph::Project {
            width: 360, height: 640, fps: 30, quality: "std".into(), encoder: "libx264".into(), turbo: true,
            canvas: graph::CanvasCfg { bg: "color".into(), color: "#000000".into() },
            clips: vec![clip], texts: vec![], music: None, captions_ass: String::new(), subs_file: None,
        };
        let mut info = HashMap::new();
        info.insert(src.clone(), tools::probe(&t, &src).unwrap());
        let out = dir.join("out.mp4").display().to_string();
        let plan = graph::build(&proj, &info, "libx264", &out, graph::supports_slash_opt(&tools::version(&t))).unwrap();
        let script = dir.join("g.txt");
        std::fs::write(&script, &plan.filter).unwrap();
        let args: Vec<String> = plan.args.iter().map(|a| if a == graph::FILTER_SCRIPT { script.display().to_string() } else { a.clone() }).collect();
        let o = cmd(&t.ffmpeg).args(&args).output().unwrap();
        assert!(o.status.success(), "{}\n{}", String::from_utf8_lossy(&o.stderr), plan.filter);
        // grab the middle frame as raw RGB and check: wall -> magenta, disc kept
        let raw = cmd(&t.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-ss", "1", "-i", &out, "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output().unwrap().stdout;
        assert_eq!(raw.len(), 360 * 640 * 3);
        let px = |x: usize, y: usize| { let i = (y * 360 + x) * 3; (raw[i], raw[i + 1], raw[i + 2]) };
        let corner = px(5, 5);
        eprintln!("corner {:?}", corner);
        assert!(corner.0 > 200 && corner.1 < 60 && corner.2 > 200, "background replaced: {corner:?}");
        // disc is somewhere along the middle row; look for its warm colour
        let found = (0..360).any(|x| { let (r, g, b) = px(x, 320); r > 180 && g < 120 && b < 100 });
        assert!(found, "subject kept");
        std::fs::copy(&out, std::env::temp_dir().join("cutdeck-cutout-result.mp4")).ok();
    }
}
