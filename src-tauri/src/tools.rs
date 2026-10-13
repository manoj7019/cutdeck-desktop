//! Finding FFmpeg, picking the fastest working H.264 encoder, and probing media.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Serialize)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Build a Command that never flashes a console window on Windows.
pub fn cmd(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn works(p: &Path) -> bool {
    cmd(p)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Search order: CUTDECK_FFMPEG_DIR, next to the app executable (bundled sidecar),
/// common install folders, then PATH.
pub fn locate() -> Result<Tools, String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("CUTDECK_FFMPEG_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
            // macOS bundle: Contents/MacOS next to the binary already; Resources as fallback
            dirs.push(d.join("../Resources"));
        }
    }
    if cfg!(target_os = "macos") {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    if cfg!(windows) {
        if let Ok(p) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(&p).join("Microsoft/WinGet/Links"));
        }
        dirs.push("C:/ffmpeg/bin".into());
        dirs.push("C:/Program Files/ffmpeg/bin".into());
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    for d in dirs {
        // bundled copies are prefixed so they never clash with a system FFmpeg
        for (fa, pa) in [("cutdeck-ffmpeg", "cutdeck-ffprobe"), ("ffmpeg", "ffprobe")] {
            let f = d.join(exe_name(fa));
            let p = d.join(exe_name(pa));
            if f.is_file() && p.is_file() && works(&f) {
                return Ok(Tools { ffmpeg: f, ffprobe: p });
            }
        }
    }
    Err("FFmpeg was not found. Install it (Windows: `winget install Gyan.FFmpeg`, macOS: `brew install ffmpeg`, Linux: your package manager) or use an installer build that bundles it.".into())
}

pub fn version(t: &Tools) -> String {
    cmd(&t.ffmpeg)
        .arg("-version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(|l| l.to_string()))
        .unwrap_or_default()
}

/// H.264 encoders in order of preference. The first one that can actually encode
/// a test frame on this machine wins (a listed encoder can still fail without the GPU/driver).
pub const ENCODERS: &[&str] = &[
    "h264_nvenc",        // NVIDIA
    "h264_videotoolbox", // Apple
    "h264_qsv",          // Intel Quick Sync
    "h264_amf",          // AMD
    "libx264",           // CPU
];

pub fn detect_encoder(t: &Tools) -> String {
    let listed = cmd(&t.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    for enc in ENCODERS {
        if !listed.contains(enc) {
            continue;
        }
        if *enc == "libx264" {
            return enc.to_string();
        }
        let ok = cmd(&t.ffmpeg)
            .args([
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30:d=0.5", "-c:v", enc, "-pix_fmt", "yuv420p",
                "-f", "null", "-",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return enc.to_string();
        }
    }
    "libx264".into()
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub duration: f64,
    /// Display size (rotation metadata already applied, as FFmpeg and browsers show it).
    pub width: u32,
    pub height: u32,
    pub has_video: bool,
    pub has_audio: bool,
    pub video_codec: String,
}

pub fn probe(t: &Tools, path: &str) -> Result<MediaInfo, String> {
    let out = cmd(&t.ffprobe)
        .args([
            "-v", "error", "-print_format", "json", "-show_format", "-show_streams", path,
        ])
        .output()
        .map_err(|e| format!("Could not run ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "Could not read {}: {}",
            path,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("Bad ffprobe output: {e}"))?;
    let mut info = MediaInfo::default();
    info.duration = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    for s in v["streams"].as_array().cloned().unwrap_or_default() {
        match s["codec_type"].as_str() {
            Some("video") if !info.has_video => {
                // attached cover art in audio files is not real video
                if s["disposition"]["attached_pic"].as_i64() == Some(1) {
                    continue;
                }
                info.has_video = true;
                info.video_codec = s["codec_name"].as_str().unwrap_or("").to_string();
                let w = s["width"].as_u64().unwrap_or(0) as u32;
                let h = s["height"].as_u64().unwrap_or(0) as u32;
                let mut rot = s["tags"]["rotate"]
                    .as_str()
                    .and_then(|r| r.parse::<i64>().ok())
                    .unwrap_or(0);
                if let Some(list) = s["side_data_list"].as_array() {
                    for sd in list {
                        if let Some(r) = sd["rotation"].as_i64() {
                            rot = r;
                        }
                    }
                }
                if rot.rem_euclid(180) == 90 {
                    info.width = h;
                    info.height = w;
                } else {
                    info.width = w;
                    info.height = h;
                }
                if info.duration == 0.0 {
                    info.duration = s["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
                }
            }
            Some("audio") => info.has_audio = true,
            _ => {}
        }
    }
    Ok(info)
}
