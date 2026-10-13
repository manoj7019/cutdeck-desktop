//! Native background removal: U²-Net models run with ONNX Runtime on all CPU cores.
//!
//! * `fast` = u2netp (4.6 MB) — used for live preview and by default for export.
//! * `best` = silueta (43 MB) — a distilled full U²-Net, cleaner edges, ~4x slower.
//!
//! Both are salient-object models, so they cut out people, pets, cars, products…
//! The models were re-exported with a fixed 1×3×320×320 input (see scripts/prepare-models.py).

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub const SIZE: usize = 320;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

pub struct Cutout {
    dir: PathBuf,
    search: Vec<PathBuf>,
    sessions: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
}

static RUNTIME: OnceLock<Result<(), String>> = OnceLock::new();

fn ort_lib_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["onnxruntime.dll"]
    } else if cfg!(target_os = "macos") {
        &["libonnxruntime.dylib", "libonnxruntime.1.22.0.dylib"]
    } else {
        &["libonnxruntime.so", "libonnxruntime.so.1", "libonnxruntime.so.1.22.0"]
    }
}

/// Load the ONNX Runtime shared library once (bundled next to the models).
pub fn init_runtime(search: &[PathBuf]) -> Result<(), String> {
    RUNTIME
        .get_or_init(|| {
            let mut dirs: Vec<PathBuf> = search.to_vec();
            if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
                if let Some(d) = Path::new(&p).parent() {
                    dirs.insert(0, d.to_path_buf());
                }
            }
            for d in &dirs {
                for n in ort_lib_names() {
                    let p = d.join(n);
                    if p.is_file() {
                        return std::panic::catch_unwind(|| ort::init_from(p.display().to_string()).commit())
                            .map_err(|_| format!("Could not load {}", p.display()))?
                            .map(|_| ())
                            .map_err(|e| format!("Could not start ONNX Runtime: {e}"));
                    }
                }
            }
            Err("ONNX Runtime library not found; background removal is unavailable.".into())
        })
        .clone()
}

impl Cutout {
    pub fn new(model_dir: PathBuf, runtime_search: Vec<PathBuf>) -> Self {
        Cutout { dir: model_dir, search: runtime_search, sessions: Mutex::new(HashMap::new()) }
    }

    pub fn model_path(&self, quality: &str) -> PathBuf {
        self.dir.join(if quality == "best" { "silueta.onnx" } else { "u2netp.onnx" })
    }

    pub fn available(&self) -> Result<(), String> {
        init_runtime(&self.search)?;
        if !self.model_path("fast").is_file() {
            return Err(format!("Cutout model missing in {}", self.dir.display()));
        }
        Ok(())
    }

    pub fn session(&self, quality: &str) -> Result<Arc<Mutex<Session>>, String> {
        self.available()?;
        let q = if quality == "best" && self.model_path("best").is_file() { "best" } else { "fast" };
        let mut map = self.sessions.lock().unwrap();
        if let Some(s) = map.get(q) {
            return Ok(s.clone());
        }
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let s = Session::builder()
            .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
            .and_then(|b| b.with_intra_threads(threads))
            .and_then(|b| b.commit_from_file(self.model_path(q)))
            .map_err(|e| format!("Could not load the cutout model: {e}"))?;
        let s = Arc::new(Mutex::new(s));
        map.insert(q.to_string(), s.clone());
        Ok(s)
    }
}

/// Bilinear sample of an interleaved 8-bit image (`ch` channels) at (x, y) in source pixels.
#[inline]
fn sample(img: &[u8], w: usize, h: usize, ch: usize, x: f32, y: f32, c: usize) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let p = |xx: usize, yy: usize| img[(yy * w + xx) * ch + c] as f32;
    let a = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
    let b = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
    a * (1.0 - fy) + b * fy
}

/// Run the model on an RGB(A) image of any size; returns a SIZE×SIZE soft mask (0..=255).
pub fn infer(session: &Mutex<Session>, img: &[u8], w: usize, h: usize, ch: usize) -> Result<Vec<u8>, String> {
    if w == 0 || h == 0 || img.len() < w * h * ch {
        return Err("empty frame".into());
    }
    let n = SIZE * SIZE;
    let mut rgb = vec![0f32; n * 3];
    let mut maxv = 1f32;
    let (sx, sy) = (w as f32 / SIZE as f32, h as f32 / SIZE as f32);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (fx, fy) = ((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5);
            for c in 0..3 {
                let v = sample(img, w, h, ch, fx, fy, c);
                maxv = maxv.max(v);
                rgb[c * n + y * SIZE + x] = v;
            }
        }
    }
    for c in 0..3 {
        for v in &mut rgb[c * n..(c + 1) * n] {
            *v = (*v / maxv - MEAN[c]) / STD[c];
        }
    }
    let input = Tensor::from_array(([1usize, 3, SIZE, SIZE], rgb)).map_err(|e| e.to_string())?;
    let mut s = session.lock().unwrap();
    let out = s.run(ort::inputs![input]).map_err(|e| format!("cutout model failed: {e}"))?;
    let (_, d) = out[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let (mn, mx) = d[..n].iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let span = (mx - mn).max(1e-6);
    Ok(d[..n].iter().map(|v| (((v - mn) / span) * 255.0).round().clamp(0.0, 255.0) as u8).collect())
}

/// Resize a SIZE×SIZE mask to w×h (bilinear).
pub fn upscale(mask: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    let (sx, sy) = (SIZE as f32 / w as f32, SIZE as f32 / h as f32);
    for y in 0..h {
        for x in 0..w {
            let v = sample(mask, SIZE, SIZE, 1, (x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5, 0);
            out[y * w + x] = v.round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Needs the bundled runtime + model (resources/), skipped otherwise.
    #[test]
    fn cuts_out_a_centered_subject() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
        let cut = Cutout::new(root.join("models"), vec![root.join("onnxruntime")]);
        if cut.available().is_err() {
            eprintln!("runtime/model not present, skipping");
            return;
        }
        // grey background with a bright disc in the middle
        let (w, h) = (200usize, 300usize);
        let mut img = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let d = (((x as f32 - 100.0).powi(2) + (y as f32 - 150.0).powi(2)) as f32).sqrt();
                let v = if d < 60.0 { [230, 80, 60] } else { [120, 125, 130] };
                img[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&v);
            }
        }
        let s = cut.session("fast").unwrap();
        let t = std::time::Instant::now();
        let m = infer(&s, &img, w, h, 3).unwrap();
        eprintln!("u2netp: {:?}", t.elapsed());
        let full = upscale(&m, w, h);
        let center = full[150 * w + 100] as u32;
        let corner = full[5 * w + 5] as u32;
        assert!(center > 200 && corner < 40, "center {center}, corner {corner}");
    }
}
