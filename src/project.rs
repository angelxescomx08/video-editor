//! Modelo del proyecto (JSON que escribe la IA).

use crate::ease::Ease;
use crate::util::{OneOrMany, Time};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default)]
    pub output: Output,
    /// Estilo global: "ryker" | "clean" | "meme" | "cinematic".
    #[serde(default)]
    pub style: Option<String>,
    /// Alias de archivos: {"batalla": "C:/videos/batalla.mp4"}.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Transcripciones por alias de fuente: {"batalla": "batalla.transcript.json"}. Permiten anclar con "@palabra".
    #[serde(default)]
    pub transcripts: BTreeMap<String, String>,
    /// Secuencia principal, una tras otra.
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Capas superpuestas (texto, imágenes, video, formas, partículas, subtítulos).
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Efectos globales con tiempos absolutos de salida.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Música, efectos de sonido y otras pistas.
    #[serde(default)]
    pub audio: Vec<AudioItem>,
    /// Efectos de audio por rango de tiempo.
    #[serde(default)]
    pub audio_effects: Vec<AudioFx>,
    /// Notas libres (se ignoran).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    #[serde(default = "d_w")]
    pub width: u32,
    #[serde(default = "d_h")]
    pub height: u32,
    #[serde(default = "d_fps")]
    pub fps: f64,
    #[serde(default)]
    pub background: Option<String>,
    /// Duración forzada (si no hay clips, o para recortar).
    #[serde(default)]
    pub duration: Option<Time>,
    /// Calidad 0-51 (menor = mejor). Por defecto 19.
    #[serde(default)]
    pub quality: Option<u8>,
    /// Volumen maestro (1.0).
    #[serde(default)]
    pub volume: Option<f32>,
    /// Normalizar volumen a un objetivo aproximado (LUFS, p. ej. -14).
    #[serde(default)]
    pub loudness: Option<f32>,
    /// Formato preestablecido: "youtube" (1920x1080), "shorts"/"tiktok"/"reels" (1080x1920), "square" (1080x1080).
    #[serde(default)]
    pub preset: Option<String>,
}

fn d_w() -> u32 {
    1920
}
fn d_h() -> u32 {
    1080
}
fn d_fps() -> f64 {
    30.0
}

impl Default for Output {
    fn default() -> Self {
        Output {
            width: 1920,
            height: 1080,
            fps: 30.0,
            background: None,
            duration: None,
            quality: None,
            volume: None,
            loudness: None,
            preset: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub src: Option<String>,
    /// Clip de color sólido.
    pub color: Option<String>,
    /// Clip de degradado: ["#1a0033", "#000000"].
    pub gradient: Option<Vec<String>>,
    pub gradient_angle: Option<f32>,
    #[serde(rename = "in")]
    pub in_: Option<Time>,
    pub out: Option<Time>,
    pub duration: Option<Time>,
    pub speed: Option<f64>,
    pub reverse: Option<bool>,
    /// Segundos extra congelando el último frame.
    pub freeze: Option<Time>,
    /// Repetir el clip N veces seguidas (stutter).
    pub repeat: Option<u32>,
    pub volume: Option<f32>,
    pub mute: Option<bool>,
    /// "stretch" (mantiene el tono) | "pitch" (el tono cambia con la velocidad).
    pub audio_mode: Option<String>,
    /// "cover" | "contain" | "blur" | "stretch".
    pub fit: Option<String>,
    /// Recorte normalizado [x, y, w, h] antes de escalar.
    pub crop: Option<[f32; 4]>,
    /// Transición desde el clip anterior, centrada en el corte.
    pub transition: Option<Transition>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    #[serde(rename = "type")]
    pub kind: String,
    pub duration: Option<f64>,
    pub color: Option<String>,
    pub direction: Option<String>,
    pub ease: Option<Ease>,
    pub intensity: Option<f32>,
    /// Sonido automático en el corte: "whoosh", "swoosh", "glitch"... o false.
    pub sfx: Option<Value>,
    pub sfx_volume: Option<f32>,
}

/// Efecto de video (o macro). Todos los parámetros son opcionales; cada tipo usa los suyos.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Effect {
    #[serde(rename = "type")]
    pub kind: String,
    pub start: Option<Time>,
    pub end: Option<Time>,
    pub at: Option<OneOrMany<Time>>,
    pub duration: Option<f64>,
    pub fade_in: Option<f64>,
    pub fade_out: Option<f64>,
    pub ease: Option<Ease>,
    /// "video" (solo clips) | "all" (incluye capas/textos).
    pub apply_to: Option<String>,
    pub amount: Option<f32>,
    pub intensity: Option<f32>,
    pub from: Option<Value>,
    pub to: Option<Value>,
    pub scale: Option<f32>,
    pub center: Option<[f32; 2]>,
    pub color: Option<String>,
    pub opacity: Option<f32>,
    pub radius: Option<f32>,
    pub angle: Option<f32>,
    pub size: Option<f32>,
    pub frequency: Option<f32>,
    pub rotation: Option<f32>,
    pub seed: Option<u64>,
    pub decay: Option<bool>,
    pub hold: Option<bool>,
    pub brightness: Option<f32>,
    pub contrast: Option<f32>,
    pub saturation: Option<f32>,
    pub exposure: Option<f32>,
    pub gamma: Option<f32>,
    pub hue: Option<f32>,
    pub temperature: Option<f32>,
    pub tint: Option<String>,
    pub tint_amount: Option<f32>,
    pub ratio: Option<f32>,
    pub softness: Option<f32>,
    pub threshold: Option<f32>,
    pub axis: Option<String>,
    pub rate: Option<f32>,
    pub colors: Option<Vec<String>>,
    // macros
    pub text: Option<String>,
    pub sfx: Option<Value>,
    pub volume: Option<f32>,
    pub preset: Option<String>,
}

/// Capa superpuesta.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// "text" | "image" | "video" | "rect" | "particles" | "captions"
    #[serde(rename = "type")]
    pub kind: String,
    /// Plantilla de estilo: "subtitle" | "banner" | "title" | "impact" | "watermark" | "kinetic".
    pub preset: Option<String>,
    pub start: Option<Time>,
    pub end: Option<Time>,
    pub duration: Option<f64>,
    // posición y transformación
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub position: Option<String>,
    pub anchor: Option<[f32; 2]>,
    pub scale: Option<f32>,
    pub rotation: Option<f32>,
    pub opacity: Option<f32>,
    pub blend: Option<String>,
    pub width: Option<Len>,
    pub height: Option<Len>,
    // animaciones
    pub animate_in: Option<Anim>,
    pub animate_out: Option<Anim>,
    pub animate: Option<Anim>,
    /// Movimiento lineal durante toda la capa: [dx, dy] en fracción de lienzo.
    pub drift: Option<[f32; 2]>,
    /// Zoom lento durante toda la capa (p. ej. 1.1 = termina al 110%).
    pub zoom_to: Option<f32>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    // texto
    pub text: Option<String>,
    pub font: Option<String>,
    pub size: Option<f32>,
    pub color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_width: Option<f32>,
    pub shadow_color: Option<String>,
    pub shadow_offset: Option<[f32; 2]>,
    pub shadow_blur: Option<f32>,
    pub glow_color: Option<String>,
    pub glow_radius: Option<f32>,
    pub box_color: Option<String>,
    pub box_padding: Option<[f32; 2]>,
    pub box_full_width: Option<bool>,
    pub highlight_color: Option<String>,
    pub highlight_box: Option<String>,
    pub highlight_colors: Option<Vec<String>>,
    pub letter_spacing: Option<f32>,
    pub line_height: Option<f32>,
    pub max_width: Option<Len>,
    pub align: Option<String>,
    pub uppercase: Option<bool>,
    pub italic: Option<bool>,
    pub skew: Option<f32>,
    /// "none" | "words" | "letters"
    pub reveal: Option<String>,
    pub reveal_duration: Option<f64>,
    pub word_times: Option<Vec<Time>>,
    pub echo: Option<Echo>,
    // medios
    pub src: Option<String>,
    #[serde(rename = "in")]
    pub in_: Option<Time>,
    pub out: Option<Time>,
    pub speed: Option<f64>,
    pub volume: Option<f32>,
    #[serde(rename = "loop")]
    pub loop_: Option<bool>,
    pub fit: Option<String>,
    pub crop: Option<[f32; 4]>,
    pub chroma_key: Option<String>,
    pub chroma_similarity: Option<f32>,
    pub chroma_smoothness: Option<f32>,
    pub radius: Option<f32>,
    /// Dibujar como parte del video (antes de la gradación y efectos globales). Lo activa el preset "cutaway".
    pub base: Option<bool>,
    /// Transición de entrada (centrada en `start`): mezcla el plano sin la capa con el plano con la capa.
    pub transition_in: Option<Transition>,
    /// Transición de salida (centrada en `end`).
    pub transition_out: Option<Transition>,
    // partículas
    pub particle: Option<String>,
    pub count: Option<u32>,
    pub seed: Option<u64>,
    pub colors: Option<Vec<String>>,
    pub particle_size: Option<f32>,
    pub particle_speed: Option<f32>,
    // subtítulos
    pub segments: Option<Vec<Segment>>,
    pub srt: Option<String>,
    /// Transcripción de `transcribe` (.transcript.json); sus tiempos de fuente se convierten a tiempo de salida.
    pub transcript: Option<String>,
    /// Palabras por subtítulo al usar `transcript` (5).
    pub max_words: Option<u32>,
    /// Palabras a resaltar con caja.
    pub highlight_words: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub start: Time,
    pub end: Time,
    pub text: String,
    pub word_times: Option<Vec<Time>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Echo {
    pub count: Option<u32>,
    pub spacing: Option<f32>,
    pub opacity: Option<f32>,
    pub outline: Option<bool>,
    /// "vertical" | "horizontal" | "depth"
    pub direction: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Anim {
    pub kind: String,
    pub duration: Option<f64>,
    pub intensity: Option<f32>,
    pub ease: Option<Ease>,
}

impl<'de> Deserialize<'de> for Anim {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v {
            Value::String(s) => Ok(Anim { kind: s, duration: None, intensity: None, ease: None }),
            Value::Object(m) => {
                for k in m.keys() {
                    if !["type", "duration", "intensity", "ease"].contains(&k.as_str()) {
                        return Err(serde::de::Error::custom(format!(
                            "campo desconocido '{k}' en animación (usa type, duration, intensity, ease)"
                        )));
                    }
                }
                let kind = m
                    .get("type")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| serde::de::Error::custom("animación sin 'type'"))?
                    .to_string();
                let ease = match m.get("ease") {
                    Some(e) => Some(serde_json::from_value(e.clone()).map_err(serde::de::Error::custom)?),
                    None => None,
                };
                Ok(Anim {
                    kind,
                    duration: m.get("duration").and_then(|x| x.as_f64()),
                    intensity: m.get("intensity").and_then(|x| x.as_f64()).map(|x| x as f32),
                    ease,
                })
            }
            _ => Err(serde::de::Error::custom("animación inválida: usa \"pop\" o {\"type\":\"pop\",\"duration\":0.3}")),
        }
    }
}

/// Longitud: número = píxeles, "50%" = porcentaje del lienzo.
#[derive(Clone, Copy, Debug, Serialize)]
pub enum Len {
    Px(f32),
    Pct(f32),
}

impl<'de> Deserialize<'de> for Len {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v {
            Value::Number(n) => Ok(Len::Px(n.as_f64().unwrap_or(0.0) as f32)),
            Value::String(s) => {
                let t = s.trim();
                if let Some(p) = t.strip_suffix('%') {
                    p.trim().parse::<f32>().map(Len::Pct).map_err(serde::de::Error::custom)
                } else {
                    t.trim_end_matches("px").trim().parse::<f32>().map(Len::Px).map_err(serde::de::Error::custom)
                }
            }
            _ => Err(serde::de::Error::custom("longitud inválida: usa píxeles (400) o \"50%\"")),
        }
    }
}

impl Len {
    /// Resuelve en píxeles de proyecto.
    pub fn px(&self, total: f32) -> f32 {
        match *self {
            Len::Px(v) => v,
            Len::Pct(p) => total * p / 100.0,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioItem {
    /// Archivo de audio/video.
    pub src: Option<String>,
    /// Sonido incorporado: "boom", "whoosh", "riser"...
    pub sfx: Option<String>,
    /// Momento(s) en que suena (equivale a start).
    pub at: Option<OneOrMany<Time>>,
    pub start: Option<Time>,
    pub end: Option<Time>,
    #[serde(rename = "in")]
    pub in_: Option<Time>,
    pub out: Option<Time>,
    pub volume: Option<f32>,
    pub fade_in: Option<f64>,
    pub fade_out: Option<f64>,
    #[serde(rename = "loop")]
    pub loop_: Option<bool>,
    pub speed: Option<f64>,
    pub pitch: Option<f32>,
    pub duration: Option<f64>,
    pub pan: Option<f32>,
    /// Bajar este audio cuando hay voz en los clips: 0..1 (p. ej. 0.6).
    pub duck: Option<f32>,
    /// "music" | "sfx" | "voice" (por defecto: sfx si es sfx, si no music).
    pub bus: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioFx {
    #[serde(rename = "type")]
    pub kind: String,
    pub start: Option<Time>,
    pub end: Option<Time>,
    pub at: Option<OneOrMany<Time>>,
    pub duration: Option<f64>,
    /// "master" | "clips" | "music" | "sfx"
    pub target: Option<String>,
    pub cutoff: Option<f32>,
    pub cutoff_to: Option<f32>,
    pub q: Option<f32>,
    pub volume: Option<f32>,
    pub volume_to: Option<f32>,
    pub db: Option<f32>,
    pub delay: Option<f32>,
    pub feedback: Option<f32>,
    pub mix: Option<f32>,
    pub size: Option<f32>,
    pub slice: Option<f32>,
    pub repeats: Option<u32>,
    pub bits: Option<u32>,
    pub downsample: Option<u32>,
    pub drive: Option<f32>,
    pub frequency: Option<f32>,
    pub fade: Option<f32>,
}

pub fn parse_project(text: &str) -> anyhow::Result<Project> {
    let p: Project = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!("JSON del proyecto inválido (línea {}, columna {}): {}", e.line(), e.column(), e)
    })?;
    Ok(p)
}

// ------------------------------------------------------------ listas de tipos conocidos

pub const EFFECTS: &[(&str, &str)] = &[
    ("zoom", "zoom de `from` a `to` (escala) con `ease`; `center` [x,y]"),
    ("zoom_punch", "zoom brusco a `scale` (1.25) y regreso; `hold`:true para quedarse"),
    ("shake", "temblor de cámara: `intensity` px, `frequency` Hz, `rotation` grados, `decay`"),
    ("move", "desplazamiento: `from`/`to` = [dx,dy] en fracción de lienzo"),
    ("rotate", "rotación: `from`/`to` en grados"),
    ("flash", "destello de `color` que se desvanece (`duration` 0.15)"),
    ("fade", "fundido a `color`: opacidad de `from` a `to`"),
    ("bw", "blanco y negro (`amount`)"),
    ("grade", "color: brightness, contrast, saturation, exposure, gamma, hue, temperature, tint+tint_amount"),
    ("invert", "invertir colores"),
    ("rgb_split", "aberración cromática: `amount` px, `angle`"),
    ("glitch", "glitch digital: `intensity` 0..1, `rate` cambios/seg"),
    ("blur", "desenfoque: `radius` px (o from/to)"),
    ("zoom_blur", "desenfoque radial: `amount` 0..1, `center`"),
    ("motion_blur", "desenfoque direccional: `amount` px, `angle`"),
    ("vignette", "viñeta: `amount` 0..1, `softness`"),
    ("letterbox", "franjas de cine: `ratio` (2.35) o `size` fracción, `color`"),
    ("pixelate", "pixelado: `size` px"),
    ("grain", "grano de película: `amount`"),
    ("scanlines", "líneas de TV/VHS: `amount`"),
    ("vhs", "look VHS (desfase de color, ruido, líneas)"),
    ("glow", "resplandor/bloom: `amount`, `radius`, `threshold`"),
    ("light_leak", "fugas de luz animadas: `color`/`colors`, `amount`"),
    ("mirror", "espejo: `axis` x|y"),
    ("strobe", "parpadeo de `color` a `rate` Hz"),
    ("duotone", "dos tonos: `colors` [sombras, luces]"),
    ("posterize", "posterizar: `amount` niveles"),
    ("sharpen", "enfoque: `amount`"),
    ("color_pop", "deja solo un color (`color`) y el resto en grises"),
];

pub const MACROS: &[(&str, &str)] = &[
    ("punch", "golpe en un punchline: zoom_punch + shake + flash + rgb_split + sonido boom (+ `text`)"),
    ("impact", "como punch pero más fuerte, con destello largo y sub_drop"),
    ("glitch_hit", "glitch corto + rgb_split + sonido glitch"),
    ("drop", "subida antes del momento (filtro + riser) y golpe en `at`"),
    ("beat", "pulso de zoom + flash suave (útil en cada `at` de una lista de beats)"),
    ("whip", "barrido rápido con motion blur + sonido whoosh"),
    ("bw_hit", "corte a blanco y negro con zoom y golpe durante `duration`"),
];

pub const TRANSITIONS: &[(&str, &str)] = &[
    ("cut", "corte seco"),
    ("fade", "fundido cruzado"),
    ("fade_black", "fundido a negro"),
    ("flash", "destello blanco (o `color`)"),
    ("zoom_in", "zoom-through hacia dentro con desenfoque radial"),
    ("zoom_out", "zoom hacia fuera"),
    ("whip", "barrido con motion blur (`direction`: left|right|up|down)"),
    ("slide", "deslizar empujando (`direction`)"),
    ("spin", "giro con zoom y desenfoque"),
    ("glitch", "glitch digital entre clips"),
    ("wipe", "cortinilla (`direction`)"),
    ("circle", "iris circular que se abre"),
    ("pixelate", "pixelado entre clips"),
    ("blur", "desenfoque y cambio"),
    ("shake", "corte con sacudida fuerte"),
    ("rgb", "separación RGB violenta"),
    ("stretch", "estiramiento horizontal tipo warp"),
    ("luma", "fundido por luminancia"),
];

pub const ANIMATIONS: &[(&str, &str)] = &[
    ("pop", "aparece escalando con rebote"),
    ("fade", "fundido"),
    ("slide_up", "sube desde abajo"),
    ("slide_down", "baja desde arriba"),
    ("slide_left", "entra desde la derecha"),
    ("slide_right", "entra desde la izquierda"),
    ("zoom_in", "de grande a normal"),
    ("zoom_out", "de pequeño a normal"),
    ("blur", "de desenfocado a nítido"),
    ("glitch", "aparición con glitch"),
    ("wipe", "revelado de izquierda a derecha"),
    ("spin", "gira al entrar"),
    ("bounce", "cae rebotando"),
    ("stretch", "se estira horizontalmente"),
    ("flicker", "parpadea"),
    ("shake", "entra temblando"),
    ("typewriter", "letra por letra (solo texto)"),
];

pub const LOOP_ANIMATIONS: &[(&str, &str)] = &[
    ("wiggle", "balanceo"),
    ("pulse", "late suavemente"),
    ("float", "flota arriba/abajo"),
    ("shake", "tiembla"),
    ("flicker", "parpadea"),
    ("spin", "gira continuamente"),
    ("glitch", "glitch intermitente"),
];
