//! Turns an editor project into a single FFmpeg command + filtergraph.
//!
//! Every clip becomes its own input (fast input seeking with -ss/-t), is retimed with
//! setpts and immediately resampled to the output frame rate, so at 100x only the frames
//! that end up in the video are scaled, filtered and composited. The canvas (blurred
//! background or color), look filters, rotation, chroma key, fades, text overlays and
//! music are all done inside FFmpeg; clips that use the AI person cutout arrive
//! pre-rendered from the editor and are treated as plain video.

use crate::tools::MediaInfo;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    #[serde(default)]
    pub quality: String,
    #[serde(default)]
    pub encoder: String,
    #[serde(default = "yes")]
    pub turbo: bool,
    pub canvas: CanvasCfg,
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub texts: Vec<TextLayer>,
    #[serde(default)]
    pub music: Option<Music>,
}
fn yes() -> bool {
    true
}

#[derive(Deserialize, Clone, Debug)]
pub struct CanvasCfg {
    pub bg: String,
    pub color: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Adj {
    #[serde(default)]
    pub b: f64,
    #[serde(default)]
    pub c: f64,
    #[serde(default)]
    pub s: f64,
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub v: f64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    /// "video" | "image" | "rendered"
    pub kind: String,
    /// Original media file (also the audio source for rendered clips).
    pub path: String,
    #[serde(default)]
    pub rendered_path: Option<String>,
    pub in_p: f64,
    pub out_p: f64,
    pub speed: f64,
    pub vol: f64,
    #[serde(default)]
    pub rot: i32,
    #[serde(default)]
    pub flip: bool,
    #[serde(default = "fit")]
    pub fit: String,
    #[serde(default = "one")]
    pub zoom: f64,
    #[serde(default)]
    pub px: f64,
    #[serde(default)]
    pub py: f64,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub adj: Adj,
    #[serde(default)]
    pub bg_mode: String,
    #[serde(default)]
    pub chroma: String,
    #[serde(default)]
    pub tol: f64,
    #[serde(default)]
    pub bg_replace: String,
    #[serde(default)]
    pub bg_color: String,
    #[serde(default)]
    pub bg_image_path: Option<String>,
    #[serde(default)]
    pub trans: String,
    /// "fast" | "best" model for the AI cutout.
    #[serde(default)]
    pub cut_quality: String,
    /// Filled in by the exporter: per-frame mask video (or PNG for photos) for AI cutout.
    #[serde(default)]
    pub mask_path: Option<String>,
}
fn fit() -> String {
    "fit".into()
}
fn one() -> f64 {
    1.0
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TextLayer {
    /// Base64 PNG of the text at output size (written to disk by the exporter).
    #[serde(default)]
    pub png: String,
    #[serde(default)]
    pub png_path: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Music {
    pub path: String,
    pub start: f64,
    pub offset: f64,
    pub vol: f64,
    #[serde(default)]
    pub fade: bool,
}

pub struct Plan {
    /// Arguments after the ffmpeg program name. Contains FILTER_SCRIPT where the
    /// filtergraph file path goes.
    pub args: Vec<String>,
    pub filter: String,
    pub total: f64,
    #[allow(dead_code)]
    pub frames: u64,
}

pub const FILTER_SCRIPT: &str = "{FILTER_SCRIPT}";
/// Speeds at or above this decode only keyframes when turbo is on.
pub const TURBO_SPEED: f64 = 50.0;

fn even(x: f64) -> u32 {
    ((x / 2.0).round() as u32 * 2).clamp(2, 16000)
}
fn num(x: f64) -> String {
    let s = format!("{:.6}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s
    }
}
fn hex(c: &str) -> String {
    let h = c.trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("0x{}", h.to_lowercase())
    } else {
        "0x000000".into()
    }
}
fn rgb(c: &str) -> (f64, f64, f64) {
    let h = hex(c);
    let v = u32::from_str_radix(&h[2..], 16).unwrap_or(0);
    (((v >> 16) & 255) as f64, ((v >> 8) & 255) as f64, (v & 255) as f64)
}

pub fn clip_frames(c: &Clip, fps: u32) -> u64 {
    let len = ((c.out_p - c.in_p) / c.speed.max(0.01)).max(0.0);
    ((len * fps as f64).round() as u64).max(1)
}

/// atempo works best in 0.5..2 steps; chain them for 0.25x..100x.
pub fn atempo_chain(speed: f64) -> String {
    let mut s = speed.clamp(0.1, 100.0);
    let mut parts = vec![];
    while s > 2.0 + 1e-9 {
        parts.push("atempo=2".to_string());
        s /= 2.0;
    }
    while s < 0.5 - 1e-9 {
        parts.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-6 {
        parts.push(format!("atempo={}", num(s)));
    }
    parts.join(",")
}

/// Color look presets matching the editor's filter list, as FFmpeg filters.
fn look_filters(c: &Clip) -> Vec<String> {
    // (brightness add, contrast mul, saturation mul, extra filters, tint)
    let (mut br, mut ct, mut sat, extra, tint): (f64, f64, f64, Vec<String>, Option<(&str, f64)>) =
        match c.filter.as_str() {
            "vivid" => (0.0, 1.1, 1.45, vec![], None),
            "warm" => (0.0, 1.05, 1.15, vec![], Some(("#ff9a3c", 0.16))),
            "cool" => (0.015, 1.0, 0.95, vec![], Some(("#3c8cff", 0.15))),
            "fade" => (0.04, 0.82, 0.8, vec![], None),
            "vintage" => {
                let k = 0.45;
                let m = |id: f64, sp: f64| num(id * (1.0 - k) + sp * k);
                (
                    0.025,
                    0.9,
                    1.0,
                    vec![format!(
                        "colorchannelmixer=rr={}:rg={}:rb={}:gr={}:gg={}:gb={}:br={}:bg={}:bb={}",
                        m(1.0, 0.393), m(0.0, 0.769), m(0.0, 0.189),
                        m(0.0, 0.349), m(1.0, 0.686), m(0.0, 0.168),
                        m(0.0, 0.272), m(0.0, 0.534), m(1.0, 0.131)
                    )],
                    None,
                )
            }
            "mono" => (0.0, 1.1, 0.0, vec![], None),
            "noir" => (-0.05, 1.6, 0.0, vec![], None),
            "dusk" => (0.0, 1.0, 1.2, vec!["hue=h=-12".into()], Some(("#ff4f8b", 0.12))),
            "mint" => (0.025, 1.0, 0.9, vec!["hue=h=10".into()], Some(("#3fffc0", 0.08))),
            "teal" => (0.0, 1.15, 1.1, vec![], Some(("#0f6b7a", 0.14))),
            "pop" => (0.025, 1.2, 1.8, vec![], None),
            _ => (0.0, 1.0, 1.0, vec![], None),
        };
    br += c.adj.b / 200.0;
    ct *= 1.0 + c.adj.c / 100.0;
    sat = (sat * (1.0 + c.adj.s / 100.0)).clamp(0.0, 3.0);
    let mut f = extra;
    if br.abs() > 1e-4 || (ct - 1.0).abs() > 1e-4 || (sat - 1.0).abs() > 1e-4 {
        f.push(format!(
            "eq=brightness={}:contrast={}:saturation={}",
            num(br.clamp(-1.0, 1.0)),
            num(ct.clamp(0.0, 4.0)),
            num(sat)
        ));
    }
    if let Some((col, a)) = tint {
        f.push(format!("drawbox=x=0:y=0:w=iw:h=ih:color={}@{}:t=fill", hex(col), num(a)));
    }
    if c.adj.w.abs() > 0.5 {
        let col = if c.adj.w > 0.0 { "#ff8a2a" } else { "#2a7dff" };
        f.push(format!(
            "drawbox=x=0:y=0:w=iw:h=ih:color={}@{}:t=fill",
            hex(col),
            num(c.adj.w.abs() / 100.0 * 0.28)
        ));
    }
    f
}

pub fn build(
    p: &Project,
    info: &HashMap<String, MediaInfo>,
    encoder: &str,
    out_path: &str,
    slash_filter_opt: bool,
) -> Result<Plan, String> {
    if p.clips.is_empty() {
        return Err("The timeline is empty.".into());
    }
    let (w, h, fps) = (p.width, p.height, p.fps.max(1));
    let fpsf = fps as f64;
    let mut inputs: Vec<Vec<String>> = vec![];
    let mut g = String::new(); // filtergraph
    let mut concat_in = String::new();
    let mut total = 0.0;
    let mut frames_total = 0u64;
    let s = |x: &str| x.to_string();

    for (i, c) in p.clips.iter().enumerate() {
        let frames = clip_frames(c, fps);
        let dur = frames as f64 / fpsf;
        total += dur;
        frames_total += frames;
        let mi = info.get(&c.path).cloned().unwrap_or_default();
        let speed = c.speed.clamp(0.1, 100.0);
        let src_len = (c.out_p - c.in_p).max(0.01);
        let fit_trim = format!(
            "tpad=stop_mode=clone:stop_duration=1,trim=end_frame={frames},setpts=PTS-STARTPTS"
        );
        let mut audio_input: Option<usize> = None;
        let wants_audio = c.kind != "image" && mi.has_audio && c.vol > 0.001;

        if c.kind == "rendered" {
            let rp = c
                .rendered_path
                .clone()
                .ok_or_else(|| format!("Clip {} is missing its rendered cutout", i + 1))?;
            let n = inputs.len();
            inputs.push(vec![s("-i"), rp]);
            writeln!(
                g,
                "[{n}:v]setpts=PTS-STARTPTS,fps={fps},scale={w}:{h},setsar=1,{fit_trim},format=yuv420p[v{i}];"
            )
            .unwrap();
            if wants_audio {
                let an = inputs.len();
                inputs.push(vec![
                    s("-ss"), num(c.in_p), s("-t"), num(src_len), s("-vn"), s("-i"), c.path.clone(),
                ]);
                audio_input = Some(an);
            }
        } else {
            let n = inputs.len();
            let turbo = c.kind == "video" && p.turbo && speed >= TURBO_SPEED;
            let src;
            if c.kind == "image" {
                inputs.push(vec![
                    s("-loop"), s("1"), s("-framerate"), fps.to_string(), s("-t"), num(dur + 1.0),
                    s("-i"), c.path.clone(),
                ]);
                src = format!("[{n}:v]setpts=PTS-STARTPTS,fps={fps}");
            } else {
                let mut a = vec![s("-ss"), num(c.in_p), s("-t"), num(src_len)];
                if turbo {
                    a.extend([s("-skip_frame"), s("nokey")]);
                }
                a.extend([s("-i"), c.path.clone()]);
                inputs.push(a);
                src = format!("[{n}:v]setpts=(PTS-STARTPTS)/{},fps={fps}", num(speed));
                if wants_audio {
                    if turbo {
                        let an = inputs.len();
                        inputs.push(vec![
                            s("-ss"), num(c.in_p), s("-t"), num(src_len), s("-vn"), s("-i"),
                            c.path.clone(),
                        ]);
                        audio_input = Some(an);
                    } else {
                        audio_input = Some(n);
                    }
                }
            }
            // Source display size (rotation metadata already applied by the decoder).
            let (sw, sh) = (mi.width.max(2) as f64, mi.height.max(2) as f64);
            let rot = c.rot.rem_euclid(360);
            let (bw, bh) = if rot % 180 != 0 { (sh, sw) } else { (sw, sh) };
            let wf = w as f64;
            let hf = h as f64;
            let base = if c.fit == "fill" {
                (wf / bw).max(hf / bh)
            } else {
                (wf / bw).min(hf / bh)
            };
            let scale = base * c.zoom.clamp(0.1, 6.0);
            let fw = even(bw * scale);
            let fh = even(bh * scale);
            let x = ((wf - fw as f64) / 2.0 + c.px * wf).round() as i64;
            let y = ((hf - fh as f64) / 2.0 + c.py * hf).round() as i64;

            // foreground chain
            let mut fg: Vec<String> = vec![];
            if c.flip {
                fg.push("hflip".into());
            }
            match rot {
                90 => fg.push("transpose=1".into()),
                180 => fg.push("hflip,vflip".into()),
                270 => fg.push("transpose=2".into()),
                _ => {}
            }
            fg.extend(look_filters(c));
            fg.push(format!("scale={fw}:{fh}:flags=bicubic"));
            if c.adj.v > 0.5 {
                fg.push(format!("vignette=angle={}", num((0.25 + c.adj.v / 100.0 * 0.9).min(1.5))));
            }
            let person = c.bg_mode == "person" && c.mask_path.is_some();
            let keyed = c.bg_mode == "chroma" || person;
            if c.bg_mode == "chroma" {
                let (kr, kg, kb) = rgb(&c.chroma);
                let sim = (c.tol * 2.6 / 441.67).clamp(0.01, 0.95);
                fg.push(format!(
                    "format=rgba,colorkey={}:{}:{}",
                    hex(&c.chroma),
                    num(sim),
                    num((sim * 0.5).clamp(0.02, 0.5))
                ));
                if kg > kr && kg > kb {
                    fg.push("despill=type=green:mix=0.5:expand=0".into());
                } else if kb > kr && kb > kg {
                    fg.push("despill=type=blue:mix=0.5:expand=0".into());
                }
            }

            // background
            let bg_kind: &str = if keyed {
                match c.bg_replace.as_str() {
                    "color" => "color",
                    "image" if c.bg_image_path.is_some() => "image",
                    "blur" if person => "blur",
                    _ if person && p.canvas.bg == "blur" => "blur",
                    _ => "canvas",
                }
            } else if p.canvas.bg == "blur" {
                "blur"
            } else {
                "canvas"
            };
            let fgl = if person { format!("fgr{i}") } else { format!("fg{i}") };
            if person {
                // mask: same retiming as the clip, then the same flip/rotate/scale as the picture
                let mp = c.mask_path.clone().unwrap();
                let m = inputs.len();
                if c.kind == "image" {
                    inputs.push(vec![
                        s("-loop"), s("1"), s("-framerate"), fps.to_string(), s("-t"), num(dur + 1.0), s("-i"), mp,
                    ]);
                } else {
                    inputs.push(vec![s("-i"), mp]);
                }
                let mut mk = vec![
                    format!("setpts=PTS-STARTPTS,fps={fps}"),
                    fit_trim.clone(),
                    "format=gray".into(),
                    format!("scale={}:{}:flags=bicubic", even(sw), even(sh)),
                ];
                if c.flip {
                    mk.push("hflip".into());
                }
                match rot {
                    90 => mk.push("transpose=1".into()),
                    180 => mk.push("hflip,vflip".into()),
                    270 => mk.push("transpose=2".into()),
                    _ => {}
                }
                mk.push(format!("scale={fw}:{fh}:flags=bicubic"));
                mk.push("gblur=sigma=0.8".into());
                writeln!(g, "[{m}:v]{}[mk{i}];", mk.join(",")).unwrap();
            }
            if bg_kind == "blur" {
                writeln!(g, "{src},{fit_trim},split=2[s{i}a][s{i}b];").unwrap();
                let k = (wf / sw).max(hf / sh) * 1.12;
                let (cw, ch) = (even(sw * k), even(sh * k));
                let r = ((wf.max(hf)) / 320.0).round().max(2.0) as u32;
                writeln!(
                    g,
                    "[s{i}b]scale={}:{},boxblur={r}:2,scale={cw}:{ch}:flags=bilinear,crop={w}:{h},eq=brightness=-0.12,setsar=1[bg{i}];",
                    even(cw as f64 / 8.0).max(2 * r + 2),
                    even(ch as f64 / 8.0).max(2 * r + 2)
                )
                .unwrap();
                writeln!(g, "[s{i}a]{}[{fgl}];", fg.join(",")).unwrap();
            } else {
                writeln!(g, "{src},{fit_trim},{}[{fgl}];", fg.join(",")).unwrap();
                match bg_kind {
                    "image" => {
                        let m = inputs.len();
                        inputs.push(vec![
                            s("-loop"), s("1"), s("-framerate"), fps.to_string(), s("-t"),
                            num(dur + 1.0), s("-i"), c.bg_image_path.clone().unwrap(),
                        ]);
                        writeln!(
                            g,
                            "[{m}:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1,fps={fps}[bg{i}];"
                        )
                        .unwrap();
                    }
                    _ => {
                        let col = if bg_kind == "color" { &c.bg_color } else { &p.canvas.color };
                        writeln!(
                            g,
                            "color=c={}:s={w}x{h}:r={fps}:d={}[bg{i}];",
                            hex(col),
                            num(dur + 1.0)
                        )
                        .unwrap();
                    }
                }
            }
            if person {
                writeln!(g, "[fgr{i}][mk{i}]alphamerge[fg{i}];").unwrap();
            }
            let mut post = vec![format!(
                "overlay=x={x}:y={y}:shortest=1:eof_action=pass:format=auto"
            )];
            if c.trans == "fade" {
                post.push(format!("fade=t=in:st=0:d={}", num((dur / 3.0).min(0.6))));
            }
            post.push(fit_trim.clone());
            post.push("setsar=1,format=yuv420p".into());
            writeln!(g, "[bg{i}][fg{i}]{}[v{i}];", post.join(",")).unwrap();
        }

        // audio for this clip, always exactly `dur` long
        match audio_input {
            Some(an) => {
                let mut chain = vec![s("asetpts=PTS-STARTPTS")];
                let at = atempo_chain(speed);
                if !at.is_empty() {
                    chain.push(at);
                }
                chain.push(format!("volume={}", num(c.vol.clamp(0.0, 4.0))));
                chain.push(s("aresample=48000"));
                chain.push(s("aformat=sample_fmts=fltp:channel_layouts=stereo"));
                chain.push(s("apad"));
                chain.push(format!("atrim=end={}", num(dur)));
                chain.push(s("asetpts=PTS-STARTPTS"));
                writeln!(g, "[{an}:a]{}[a{i}];", chain.join(",")).unwrap();
            }
            None => {
                writeln!(
                    g,
                    "anullsrc=r=48000:cl=stereo,atrim=end={},aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}];",
                    num(dur)
                )
                .unwrap();
            }
        }
        write!(concat_in, "[v{i}][a{i}]").unwrap();
    }
    writeln!(
        g,
        "{concat_in}concat=n={}:v=1:a=1[vc][ac];",
        p.clips.len()
    )
    .unwrap();

    // text overlays (pre-rendered PNGs at output size)
    let mut vlabel = "vc".to_string();
    let mut ti = 0;
    for t in &p.texts {
        if t.png_path.is_empty() || t.end <= t.start || t.start >= total {
            continue;
        }
        let n = inputs.len();
        inputs.push(vec![
            s("-loop"), s("1"), s("-framerate"), fps.to_string(), s("-t"), num(total + 1.0),
            s("-i"), t.png_path.clone(),
        ]);
        writeln!(
            g,
            "[{vlabel}][{n}:v]overlay=0:0:format=auto:eof_action=pass:enable='between(t,{},{})'[vt{ti}];",
            num(t.start),
            num(t.end - 0.5 / fpsf)
        )
        .unwrap();
        vlabel = format!("vt{ti}");
        ti += 1;
    }
    writeln!(g, "[{vlabel}]format=yuv420p[vout];").unwrap();

    // music bed
    let mut alabel = "ac";
    if let Some(m) = &p.music {
        if m.start < total && !m.path.is_empty() {
            let n = inputs.len();
            inputs.push(vec![s("-ss"), num(m.offset.max(0.0)), s("-vn"), s("-i"), m.path.clone()]);
            let ms = (m.start * 1000.0).round() as i64;
            let mut ch = vec![
                s("asetpts=PTS-STARTPTS"),
                s("aresample=48000"),
                s("aformat=sample_fmts=fltp:channel_layouts=stereo"),
                format!("volume={}", num(m.vol.clamp(0.0, 4.0))),
            ];
            if m.fade {
                ch.push(s("afade=t=in:st=0:d=1"));
            }
            if ms > 0 {
                ch.push(format!("adelay={ms}:all=1"));
            }
            ch.push(s("apad"));
            ch.push(format!("atrim=end={}", num(total)));
            if m.fade {
                ch.push(format!(
                    "afade=t=out:st={}:d=1.5",
                    num((total - 1.5).max(0.0))
                ));
            }
            writeln!(g, "[{n}:a]{}[mus];", ch.join(",")).unwrap();
            writeln!(
                g,
                "[ac][mus]amix=inputs=2:duration=first:dropout_transition=0:normalize=0[amix];"
            )
            .unwrap();
            alabel = "amix";
        }
    }
    writeln!(g, "[{alabel}]alimiter=limit=0.98[aout]").unwrap();

    // assemble arguments
    let mut args: Vec<String> = vec![
        s("-hide_banner"), s("-y"), s("-nostdin"), s("-loglevel"), s("error"), s("-nostats"),
        s("-progress"), s("pipe:1"),
    ];
    for i in &inputs {
        args.extend(i.iter().cloned());
    }
    if slash_filter_opt {
        args.extend([s("-/filter_complex"), s(FILTER_SCRIPT)]);
    } else {
        args.extend([s("-filter_complex_script"), s(FILTER_SCRIPT)]);
    }
    args.extend([s("-map"), s("[vout]"), s("-map"), s("[aout]")]);
    args.extend(encoder_args(encoder, &p.quality, w, h, fps));
    args.extend([
        s("-r"), fps.to_string(), s("-c:a"), s("aac"), s("-b:a"), s("192k"), s("-movflags"),
        s("+faststart"), s("-max_muxing_queue_size"), s("4096"), out_path.to_string(),
    ]);
    Ok(Plan { args, filter: g, total, frames: frames_total })
}

pub fn encoder_args(enc: &str, quality: &str, w: u32, h: u32, fps: u32) -> Vec<String> {
    let high = quality != "std";
    let bitrate = ((w as f64 * h as f64 * fps as f64) * if high { 0.14 } else { 0.07 }) as u64;
    let v: Vec<&str> = match enc {
        "h264_nvenc" => vec![
            "-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq",
            if high { "19" } else { "26" }, "-b:v", "0", "-profile:v", "high",
        ],
        "h264_qsv" => vec![
            "-c:v", "h264_qsv", "-preset", "medium", "-global_quality",
            if high { "20" } else { "26" },
        ],
        "h264_amf" => vec![
            "-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i",
            if high { "18" } else { "24" }, "-qp_p", if high { "20" } else { "26" },
        ],
        "h264_videotoolbox" => {
            return vec![
                "-c:v".into(), "h264_videotoolbox".into(), "-b:v".into(), bitrate.to_string(),
                "-allow_sw".into(), "1".into(), "-profile:v".into(), "high".into(),
                "-pix_fmt".into(), "yuv420p".into(),
            ]
        }
        _ => vec![
            "-c:v", "libx264", "-preset", if high { "fast" } else { "veryfast" }, "-crf",
            if high { "18" } else { "23" }, "-profile:v", "high",
        ],
    };
    let mut out: Vec<String> = v.into_iter().map(String::from).collect();
    out.extend(["-pix_fmt".into(), "yuv420p".into()]);
    out
}

/// FFmpeg 7.1+ replaced -filter_complex_script with "-/filter_complex <file>".
pub fn supports_slash_opt(version_line: &str) -> bool {
    // "ffmpeg version 6.1.1-3ubuntu5", "ffmpeg version n7.1", "ffmpeg version N-118000-g..",
    // "ffmpeg version 2025-01-01-git-..-full_build-www.gyan.dev"
    let v = version_line
        .split_whitespace()
        .nth(2)
        .unwrap_or("")
        .trim_start_matches('n');
    if v.starts_with('N') || v.starts_with("20") {
        return true; // nightly / dated git builds are recent
    }
    let mut it = v.split(|c: char| !c.is_ascii_digit());
    let major: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let minor: u32 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    major > 7 || (major == 7 && minor >= 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atempo_ranges() {
        assert_eq!(atempo_chain(1.0), "");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5");
        assert_eq!(atempo_chain(4.0), "atempo=2,atempo=2");
        let c = atempo_chain(100.0);
        let prod: f64 = c
            .split(',')
            .map(|p| p.trim_start_matches("atempo=").parse::<f64>().unwrap())
            .product();
        assert!((prod - 100.0).abs() < 1e-6, "{c}");
    }

    #[test]
    fn versions() {
        assert!(!supports_slash_opt("ffmpeg version 6.1.1-3ubuntu5 Copyright"));
        assert!(supports_slash_opt("ffmpeg version n7.1 Copyright"));
        assert!(supports_slash_opt("ffmpeg version 7.1.1-full_build-www.gyan.dev"));
        assert!(supports_slash_opt("ffmpeg version 8.0 Copyright"));
        assert!(supports_slash_opt("ffmpeg version N-118000-gabc Copyright"));
        assert!(supports_slash_opt("ffmpeg version 2025-03-31-git-35c091f4b7-full_build-www.gyan.dev"));
        assert!(!supports_slash_opt("ffmpeg version 7.0.2"));
    }

    #[test]
    fn frames_follow_speed() {
        let c: Clip = serde_json::from_value(serde_json::json!({
            "kind":"video","path":"a.mp4","inP":0.0,"outP":100.0,"speed":100.0,"vol":1.0
        }))
        .unwrap();
        assert_eq!(clip_frames(&c, 30), 30); // 100 s at 100x = 1 s = 30 frames
    }
    /// End-to-end: build real projects and render them with the system FFmpeg.
    #[test]
    fn renders_with_ffmpeg() {
        let Ok(t) = crate::tools::locate() else {
            eprintln!("ffmpeg not installed, skipping");
            return;
        };
        let dir = std::env::temp_dir().join("cutdeck-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let d = |n: &str| dir.join(n).display().to_string();
        let run = |args: &[&str]| {
            let o = crate::tools::cmd(&t.ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        };
        // 120 s landscape clip with tone, keyframe every second
        run(&["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=120", "-f", "lavfi", "-i", "sine=f=440:d=120",
              "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-c:a", "aac", "-shortest", &d("long.mp4")]);
        // portrait clip without audio
        run(&["-f", "lavfi", "-i", "testsrc=s=360x640:r=25:d=6", "-c:v", "libx264", "-preset", "ultrafast", &d("portrait.mp4")]);
        // green-screen still: green background with a red square
        run(&["-f", "lavfi", "-i", "color=c=0x00d000:s=800x600,drawbox=x=250:y=150:w=300:h=300:color=red:t=fill", "-frames:v", "1", &d("green.png")]);
        run(&["-f", "lavfi", "-i", "color=c=black@0.0:s=720x1280,format=rgba,geq=r=255:g=210:b=63:a='if(between(X,100,620)*between(Y,100,220),255,0)'", "-frames:v", "1", &d("text.png")]);
        run(&["-f", "lavfi", "-i", "sine=f=220:d=30", &d("music.mp3")]);
        run(&["-f", "lavfi", "-i", "testsrc2=s=720x1280:r=30:d=2", "-c:v", "libx264", "-preset", "ultrafast", &d("rendered.mkv")]);

        let mut info = HashMap::new();
        for f in ["long.mp4", "portrait.mp4", "green.png", "rendered.mkv"] {
            info.insert(d(f), crate::tools::probe(&t, &d(f)).unwrap());
        }
        assert!(info[&d("long.mp4")].has_audio);
        assert!(!info[&d("portrait.mp4")].has_audio);

        let ver = crate::tools::version(&t);
        for (name, canvas_bg, turbo) in [("blur", "blur", true), ("color", "color", false)] {
            let proj: Project = serde_json::from_value(serde_json::json!({
                "width": 720, "height": 1280, "fps": 30, "quality": "std", "turbo": turbo,
                "canvas": {"bg": canvas_bg, "color": "#14181f"},
                "clips": [
                    {"kind":"video","path":d("long.mp4"),"inP":0.0,"outP":20.0,"speed":10.0,"vol":1.0,"filter":"vivid","trans":"fade"},
                    {"kind":"video","path":d("long.mp4"),"inP":5.0,"outP":7.0,"speed":0.5,"vol":0.8,"rot":90,"flip":true,"fit":"fill","zoom":1.2,"px":0.05,
                     "filter":"vintage","adj":{"b":10,"c":10,"s":-20,"w":40,"v":50}},
                    {"kind":"video","path":d("long.mp4"),"inP":0.0,"outP":120.0,"speed":100.0,"vol":0.0},
                    {"kind":"video","path":d("portrait.mp4"),"inP":1.0,"outP":5.0,"speed":2.0,"vol":1.0,"filter":"noir"},
                    {"kind":"image","path":d("green.png"),"inP":0.0,"outP":2.0,"speed":1.0,"vol":0.0,"bgMode":"chroma","chroma":"#00d000","tol":35.0,
                     "bgReplace":"color","bgColor":"#3355ff"},
                    {"kind":"rendered","path":d("long.mp4"),"renderedPath":d("rendered.mkv"),"inP":10.0,"outP":12.0,"speed":1.0,"vol":1.0}
                ],
                "texts": [{"pngPath": d("text.png"), "start": 1.0, "end": 3.0}],
                "music": {"path": d("music.mp3"), "start": 0.5, "offset": 2.0, "vol": 0.5, "fade": true}
            })).unwrap();
            let out = d(&format!("out-{name}.mp4"));
            let plan = build(&proj, &info, "libx264", &out, supports_slash_opt(&ver)).unwrap();
            let script = d(&format!("graph-{name}.txt"));
            std::fs::write(&script, &plan.filter).unwrap();
            let args: Vec<String> = plan.args.iter().map(|a| if a == FILTER_SCRIPT { script.clone() } else { a.clone() }).collect();
            let started = std::time::Instant::now();
            let o = crate::tools::cmd(&t.ffmpeg).args(&args).output().unwrap();
            assert!(o.status.success(), "ffmpeg failed:\n{}\n--- graph ---\n{}", String::from_utf8_lossy(&o.stderr), plan.filter);
            // expected: 2 s + 4 s + 1.2 s + 2 s + 2 s + 2 s = 13.2 s
            assert!((plan.total - 13.2).abs() < 0.05, "planned {}", plan.total);
            let mi = crate::tools::probe(&t, &out).unwrap();
            eprintln!("{name}: {:.3}s rendered in {:.2}s, {}x{}", mi.duration, started.elapsed().as_secs_f64(), mi.width, mi.height);
            assert_eq!((mi.width, mi.height), (720, 1280));
            assert!(mi.has_audio);
            assert!((mi.duration - 13.2).abs() < 0.15, "duration {}", mi.duration);
            let frames = crate::tools::cmd(&t.ffprobe)
                .args(["-v", "error", "-select_streams", "v:0", "-count_packets", "-show_entries", "stream=nb_read_packets", "-of", "csv=p=0", &out])
                .output().unwrap();
            let n: u64 = String::from_utf8_lossy(&frames.stdout).trim().parse().unwrap();
            assert_eq!(n, plan.frames, "frame count");
        }
    }
}
