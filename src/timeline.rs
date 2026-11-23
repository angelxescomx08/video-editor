//! Convierte un Project en una línea de tiempo resuelta: tiempos absolutos, presets, macros y estilos.

use crate::ease::Ease;
use crate::fx::{default_apply_all, default_duration, TFx};
use crate::media::{self, Crop, Fit, MediaInfo};
use crate::project::*;
use crate::transitions::TTransition;
use crate::util::*;
use anyhow::{bail, Result};
use serde_json::Value;
use std::path::Path;

pub const AUDIO_FX: &[(&str, &str)] = &[
    ("lowpass", "sonido apagado/bajo el agua: `cutoff` Hz (500), `cutoff_to` para barrido"),
    ("highpass", "quita graves: `cutoff` (1200), `cutoff_to`"),
    ("bandpass", "banda estrecha: `cutoff`, `q`"),
    ("telephone", "voz de teléfono/radio"),
    ("gain", "volumen: `volume` (o `db`), `volume_to` para rampa"),
    ("mute", "silencio"),
    ("beep", "censura con pitido"),
    ("echo", "eco: `delay` s (0.22), `feedback` (0.45), `mix` (0.6). La cola sigue después de `end`"),
    ("reverb", "reverberación: `size` 0..1, `mix`"),
    ("tape_stop", "la cinta se frena en el rango"),
    ("stutter", "repite el fragmento inicial: `slice` s (0.12), `repeats` (4)"),
    ("reverse", "invierte el audio del rango"),
    ("bitcrush", "lo-fi digital: `bits` (6), `downsample` (4)"),
    ("distortion", "saturación: `drive` (4)"),
];

pub const LAYER_KINDS: &[&str] = &["text", "image", "video", "rect", "particles", "captions"];
pub const TEXT_PRESETS: &[(&str, &str)] = &[
    ("subtitle", "barras estilo Ryker: condensada, cursiva, espaciado ancho, palabras [resaltadas] en cajas de color, aparición palabra por palabra"),
    ("banner", "palabra clave gigante en franja de color a todo el ancho (VEGETA, LUNA), entra con glitch"),
    ("title", "título grande con brillo (LOBO VS CHUTY)"),
    ("kinetic", "texto de impacto grande que aparece con pop y tiembla"),
    ("impact", "estilo meme: Impact blanco con contorno negro"),
    ("caption", "subtítulos de Shorts/TikTok: grandes, contorno, palabra actual resaltada"),
    ("lower_third", "rótulo inferior con caja semitransparente"),
    ("watermark", "marca de agua pequeña en una esquina"),
];
pub const PARTICLES: &[&str] = &["embers", "sparks", "snow", "stars", "dust", "confetti", "bokeh", "rain"];

#[derive(Clone, Debug)]
pub enum ClipSrc {
    Video { path: String },
    Image(String),
    Color([u8; 4]),
    Gradient(Vec<[u8; 4]>, f32),
}

#[derive(Clone, Debug)]
pub struct TClip {
    pub index: usize,
    pub label: Option<String>,
    pub start: f64,
    pub dur: f64,
    pub src: ClipSrc,
    pub in_: f64,
    pub out: f64,
    pub speed: f64,
    pub reverse: bool,
    pub repeat: u32,
    pub single: f64,
    pub freeze: f64,
    pub volume: f32,
    pub has_audio: bool,
    pub stretch: bool,
    pub fit: Fit,
    pub crop: Crop,
    pub trans_in: Option<TTransition>,
    pub pre: f64,
    pub post: f64,
    pub effects: Vec<TFx>,
    pub src_dur: f64,
    pub src_fps: f64,
}

impl TClip {
    /// Tiempo de la fuente para el tiempo local τ (puede ser negativo o mayor que dur por las asas).
    /// Devuelve (tiempo, congelado).
    pub fn src_time(&self, tau: f64) -> (f64, bool) {
        let play = self.single * self.repeat as f64;
        let fwd = |d: f64| if self.reverse { self.out - d * self.speed } else { self.in_ + d * self.speed };
        let last = if self.reverse { self.in_ } else { (self.out - 0.5 / self.src_fps.max(1.0)).max(self.in_) };
        let (s, frozen) = if tau < 0.0 {
            (fwd(tau), false)
        } else if tau < play {
            let t = if self.repeat > 1 && self.single > 0.0 { tau % self.single } else { tau };
            (fwd(t), false)
        } else if self.freeze > 0.0 {
            (last, true)
        } else {
            // asa posterior: continúa más allá de `out`
            (fwd(self.single + (tau - play)), false)
        };
        let maxt = (self.src_dur - 0.5 / self.src_fps.max(1.0)).max(0.0);
        (s.clamp(0.0, maxt.max(0.0)), frozen)
    }
    pub fn end(&self) -> f64 {
        self.start + self.dur
    }
}

#[derive(Clone, Debug)]
pub struct TLayer {
    pub l: Layer,
    pub start: f64,
    pub end: f64,
    pub effects: Vec<TFx>,
    pub path: Option<String>,
    pub info: Option<MediaInfo>,
}

#[derive(Clone, Debug)]
pub enum AudioSrc {
    File(String),
    Synth(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Bus {
    Clips,
    Music,
    Sfx,
}

#[derive(Clone, Debug)]
pub struct TAudio {
    pub src: AudioSrc,
    pub start: f64,
    pub in_: f64,
    pub out: Option<f64>,
    pub end: Option<f64>,
    pub volume: f32,
    pub fade_in: f64,
    pub fade_out: f64,
    pub loop_: bool,
    pub speed: f64,
    pub pitch: f32,
    pub duration: Option<f32>,
    pub pan: f32,
    pub duck: f32,
    pub bus: Bus,
}

#[derive(Clone, Debug)]
pub struct TAudioFx {
    pub fx: AudioFx,
    pub start: f64,
    pub end: f64,
    pub target: String,
}

pub struct Timeline {
    pub w: usize,
    pub h: usize,
    pub fps: f64,
    pub duration: f64,
    pub background: [u8; 4],
    pub quality: u8,
    pub volume: f32,
    pub loudness: Option<f32>,
    pub clips: Vec<TClip>,
    pub layers: Vec<TLayer>,
    pub fx_video: Vec<TFx>,
    pub fx_all: Vec<TFx>,
    pub audio: Vec<TAudio>,
    pub audio_fx: Vec<TAudioFx>,
    pub warnings: Vec<String>,
    pub style: Option<String>,
    pub unit: f32,
}

// ------------------------------------------------------------ ayudas

fn efx(kind: &str) -> Effect {
    Effect { kind: kind.to_string(), ..Default::default() }
}

fn sfx_choice(v: &Option<Value>, def: &str) -> Option<String> {
    match v {
        None | Some(Value::Bool(true)) | Some(Value::Null) => {
            if def.is_empty() {
                None
            } else {
                Some(def.to_string())
            }
        }
        Some(Value::Bool(false)) => None,
        Some(Value::String(s)) if s == "none" || s.is_empty() => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => Some(def.to_string()),
    }
}

fn synth_audio(name: &str, at: f64, vol: f32) -> TAudio {
    TAudio {
        src: AudioSrc::Synth(name.to_string()),
        start: at,
        in_: 0.0,
        out: None,
        end: None,
        volume: vol,
        fade_in: 0.0,
        fade_out: 0.0,
        loop_: false,
        speed: 1.0,
        pitch: 1.0,
        duration: None,
        pan: 0.0,
        duck: 0.0,
        bus: Bus::Sfx,
    }
}

macro_rules! fill {
    ($dst:expr, $src:expr, $($f:ident),*) => {
        $( if $dst.$f.is_none() { $dst.$f = $src.$f.clone(); } )*
    };
}

fn merge_layer(l: &mut Layer, d: &Layer) {
    fill!(
        l, d, x, y, position, anchor, scale, rotation, opacity, blend, width, height, animate_in, animate_out, animate,
        drift, zoom_to, text, font, size, color, stroke_color, stroke_width, shadow_color, shadow_offset, shadow_blur,
        glow_color, glow_radius, box_color, box_padding, box_full_width, highlight_color, highlight_box,
        highlight_colors, letter_spacing, line_height, max_width, align, uppercase, italic, skew, reveal,
        reveal_duration, echo, radius, particle, count, colors, particle_size, particle_speed
    );
    if l.effects.is_empty() && !d.effects.is_empty() {
        l.effects = d.effects.clone();
    }
}

fn anim(kind: &str, d: f64) -> Option<Anim> {
    Some(Anim { kind: kind.into(), duration: Some(d), intensity: None, ease: None })
}

pub fn text_preset(name: &str, unit: f32, style: Option<&str>) -> Option<Layer> {
    let u = unit;
    let ryker = style == Some("ryker");
    let l = match name {
        "subtitle" | "bars" | "ryker" => Layer {
            font: Some("bebas".into()),
            size: Some(58.0 * u),
            color: Some("#ffffff".into()),
            letter_spacing: Some(0.3),
            italic: Some(true),
            uppercase: Some(true),
            highlight_colors: Some(vec![
                "#1565c0".into(),
                "#c79a1e".into(),
                "#b71c1c".into(),
                "#0e7c86".into(),
                "#7b2cff".into(),
            ]),
            highlight_color: Some("#ffffff".into()),
            shadow_color: Some("#000000b0".into()),
            shadow_offset: Some([0.0, 3.0 * u]),
            shadow_blur: Some(6.0 * u),
            reveal: Some("words".into()),
            y: Some(if ryker { 0.5 } else { 0.8 }),
            max_width: Some(Len::Pct(85.0)),
            animate_out: anim("fade", 0.12),
            ..Default::default()
        },
        "banner" => Layer {
            font: Some("archivo".into()),
            size: Some(200.0 * u),
            color: Some("#ffffff".into()),
            italic: Some(true),
            uppercase: Some(true),
            box_color: Some("#1b5aa6".into()),
            box_padding: Some([60.0 * u, 34.0 * u]),
            box_full_width: Some(true),
            animate_in: anim("glitch", 0.28),
            animate_out: anim("glitch", 0.18),
            letter_spacing: Some(-0.01),
            line_height: Some(0.95),
            ..Default::default()
        },
        "title" => Layer {
            font: Some("archivo".into()),
            size: Some(170.0 * u),
            color: Some("#7b2cff".into()),
            uppercase: Some(true),
            line_height: Some(0.85),
            glow_color: Some("#7b2cffcc".into()),
            glow_radius: Some(28.0 * u),
            max_width: Some(Len::Pct(80.0)),
            animate_in: anim("blur", 0.45),
            animate_out: anim("fade", 0.3),
            ..Default::default()
        },
        "kinetic" => Layer {
            font: Some("anton".into()),
            size: Some(150.0 * u),
            color: Some("#ffffff".into()),
            uppercase: Some(true),
            italic: Some(true),
            stroke_color: Some("#000000".into()),
            stroke_width: Some(6.0 * u),
            shadow_color: Some("#000000aa".into()),
            shadow_offset: Some([0.0, 8.0 * u]),
            shadow_blur: Some(12.0 * u),
            animate_in: anim("pop", 0.22),
            animate_out: anim("zoom_out", 0.15),
            animate: Some(Anim { kind: "shake".into(), duration: None, intensity: Some(0.5), ease: None }),
            max_width: Some(Len::Pct(90.0)),
            ..Default::default()
        },
        "impact" | "meme" => Layer {
            font: Some("impact".into()),
            size: Some(120.0 * u),
            color: Some("#ffffff".into()),
            uppercase: Some(true),
            stroke_color: Some("#000000".into()),
            stroke_width: Some(7.0 * u),
            animate_in: anim("pop", 0.2),
            max_width: Some(Len::Pct(90.0)),
            ..Default::default()
        },
        "caption" | "tiktok" | "shorts" => Layer {
            font: Some("anton".into()),
            size: Some(92.0 * u),
            color: Some("#ffffff".into()),
            uppercase: Some(true),
            stroke_color: Some("#000000".into()),
            stroke_width: Some(7.0 * u),
            shadow_color: Some("#00000099".into()),
            shadow_offset: Some([0.0, 6.0 * u]),
            shadow_blur: Some(8.0 * u),
            highlight_color: Some("#ffd400".into()),
            reveal: Some("words".into()),
            y: Some(0.72),
            max_width: Some(Len::Pct(85.0)),
            animate_in: anim("pop", 0.15),
            ..Default::default()
        },
        "lower_third" => Layer {
            font: Some("bebas".into()),
            size: Some(64.0 * u),
            color: Some("#ffffff".into()),
            letter_spacing: Some(0.12),
            box_color: Some("#000000b0".into()),
            box_padding: Some([28.0 * u, 14.0 * u]),
            x: Some(0.06),
            y: Some(0.84),
            anchor: Some([0.0, 0.5]),
            align: Some("left".into()),
            animate_in: anim("wipe", 0.35),
            animate_out: anim("fade", 0.25),
            ..Default::default()
        },
        "watermark" => Layer {
            font: Some("bebas".into()),
            size: Some(38.0 * u),
            color: Some("#ffffff".into()),
            letter_spacing: Some(0.25),
            opacity: Some(0.75),
            x: Some(0.97),
            y: Some(0.95),
            anchor: Some([1.0, 1.0]),
            ..Default::default()
        },
        _ => return None,
    };
    Some(l)
}

fn position_xy(p: &str) -> Option<(f32, f32, [f32; 2])> {
    Some(match p {
        "center" => (0.5, 0.5, [0.5, 0.5]),
        "top" => (0.5, 0.1, [0.5, 0.0]),
        "bottom" => (0.5, 0.9, [0.5, 1.0]),
        "left" => (0.05, 0.5, [0.0, 0.5]),
        "right" => (0.95, 0.5, [1.0, 0.5]),
        "top_left" | "top-left" => (0.04, 0.06, [0.0, 0.0]),
        "top_right" | "top-right" => (0.96, 0.06, [1.0, 0.0]),
        "bottom_left" | "bottom-left" => (0.04, 0.94, [0.0, 1.0]),
        "bottom_right" | "bottom-right" => (0.96, 0.94, [1.0, 1.0]),
        "lower_third" => (0.5, 0.8, [0.5, 0.5]),
        "upper_third" => (0.5, 0.25, [0.5, 0.5]),
        _ => return None,
    })
}

fn parse_srt(text: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let (a, b) = lines[ti].split_once("-->").unwrap();
        let conv = |s: &str| parse_time(&s.trim().replace(',', "."));
        if let (Ok(s), Ok(e)) = (conv(a), conv(b)) {
            let text = lines[ti + 1..].join(" ");
            out.push(Segment { start: Time(s), end: Time(e), text, word_times: None });
        }
    }
    out
}

/// Normaliza una palabra para comparar (minúsculas, sin tildes ni signos).
pub fn norm_word(w: &str) -> String {
    w.chars()
        .filter_map(|c| {
            let c = c.to_lowercase().next().unwrap_or(c);
            let c = match c {
                'á' | 'à' | 'ä' | 'â' => 'a',
                'é' | 'è' | 'ë' | 'ê' => 'e',
                'í' | 'ì' | 'ï' | 'î' => 'i',
                'ó' | 'ò' | 'ö' | 'ô' => 'o',
                'ú' | 'ù' | 'ü' | 'û' => 'u',
                c => c,
            };
            if c.is_alphanumeric() {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

fn same_file(a: &str, b: &str) -> bool {
    let ca = std::fs::canonicalize(a).ok();
    let cb = std::fs::canonicalize(b).ok();
    match (ca, cb) {
        (Some(x), Some(y)) => x == y,
        _ => a.replace('\\', "/").to_lowercase() == b.replace('\\', "/").to_lowercase(),
    }
}

/// Convierte palabras con tiempo de fuente en subtítulos con tiempo de salida según los clips.
fn map_transcript(t: &crate::transcribe::Transcript, clips: &[TClip], max_words: usize) -> Vec<Segment> {
    let max_words = max_words.max(1);
    let mut out = Vec::new();
    for c in clips {
        let ClipSrc::Video { path } = &c.src else { continue };
        if c.reverse || !same_file(path, &t.source) {
            continue;
        }
        let play = c.single; // primera pasada (las repeticiones no se subtitulan)
        for seg in &t.segments {
            let words: Vec<crate::transcribe::Word> = if seg.words.is_empty() {
                vec![crate::transcribe::Word { word: seg.text.clone(), start: seg.start, end: seg.end }]
            } else {
                seg.words.clone()
            };
            // palabras que empiezan dentro del clip
            let inside: Vec<(String, f64, f64)> = words
                .iter()
                .filter(|w| w.start >= c.in_ - 1e-3 && w.start < c.out - 0.05)
                .map(|w| {
                    let s = c.start + (w.start - c.in_) / c.speed;
                    let e = (c.start + (w.end - c.in_) / c.speed).min(c.start + play);
                    (w.word.clone(), s, e)
                })
                .filter(|(_, s, _)| *s < c.start + play)
                .collect();
            for chunk in inside.chunks(max_words) {
                let start = chunk[0].1;
                let last_end = chunk.last().unwrap().2;
                let end = (last_end + 0.35).min(c.end()).max(start + 0.3);
                out.push(Segment {
                    start: Time(start),
                    end: Time(end),
                    text: chunk.iter().map(|w| w.0.clone()).collect::<Vec<_>>().join(" "),
                    word_times: Some(chunk.iter().map(|w| Time(w.1)).collect()),
                });
            }
        }
    }
    // evitar solapes: cada subtítulo termina cuando empieza el siguiente
    out.sort_by(|a, b| a.start.0.partial_cmp(&b.start.0).unwrap());
    for i in 0..out.len().saturating_sub(1) {
        let next = out[i + 1].start.0;
        if out[i].end.0 > next {
            out[i].end = Time(next.max(out[i].start.0 + 0.1));
        }
    }
    out
}

pub fn known_effect(k: &str) -> bool {
    EFFECTS.iter().any(|(n, _)| *n == k)
        || MACROS.iter().any(|(n, _)| *n == k)
        || matches!(k, "grayscale" | "color" | "chromatic" | "radial_blur" | "directional_blur" | "noise" | "bloom" | "flip")
}

struct Ctx<'a> {
    base: &'a Path,
    sources: &'a std::collections::BTreeMap<String, String>,
    warnings: Vec<String>,
    style: Option<String>,
    extra_audio: Vec<TAudio>,
    extra_layers: Vec<Layer>,
    extra_audio_fx: Vec<TAudioFx>,
    seed: u64,
}

impl Ctx<'_> {
    fn resolve(&self, src: &str) -> String {
        let s = self.sources.get(src).cloned().unwrap_or_else(|| src.to_string());
        let p = Path::new(&s);
        if p.is_absolute() {
            s
        } else {
            self.base.join(p).to_string_lossy().to_string()
        }
    }
    fn next_seed(&mut self) -> u64 {
        self.seed = hash64(self.seed.wrapping_add(1));
        self.seed
    }

    /// Expande macros y resuelve tiempos. `offset` = tiempo absoluto del origen de la lista.
    fn effects(&mut self, list: &[Effect], scope: (f64, f64), offset: f64, where_: &str) -> Vec<TFx> {
        let mut out = Vec::new();
        for e in list {
            if !known_effect(&e.kind) {
                self.warnings.push(format!(
                    "{where_}: efecto desconocido '{}'. Usa uno de: {}",
                    e.kind,
                    EFFECTS.iter().chain(MACROS.iter()).map(|x| x.0).collect::<Vec<_>>().join(", ")
                ));
                continue;
            }
            let ats: Vec<f64> = e.at.as_ref().map(|a| a.to_vec().into_iter().map(|t| t.0).collect()).unwrap_or_default();
            if MACROS.iter().any(|(n, _)| *n == e.kind) {
                let points = if ats.is_empty() { vec![e.start.map(|t| t.0).unwrap_or(scope.0)] } else { ats };
                for at in points {
                    let sub = self.expand_macro(e, at, offset);
                    out.extend(self.effects(&sub, scope, offset, where_));
                }
                continue;
            }
            let spans: Vec<(f64, f64)> = if !ats.is_empty() {
                ats.iter().map(|&a| (a, a + e.duration.unwrap_or_else(|| default_duration(&e.kind)))).collect()
            } else {
                let s = e.start.map(|t| t.0).unwrap_or(scope.0);
                let en = e.end.map(|t| t.0).or(e.duration.map(|d| s + d)).unwrap_or(scope.1);
                vec![(s, en)]
            };
            for (s, en) in spans {
                if en <= s {
                    self.warnings.push(format!("{where_}: efecto '{}' con duración nula ({s:.2}–{en:.2})", e.kind));
                    continue;
                }
                let seed = e.seed.unwrap_or_else(|| self.next_seed());
                out.push(TFx { e: e.clone(), start: s, end: en, seed });
            }
        }
        out
    }

    fn expand_macro(&mut self, e: &Effect, at: f64, offset: f64) -> Vec<Effect> {
        let i = e.intensity.or(e.amount).unwrap_or(1.0);
        let abs = offset + at;
        let mut v = Vec::new();
        let mk = |kind: &str, dur: f64| -> Effect {
            let mut x = efx(kind);
            x.at = one(at);
            x.duration = Some(dur);
            x.center = e.center;
            x
        };
        let text_layer = |me: &mut Self, dur: f64, preset: &str| {
            if let Some(t) = &e.text {
                me.extra_layers.push(Layer {
                    kind: "text".into(),
                    preset: Some(e.preset.clone().unwrap_or_else(|| preset.to_string())),
                    text: Some(t.clone()),
                    start: Some(Time(abs)),
                    duration: Some(e.duration.unwrap_or(dur)),
                    box_color: e.color.clone().filter(|_| preset == "banner"),
                    ..Default::default()
                });
            }
        };
        let vol = e.volume.unwrap_or(0.9);
        match e.kind.as_str() {
            "punch" | "impact" => {
                let big = e.kind == "impact";
                let k = if big { i * 1.4 } else { i };
                let mut z = mk("zoom_punch", if big { 0.7 } else { 0.55 });
                z.scale = Some(1.0 + 0.18 * k);
                v.push(z);
                let mut s = mk("shake", if big { 0.6 } else { 0.45 });
                s.intensity = Some(26.0 * k);
                v.push(s);
                let mut f = mk("flash", if big { 0.25 } else { 0.14 });
                f.opacity = Some(if big { 0.95 } else { 0.7 });
                f.color = e.color.clone().filter(|_| e.text.is_none());
                v.push(f);
                let mut r = mk("rgb_split", 0.3);
                r.amount = Some(14.0 * k);
                v.push(r);
                if big {
                    let mut g = mk("glow", 0.5);
                    g.amount = Some(0.8);
                    g.fade_out = Some(0.45);
                    v.push(g);
                }
                if let Some(name) = sfx_choice(&e.sfx, if big { "impact" } else { "boom" }) {
                    self.extra_audio.push(synth_or_file(self, &name, abs, vol));
                    if big {
                        self.extra_audio.push(synth_audio("sub_drop", abs, vol * 0.7));
                    }
                }
                let style_ryker = self.style.as_deref() == Some("ryker");
                text_layer(self, 0.9, if style_ryker { "banner" } else { "kinetic" });
            }
            "glitch_hit" => {
                let mut g = mk("glitch", e.duration.unwrap_or(0.35));
                g.intensity = Some(0.8 * i);
                v.push(g);
                let mut r = mk("rgb_split", e.duration.unwrap_or(0.35));
                r.amount = Some(18.0 * i);
                v.push(r);
                if let Some(name) = sfx_choice(&e.sfx, "glitch") {
                    self.extra_audio.push(synth_or_file(self, &name, abs, vol * 0.8));
                }
                text_layer(self, 0.8, "kinetic");
            }
            "drop" => {
                let build = e.duration.unwrap_or(1.6);
                let mut z = efx("zoom");
                z.start = Some(Time(at - build));
                z.end = Some(Time(at));
                z.from = Some(Value::from(1.0));
                z.to = Some(Value::from(1.0 + 0.1 * i as f64));
                z.ease = Some(Ease::InCubic);
                v.push(z);
                let mut b = efx("blur");
                b.start = Some(Time(at - build * 0.35));
                b.end = Some(Time(at));
                b.from = Some(Value::from(0.0));
                b.to = Some(Value::from(6.0));
                v.push(b);
                self.extra_audio.push(TAudio { duration: Some(build as f32), ..synth_audio("riser", abs - build, vol * 0.55) });
                let mut lp = AudioFx { kind: "lowpass".into(), ..Default::default() };
                lp.cutoff = Some(2500.0);
                lp.cutoff_to = Some(250.0);
                lp.target = Some("clips".into());
                self.extra_audio_fx.push(TAudioFx { fx: lp, start: abs - build, end: abs, target: "clips".into() });
                let mut p = e.clone();
                p.kind = "punch".into();
                p.duration = None;
                v.extend(self.expand_macro(&p, at, offset));
            }
            "beat" => {
                let mut z = mk("zoom_punch", 0.25);
                z.scale = Some(1.0 + 0.06 * i);
                v.push(z);
                let mut f = mk("flash", 0.1);
                f.opacity = Some(0.25 * i);
                f.color = e.color.clone();
                v.push(f);
            }
            "whip" => {
                let mut m = efx("motion_blur");
                m.at = one(at - 0.08);
                m.duration = Some(0.22);
                m.amount = Some(140.0 * i);
                m.angle = e.angle;
                v.push(m);
                let mut mv = efx("move");
                mv.at = one(at - 0.08);
                mv.duration = Some(0.22);
                mv.from = Some(serde_json::json!([0.08 * i, 0.0]));
                mv.to = Some(serde_json::json!([0.0, 0.0]));
                v.push(mv);
                if let Some(name) = sfx_choice(&e.sfx, "whoosh") {
                    self.extra_audio.push(synth_or_file(self, &name, abs - 0.4, vol * 0.7));
                }
            }
            "bw_hit" => {
                let d = e.duration.unwrap_or(1.0);
                let mut bw = mk("bw", d);
                bw.amount = Some(1.0);
                v.push(bw);
                let mut gr = mk("grade", d);
                gr.contrast = Some(1.35);
                v.push(gr);
                let mut z = mk("zoom_punch", d);
                z.scale = Some(1.12 * i.max(0.5));
                z.hold = Some(true);
                v.push(z);
                let mut s = mk("shake", 0.35);
                s.intensity = Some(22.0 * i);
                v.push(s);
                if let Some(name) = sfx_choice(&e.sfx, "boom") {
                    self.extra_audio.push(synth_or_file(self, &name, abs, vol));
                }
                text_layer(self, d, "kinetic");
            }
            _ => {}
        }
        v
    }
}

fn synth_or_file(c: &Ctx, name: &str, at: f64, vol: f32) -> TAudio {
    if crate::sfx::SFX.iter().any(|(n, _)| *n == name) {
        synth_audio(name, at, vol)
    } else {
        TAudio { src: AudioSrc::File(c.resolve(name)), ..synth_audio(name, at, vol) }
    }
}

fn transition_sfx(kind: &str) -> Option<(&'static str, f64)> {
    // (sonido, adelanto respecto al corte)
    match kind {
        "whip" | "slide" | "push" | "spin" | "stretch" | "warp" => Some(("whoosh", 0.45)),
        "zoom_in" | "zoom" | "zoom_out" => Some(("swoosh", 0.22)),
        "glitch" | "rgb" | "chromatic" | "pixelate" => Some(("glitch", 0.15)),
        "shake" => Some(("impact", 0.0)),
        _ => None,
    }
}

pub fn prepare(p: &Project, base_dir: &Path) -> Result<Timeline> {
    let mut out = p.output.clone();
    if let Some(pr) = &out.preset {
        match pr.as_str() {
            "youtube" | "landscape" | "1080p" => {
                out.width = 1920;
                out.height = 1080;
            }
            "720p" => {
                out.width = 1280;
                out.height = 720;
            }
            "4k" => {
                out.width = 3840;
                out.height = 2160;
            }
            "shorts" | "tiktok" | "reels" | "vertical" => {
                out.width = 1080;
                out.height = 1920;
            }
            "square" | "instagram" => {
                out.width = 1080;
                out.height = 1080;
            }
            other => bail!("output.preset desconocido '{other}' (youtube, 720p, 4k, shorts, tiktok, reels, square)"),
        }
    }
    let w = (out.width.max(16) & !1) as usize;
    let h = (out.height.max(16) & !1) as usize;
    let unit = w.min(h) as f32 / 1080.0;
    let style = p.style.clone().filter(|s| s != "none");
    let mut c = Ctx {
        base: base_dir,
        sources: &p.sources,
        warnings: Vec::new(),
        style: style.clone(),
        extra_audio: Vec::new(),
        extra_layers: Vec::new(),
        extra_audio_fx: Vec::new(),
        seed: 0xC0FFEE,
    };
    if let Some(s) = &style {
        if !["ryker", "clean", "meme", "cinematic", "shorts", "tiktok"].contains(&s.as_str()) {
            c.warnings.push(format!("style '{s}' desconocido (ryker, cinematic, meme, shorts, clean)"));
        }
    }
    let fps = if out.fps > 0.0 { out.fps.clamp(1.0, 120.0) } else { 30.0 };

    // ---------------- clips
    let mut clips: Vec<TClip> = Vec::new();
    let mut t = 0.0f64;
    for (i, cl) in p.clips.iter().enumerate() {
        let where_ = format!("clips[{i}]");
        let speed = cl.speed.unwrap_or(1.0);
        if speed <= 0.0 {
            bail!("{where_}: speed debe ser > 0 (usa \"reverse\": true para invertir)");
        }
        let (src, src_dur, src_fps, has_audio) = if let Some(s) = &cl.src {
            let path = c.resolve(s);
            let info = media::probe(&path).map_err(|e| anyhow::anyhow!("{where_}: {e}"))?;
            match info.kind.as_str() {
                "image" => (ClipSrc::Image(path), f64::MAX, fps, false),
                "audio" => bail!("{where_}: '{s}' es solo audio; ponlo en \"audio\""),
                _ => (ClipSrc::Video { path }, info.duration, info.fps.max(1.0), info.has_audio),
            }
        } else if let Some(g) = &cl.gradient {
            let cols: Vec<[u8; 4]> = g.iter().filter_map(|x| parse_color(x).ok()).collect();
            if cols.len() < 2 {
                bail!("{where_}: gradient necesita al menos 2 colores");
            }
            (ClipSrc::Gradient(cols, cl.gradient_angle.unwrap_or(90.0)), f64::MAX, fps, false)
        } else {
            let col = cl.color.as_deref().unwrap_or("#000000");
            let col = parse_color(col).map_err(|e| anyhow::anyhow!("{where_}: {e}"))?;
            (ClipSrc::Color(col), f64::MAX, fps, false)
        };
        let is_video = matches!(src, ClipSrc::Video { .. });
        let in_ = cl.in_.map(|x| x.0).unwrap_or(0.0).max(0.0);
        let freeze = cl.freeze.map(|x| x.0).unwrap_or(0.0).max(0.0);
        let out_t = if is_video {
            let o = if let Some(o) = cl.out {
                o.0
            } else if let Some(d) = cl.duration {
                in_ + d.0 * speed
            } else if freeze > 0.0 {
                in_
            } else {
                src_dur
            };
            if o > src_dur + 0.05 {
                c.warnings.push(format!("{where_}: out={o:.2} supera la duración de la fuente ({src_dur:.2}); se recorta"));
            }
            if in_ >= src_dur {
                bail!("{where_}: in={in_:.2} está fuera de la fuente (dura {src_dur:.2} s)");
            }
            o.min(src_dur)
        } else {
            in_ + cl.duration.map(|d| d.0).unwrap_or(3.0)
        };
        if out_t < in_ {
            bail!("{where_}: out ({out_t:.2}) es menor que in ({in_:.2})");
        }
        let single = if is_video { (out_t - in_) / speed } else { out_t - in_ };
        let repeat = cl.repeat.unwrap_or(1).max(1);
        let dur = single * repeat as f64 + freeze;
        if dur <= 0.0 {
            bail!("{where_}: duración nula (revisa in/out/duration/freeze)");
        }
        let trans_in = cl.transition.as_ref().filter(|_| i > 0).and_then(|tr| {
            if !TRANSITIONS.iter().any(|(n, _)| *n == tr.kind) && !["crossfade", "dissolve", "push", "iris", "dip", "warp"].contains(&tr.kind.as_str()) {
                c.warnings.push(format!("{where_}: transición desconocida '{}'", tr.kind));
            }
            let d = tr.duration.unwrap_or(match tr.kind.as_str() {
                "cut" => 0.0,
                "flash" => 0.3,
                "glitch" | "rgb" => 0.35,
                "whip" => 0.35,
                _ => 0.5,
            });
            Some(TTransition {
                kind: tr.kind.clone(),
                duration: d,
                color: color_or(&tr.color, [255, 255, 255, 255]),
                direction: tr.direction.clone().unwrap_or_else(|| "left".into()),
                ease: tr.ease,
                intensity: tr.intensity.unwrap_or(1.0),
                seed: hash64(i as u64 * 7919),
            })
        });
        let fit = Fit::parse(cl.fit.as_deref());
        clips.push(TClip {
            index: i,
            label: cl.label.clone(),
            start: t,
            dur,
            src,
            in_,
            out: out_t,
            speed,
            reverse: cl.reverse.unwrap_or(false),
            repeat,
            single,
            freeze,
            volume: if cl.mute.unwrap_or(false) { 0.0 } else { cl.volume.unwrap_or(1.0) },
            has_audio,
            stretch: cl.audio_mode.as_deref() != Some("pitch"),
            fit,
            crop: cl.crop,
            trans_in,
            pre: 0.0,
            post: 0.0,
            effects: Vec::new(),
            src_dur,
            src_fps,
        });
        t += dur;
    }
    // limitar transiciones y calcular asas
    for i in 1..clips.len() {
        let maxd = clips[i - 1].dur.min(clips[i].dur) * 0.95;
        if let Some(tr) = clips[i].trans_in.as_mut() {
            if tr.duration > maxd {
                tr.duration = maxd;
            }
            let d = tr.duration;
            clips[i].pre = d / 2.0;
            clips[i - 1].post = d / 2.0;
        }
    }
    let seq_end = t;

    // efectos de clip (tiempos locales)
    for i in 0..clips.len() {
        let list = &p.clips[i].effects;
        let dur = clips[i].dur;
        let start = clips[i].start;
        let fx = c.effects(list, (0.0, dur), start, &format!("clips[{i}].effects"));
        clips[i].effects = fx;
    }

    // sonidos de transición
    for i in 1..clips.len() {
        if let (Some(tr), Some(spec)) = (&clips[i].trans_in, &p.clips[i].transition) {
            let def = transition_sfx(&tr.kind);
            let cut = clips[i].start;
            match (&spec.sfx, def) {
                (Some(Value::Bool(false)), _) => {}
                (Some(Value::String(s)), d) => {
                    let lead = d.map(|x| x.1).unwrap_or(0.2);
                    c.extra_audio.push(synth_or_file(&c, s, (cut - lead).max(0.0), spec.sfx_volume.unwrap_or(0.6)));
                }
                (_, Some((name, lead))) => {
                    c.extra_audio.push(synth_audio(name, (cut - lead).max(0.0), spec.sfx_volume.unwrap_or(0.55)));
                }
                _ => {}
            }
        }
    }

    // ---------------- capas (se expanden subtítulos)
    let mut raw_layers: Vec<Layer> = Vec::new();
    for (i, l) in p.layers.iter().enumerate() {
        if !LAYER_KINDS.contains(&l.kind.as_str()) {
            c.warnings.push(format!("layers[{i}]: tipo desconocido '{}' ({})", l.kind, LAYER_KINDS.join(", ")));
            continue;
        }
        if l.kind == "captions" {
            let mut segs = l.segments.clone().unwrap_or_default();
            if let Some(srt) = &l.srt {
                let path = c.resolve(srt);
                match std::fs::read_to_string(&path) {
                    Ok(txt) => segs.extend(parse_srt(&txt)),
                    Err(e) => c.warnings.push(format!("layers[{i}]: no se pudo leer {path}: {e}")),
                }
            }
            if let Some(tr) = &l.transcript {
                let path = c.resolve(tr);
                match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|t| serde_json::from_str::<crate::transcribe::Transcript>(&t).map_err(|e| e.to_string()))
                {
                    Ok(t) => {
                        let mapped = map_transcript(&t, &clips, l.max_words.unwrap_or(5) as usize);
                        if mapped.is_empty() {
                            c.warnings.push(format!(
                                "layers[{i}]: ninguna palabra de la transcripción cae dentro de los clips de {}",
                                t.source
                            ));
                        }
                        segs.extend(mapped);
                    }
                    Err(e) => c.warnings.push(format!("layers[{i}]: no se pudo leer la transcripción {path}: {e}")),
                }
            }
            let hl: Vec<String> = l.highlight_words.clone().unwrap_or_default().iter().map(|w| norm_word(w)).collect();
            for s in segs {
                let mut tl = l.clone();
                tl.kind = "text".into();
                tl.segments = None;
                tl.srt = None;
                tl.transcript = None;
                tl.highlight_words = None;
                tl.max_words = None;
                let text = if hl.is_empty() {
                    s.text.clone()
                } else {
                    s.text
                        .split_whitespace()
                        .map(|w| if !w.contains('[') && hl.contains(&norm_word(w)) { format!("[{w}]") } else { w.to_string() })
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                tl.text = Some(text);
                tl.start = Some(s.start);
                tl.end = Some(s.end);
                tl.duration = None;
                tl.word_times = s.word_times.clone();
                if tl.preset.is_none() {
                    tl.preset = Some(match style.as_deref() {
                        Some("shorts") | Some("tiktok") => "caption".into(),
                        _ => "subtitle".into(),
                    });
                }
                raw_layers.push(tl);
            }
        } else {
            raw_layers.push(l.clone());
        }
    }
    raw_layers.extend(std::mem::take(&mut c.extra_layers));

    // duración total
    let layer_end_max = raw_layers
        .iter()
        .filter_map(|l| {
            let s = l.start.map(|t| t.0).unwrap_or(0.0);
            l.end.map(|e| e.0).or(l.duration.map(|d| s + d))
        })
        .fold(0.0f64, f64::max);
    let mut duration = out.duration.map(|d| d.0).unwrap_or(if seq_end > 0.0 { seq_end } else { layer_end_max });
    if duration <= 0.0 {
        // solo audio
        duration = p
            .audio
            .iter()
            .filter_map(|a| a.end.map(|e| e.0).or(a.duration.map(|d| a.start.map(|s| s.0).unwrap_or(0.0) + d)))
            .fold(0.0, f64::max);
    }
    if duration <= 0.0 {
        bail!("el proyecto no tiene duración: añade clips, capas con end/duration o output.duration");
    }

    let mut layers = Vec::new();
    for (i, mut l) in raw_layers.into_iter().enumerate() {
        let where_ = format!("layers[{i}]");
        let default_preset = match (l.kind.as_str(), style.as_deref()) {
            ("text", Some("ryker")) => Some("subtitle"),
            ("text", Some("meme")) => Some("impact"),
            ("text", Some("shorts")) | ("text", Some("tiktok")) => Some("caption"),
            _ => None,
        };
        if let Some(pn) = l.preset.clone().or(default_preset.map(String::from)) {
            match text_preset(&pn, unit, style.as_deref()) {
                Some(d) => merge_layer(&mut l, &d),
                None => c.warnings.push(format!(
                    "{where_}: preset '{pn}' desconocido ({})",
                    TEXT_PRESETS.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")
                )),
            }
        }
        if let Some(pos) = &l.position {
            match position_xy(pos) {
                Some((x, y, a)) => {
                    l.x = l.x.or(Some(x));
                    l.y = l.y.or(Some(y));
                    l.anchor = l.anchor.or(Some(a));
                }
                None => c.warnings.push(format!("{where_}: position '{pos}' desconocida")),
            }
        }
        for (an, list) in [(&l.animate_in, ANIMATIONS), (&l.animate_out, ANIMATIONS), (&l.animate, LOOP_ANIMATIONS)] {
            if let Some(a) = an {
                if !list.iter().any(|(n, _)| *n == a.kind) && a.kind != "none" {
                    c.warnings.push(format!(
                        "{where_}: animación '{}' desconocida ({})",
                        a.kind,
                        list.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }
        let start = l.start.map(|t| t.0).unwrap_or(0.0);
        let end = l.end.map(|t| t.0).or(l.duration.map(|d| start + d)).unwrap_or(duration);
        if end <= start {
            c.warnings.push(format!("{where_}: end ({end:.2}) <= start ({start:.2}); se ignora"));
            continue;
        }
        if l.kind == "text" && l.text.as_deref().unwrap_or("").trim().is_empty() {
            c.warnings.push(format!("{where_}: capa de texto sin 'text'"));
            continue;
        }
        if let Some(f) = &l.font {
            if let Err(e) = crate::text::load_font(f) {
                c.warnings.push(format!("{where_}: {e}; se usa anton"));
                l.font = Some("anton".into());
            }
        }
        let (path, info) = if matches!(l.kind.as_str(), "image" | "video") {
            let Some(s) = &l.src else {
                c.warnings.push(format!("{where_}: capa {} sin 'src'", l.kind));
                continue;
            };
            let path = c.resolve(s);
            match media::probe(&path) {
                Ok(info) => (Some(path), Some(info)),
                Err(e) => {
                    c.warnings.push(format!("{where_}: {e}"));
                    continue;
                }
            }
        } else {
            (None, None)
        };
        if l.kind == "particles" {
            if let Some(pk) = &l.particle {
                if !PARTICLES.contains(&pk.as_str()) {
                    c.warnings.push(format!("{where_}: partícula '{pk}' desconocida ({})", PARTICLES.join(", ")));
                }
            }
        }
        let effects = c.effects(&l.effects.clone(), (0.0, end - start), start, &format!("{where_}.effects"));
        layers.push(TLayer { l, start, end, effects, path, info });
    }

    // ---------------- efectos globales
    let mut gfx = c.effects(&p.effects, (0.0, duration), 0.0, "effects");
    // estilo
    let mut style_video = Vec::new();
    let mut style_all = Vec::new();
    match style.as_deref() {
        Some("ryker") => {
            let mut g = efx("grade");
            g.contrast = Some(1.18);
            g.saturation = Some(0.82);
            g.exposure = Some(-0.12);
            g.tint = Some("#3a4cff".into());
            g.tint_amount = Some(0.1);
            style_video.push(g);
            let mut v = efx("vignette");
            v.amount = Some(0.5);
            style_video.push(v);
            let mut lb = efx("letterbox");
            lb.ratio = Some(2.35);
            style_all.push(lb);
            let mut gr = efx("grain");
            gr.amount = Some(0.035);
            style_all.push(gr);
        }
        Some("cinematic") => {
            let mut g = efx("grade");
            g.contrast = Some(1.12);
            g.saturation = Some(0.9);
            g.temperature = Some(0.15);
            g.tint = Some("#0f6f7a".into());
            g.tint_amount = Some(0.12);
            style_video.push(g);
            let mut v = efx("vignette");
            v.amount = Some(0.4);
            style_video.push(v);
            let mut lb = efx("letterbox");
            lb.ratio = Some(2.39);
            style_all.push(lb);
            let mut gr = efx("grain");
            gr.amount = Some(0.03);
            style_all.push(gr);
        }
        _ => {}
    }
    let sv = c.effects(&style_video, (0.0, duration), 0.0, "style");
    let sa = c.effects(&style_all, (0.0, duration), 0.0, "style");
    // los del estilo van primero (base), los explícitos después
    let mut fx_video = sv;
    let mut fx_all = Vec::new();
    for f in gfx.drain(..) {
        let all = match f.e.apply_to.as_deref() {
            Some("all") => true,
            Some("video") => false,
            _ => default_apply_all(&f.e.kind),
        };
        if all {
            fx_all.push(f);
        } else {
            fx_video.push(f);
        }
    }
    fx_all.extend(sa);

    // ---------------- audio
    let mut audio = Vec::new();
    for (i, a) in p.audio.iter().enumerate() {
        let where_ = format!("audio[{i}]");
        let src = if let Some(s) = &a.sfx {
            if crate::sfx::SFX.iter().any(|(n, _)| n == s) {
                AudioSrc::Synth(s.clone())
            } else {
                let path = c.resolve(s);
                if Path::new(&path).exists() {
                    AudioSrc::File(path)
                } else {
                    c.warnings.push(format!(
                        "{where_}: sfx '{s}' no existe. Incorporados: {}",
                        crate::sfx::SFX.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")
                    ));
                    continue;
                }
            }
        } else if let Some(s) = &a.src {
            let path = c.resolve(s);
            if let Err(e) = media::probe(&path) {
                c.warnings.push(format!("{where_}: {e}"));
                continue;
            }
            AudioSrc::File(path)
        } else {
            c.warnings.push(format!("{where_}: falta 'src' o 'sfx'"));
            continue;
        };
        let bus = match a.bus.as_deref() {
            Some("music") => Bus::Music,
            Some("sfx") => Bus::Sfx,
            Some("voice") | Some("clips") => Bus::Clips,
            _ => {
                if a.sfx.is_some() {
                    Bus::Sfx
                } else {
                    Bus::Music
                }
            }
        };
        let starts: Vec<f64> = match &a.at {
            Some(at) => at.to_vec().into_iter().map(|t| t.0).collect(),
            None => vec![a.start.map(|t| t.0).unwrap_or(0.0)],
        };
        for s in starts {
            audio.push(TAudio {
                src: src.clone(),
                start: s,
                in_: a.in_.map(|t| t.0).unwrap_or(0.0),
                out: a.out.map(|t| t.0),
                end: a.end.map(|t| t.0),
                volume: a.volume.unwrap_or(if bus == Bus::Music { 0.8 } else { 1.0 }),
                fade_in: a.fade_in.unwrap_or(0.0),
                fade_out: a.fade_out.unwrap_or(0.0),
                loop_: a.loop_.unwrap_or(false),
                speed: a.speed.unwrap_or(1.0).max(0.05),
                pitch: a.pitch.unwrap_or(1.0).max(0.05),
                duration: a.duration.map(|d| d as f32),
                pan: a.pan.unwrap_or(0.0),
                duck: a.duck.unwrap_or(0.0),
                bus: bus.clone(),
            });
        }
    }
    audio.extend(std::mem::take(&mut c.extra_audio));

    let mut audio_fx = Vec::new();
    for (i, f) in p.audio_effects.iter().enumerate() {
        if !AUDIO_FX.iter().any(|(n, _)| *n == f.kind) && !["delay", "censor", "muffle", "volume", "repeat", "lofi", "saturate"].contains(&f.kind.as_str()) {
            c.warnings.push(format!(
                "audio_effects[{i}]: tipo '{}' desconocido ({})",
                f.kind,
                AUDIO_FX.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")
            ));
            continue;
        }
        let target = f.target.clone().unwrap_or_else(|| "master".into());
        let starts: Vec<f64> = match &f.at {
            Some(at) => at.to_vec().into_iter().map(|t| t.0).collect(),
            None => vec![f.start.map(|t| t.0).unwrap_or(0.0)],
        };
        for s in starts {
            let def_dur = match f.kind.as_str() {
                "stutter" | "repeat" => f.slice.unwrap_or(0.12) as f64 * f.repeats.unwrap_or(4) as f64,
                "tape_stop" => 0.8,
                "echo" | "delay" | "reverb" => 0.4,
                _ => duration - s,
            };
            let e = if f.at.is_some() {
                s + f.duration.unwrap_or(def_dur)
            } else {
                f.end.map(|t| t.0).or(f.duration.map(|d| s + d)).unwrap_or(s + def_dur)
            };
            audio_fx.push(TAudioFx { fx: f.clone(), start: s, end: e, target: target.clone() });
        }
    }
    audio_fx.extend(std::mem::take(&mut c.extra_audio_fx));

    Ok(Timeline {
        w,
        h,
        fps,
        duration,
        background: color_or(&out.background, [0, 0, 0, 255]),
        quality: out.quality.unwrap_or(19).min(51),
        volume: out.volume.unwrap_or(1.0),
        loudness: out.loudness,
        clips,
        layers,
        fx_video,
        fx_all,
        audio,
        audio_fx,
        warnings: c.warnings,
        style,
        unit,
    })
}

impl Timeline {
    /// Resumen legible para la IA.
    pub fn summary(&self) -> serde_json::Value {
        let clips: Vec<Value> = self
            .clips
            .iter()
            .map(|c| {
                let src = match &c.src {
                    ClipSrc::Video { path } => path.clone(),
                    ClipSrc::Image(p) => p.clone(),
                    ClipSrc::Color(_) => "color".into(),
                    ClipSrc::Gradient(..) => "gradient".into(),
                };
                serde_json::json!({
                    "index": c.index,
                    "label": c.label,
                    "start": round3(c.start),
                    "end": round3(c.end()),
                    "src": src,
                    "in": round3(c.in_),
                    "out": round3(c.out),
                    "speed": c.speed,
                    "transition_in": c.trans_in.as_ref().map(|t| format!("{} {:.2}s", t.kind, t.duration)),
                    "effects": c.effects.len(),
                })
            })
            .collect();
        let layers: Vec<Value> = self
            .layers
            .iter()
            .map(|l| {
                serde_json::json!({
                    "type": l.l.kind,
                    "start": round3(l.start),
                    "end": round3(l.end),
                    "text": l.l.text,
                    "src": l.path,
                })
            })
            .collect();
        serde_json::json!({
            "width": self.w,
            "height": self.h,
            "fps": self.fps,
            "duration": round3(self.duration),
            "style": self.style,
            "clips": clips,
            "layers": layers,
            "effects": self.fx_video.len() + self.fx_all.len(),
            "audio_items": self.audio.len(),
            "audio_effects": self.audio_fx.len(),
            "warnings": self.warnings,
        })
    }

    pub fn clip_at(&self, t: f64) -> Option<usize> {
        if self.clips.is_empty() {
            return None;
        }
        let mut lo = 0usize;
        let mut hi = self.clips.len();
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.clips[mid].start <= t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if t >= self.clips[lo].start && t < self.clips[lo].end() + 1e-9 {
            Some(lo)
        } else {
            None
        }
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

