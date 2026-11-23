//! Operaciones compartidas por la CLI y el servidor MCP.

use crate::project::{self, Project};
use crate::render::{self, RenderOpts};
use crate::timeline::{self, Timeline};
use crate::{analyze, media, sheet, sfx, text};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const GUIDE: &str = include_str!("../docs/GUIA_IA.md");

pub struct Loaded {
    pub project: Project,
    pub base: PathBuf,
    pub stem: String,
}

pub fn load_project_file(path: &str) -> Result<Loaded> {
    let p = Path::new(path);
    let txt = std::fs::read_to_string(p).with_context(|| format!("no se pudo leer el proyecto {path}"))?;
    let project = project::parse_project(&txt)?;
    let base = p.parent().map(|x| x.to_path_buf()).filter(|x| !x.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from("."));
    let base = std::fs::canonicalize(&base).unwrap_or(base);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "video".into());
    Ok(Loaded { project, base, stem })
}

pub fn load_project_value(v: &Value) -> Result<Loaded> {
    let project: Project = serde_json::from_value(v.clone()).map_err(|e| anyhow::anyhow!("proyecto inválido: {e}"))?;
    let base = std::env::current_dir()?;
    Ok(Loaded { project, base, stem: "video".into() })
}

fn strip_unc(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").map(String::from).unwrap_or(s)
}

pub fn prepare(l: &Loaded) -> Result<Timeline> {
    timeline::prepare(&l.project, &l.base)
}

pub fn check(l: &Loaded) -> Result<Value> {
    let tl = prepare(l)?;
    let mut v = tl.summary();
    v["ok"] = json!(tl.warnings.is_empty());
    Ok(v)
}

pub struct RenderArgs {
    pub output: Option<String>,
    pub draft: bool,
    pub scale: Option<f32>,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub codec: Option<String>,
    pub quiet: bool,
    pub progress: Option<Box<dyn Fn(u64, u64) + Send>>,
}

pub fn render(l: &Loaded, a: RenderArgs) -> Result<Value> {
    let tl = prepare(l)?;
    let out = a
        .output
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { l.base.join(p) })
        .unwrap_or_else(|| l.base.join(format!("{}{}.mp4", l.stem, if a.draft { "_borrador" } else { "" })));
    let scale = a.scale.unwrap_or(if a.draft { 0.5 } else { 1.0 }).clamp(0.1, 2.0);
    let opts = RenderOpts { scale, draft: a.draft, codec: a.codec, start: a.from, end: a.to, quiet: a.quiet, progress: a.progress };
    let st = render::render(&tl, &out, &opts)?;
    let mut v = serde_json::to_value(&st)?;
    v["output"] = json!(strip_unc(&out));
    Ok(v)
}

pub fn out_dir(base: &Path) -> PathBuf {
    let d = base.join("montaje-out");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Devuelve (info, png, ruta guardada).
pub fn preview(l: &Loaded, times: &[f64], scale: f32, cols: usize, thumb: usize, output: Option<String>) -> Result<(Value, crate::frame::Frame, String)> {
    let tl = prepare(l)?;
    let times: Vec<f64> = if times.is_empty() {
        let n = 6;
        (0..n).map(|i| tl.duration * (i as f64 + 0.5) / n as f64).collect()
    } else {
        times.to_vec()
    };
    let (frames, warnings) = render::render_stills(&tl, &times, scale)?;
    let img = if frames.len() == 1 && thumb == 0 {
        frames[0].1.clone()
    } else {
        let items: Vec<(String, crate::frame::Frame)> =
            frames.into_iter().map(|(t, f)| (format!("{} ({:.2}s)", crate::util::fmt_time(t), t), f)).collect();
        let tw = if thumb == 0 { 640 } else { thumb };
        sheet::tile(&items, cols, tw)
    };
    let png = img.encode_png()?;
    let path = output.map(PathBuf::from).unwrap_or_else(|| out_dir(&l.base).join(format!("{}_preview.png", l.stem)));
    std::fs::write(&path, &png)?;
    Ok((json!({"times": times, "image": strip_unc(&path), "width": img.w, "height": img.h, "warnings": warnings}), img, strip_unc(&path)))
}

pub fn probe(path: &str) -> Result<Value> {
    Ok(serde_json::to_value(media::probe(path)?)?)
}

pub fn analyze(path: &str, from: Option<f64>, to: Option<f64>, scenes: bool) -> Result<Value> {
    Ok(serde_json::to_value(analyze::analyze(path, from.unwrap_or(0.0), to, scenes)?)?)
}

pub fn contact_sheet(
    path: &str,
    from: Option<f64>,
    to: Option<f64>,
    count: usize,
    cols: usize,
    width: usize,
    output: Option<String>,
) -> Result<(Value, crate::frame::Frame, String)> {
    let (img, times) = sheet::contact_sheet(path, from.unwrap_or(0.0), to, count, cols, width)?;
    let png = img.encode_png()?;
    let out = match output {
        Some(o) => PathBuf::from(o),
        None => {
            let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "video".into());
            let safe: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
            let safe: String = safe.chars().take(40).collect();
            out_dir(&std::env::current_dir()?).join(format!("{safe}_sheet_{:.0}-{:.0}.png", from.unwrap_or(0.0), to.unwrap_or(0.0)))
        }
    };
    std::fs::write(&out, &png)?;
    Ok((json!({"times": times.iter().map(|t| (t * 100.0).round() / 100.0).collect::<Vec<_>>(), "image": strip_unc(&out), "width": img.w, "height": img.h}), img, strip_unc(&out)))
}

/// Transcribe y guarda `<nombre>.transcript.json` y `.srt`. Devuelve un resumen compacto.
pub fn transcribe(
    path: &str,
    model: &str,
    lang: Option<&str>,
    from: Option<f64>,
    to: Option<f64>,
    output: Option<String>,
    log: &dyn Fn(&str),
) -> Result<Value> {
    let abs = std::fs::canonicalize(path).map(|p| strip_unc(&p)).unwrap_or_else(|_| path.to_string());
    let t = crate::transcribe::transcribe(&abs, model, lang, from.unwrap_or(0.0), to, log)?;
    let out = match output {
        Some(o) => PathBuf::from(o),
        None => {
            let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "audio".into());
            let safe: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
            let suffix = match (from, to) {
                (None, None) => String::new(),
                _ => format!("_{:.0}-{:.0}", from.unwrap_or(0.0), to.unwrap_or(t.to)),
            };
            out_dir(&std::env::current_dir()?).join(format!("{safe}{suffix}.transcript.json"))
        }
    };
    std::fs::write(&out, serde_json::to_string_pretty(&t)?)?;
    let srt = out.with_extension("").with_extension("srt");
    std::fs::write(&srt, crate::transcribe::to_srt(&t))?;
    let segs: Vec<Value> = t
        .segments
        .iter()
        .map(|s| json!({"start": s.start, "end": s.end, "text": s.text}))
        .collect();
    Ok(json!({
        "transcript": strip_unc(&std::fs::canonicalize(&out).unwrap_or(out.clone())),
        "srt": strip_unc(&std::fs::canonicalize(&srt).unwrap_or(srt.clone())),
        "language": t.language,
        "model": t.model,
        "segments": segs,
        "notes": "Tiempos en segundos de la fuente. Usa el archivo en una capa {\"type\":\"captions\",\"transcript\":\"...\"}: los tiempos se convierten solos al tiempo de salida según tus clips. Los tiempos por palabra están en el archivo."
    }))
}

pub fn catalog() -> Value {
    let m = |v: &[(&str, &str)]| -> Value { v.iter().map(|(k, d)| json!({"name": k, "info": d})).collect::<Vec<_>>().into() };
    json!({
        "effects": m(project::EFFECTS),
        "macros": m(project::MACROS),
        "transitions": m(project::TRANSITIONS),
        "animations_in_out": m(project::ANIMATIONS),
        "animations_loop": m(project::LOOP_ANIMATIONS),
        "text_presets": m(timeline::TEXT_PRESETS),
        "layer_types": timeline::LAYER_KINDS,
        "particles": timeline::PARTICLES,
        "sfx": m(sfx::SFX),
        "audio_effects": m(timeline::AUDIO_FX),
        "fonts": text::font_names(),
        "styles": ["ryker", "cinematic", "meme", "shorts", "clean"],
        "output_presets": ["youtube", "720p", "4k", "shorts", "tiktok", "reels", "square"],
    })
}

pub fn sfx_to_wav(name: &str, out: &Path, duration: Option<f32>) -> Result<()> {
    let b = sfx::synth(name, duration).ok_or_else(|| anyhow::anyhow!("sfx desconocido '{name}'"))?;
    media::write_wav(out, &b, 2, media::SR)
}

pub fn template(style: &str, src: Option<&str>) -> Value {
    let src = src.unwrap_or("C:/ruta/a/tu/video.mp4");
    json!({
        "output": {"preset": "youtube", "fps": 30},
        "style": style,
        "sources": {"main": src},
        "clips": [
            {"src": "main", "in": 0, "out": 4, "label": "intro"},
            {"src": "main", "in": 10, "out": 14, "transition": {"type": "zoom_in", "duration": 0.4},
             "effects": [{"type": "punch", "at": 1.2}]}
        ],
        "layers": [
            {"type": "text", "preset": "title", "text": "TÍTULO", "start": 0, "duration": 2.5},
            {"type": "captions", "segments": [{"start": 4.2, "end": 6.0, "text": "esto es una [BARRA] de ejemplo"}]}
        ],
        "audio": [
            {"sfx": "riser", "start": 2.4, "duration": 1.6, "volume": 0.6}
        ]
    })
}
