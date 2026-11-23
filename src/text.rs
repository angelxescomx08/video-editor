//! Texto: fuentes, maquetación con resaltados [palabra|color], contorno, sombra, brillo y cajas.

use crate::frame::*;
use ab_glyph::{point, Font, FontArc, GlyphId, PxScale, ScaleFont};
use anyhow::{bail, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

const BUILTIN: &[(&str, &[u8])] = &[
    ("anton", include_bytes!("../assets/fonts/Anton-Regular.ttf")),
    ("bebas", include_bytes!("../assets/fonts/BebasNeue-Regular.ttf")),
    ("archivo", include_bytes!("../assets/fonts/ArchivoBlack-Regular.ttf")),
    ("bangers", include_bytes!("../assets/fonts/Bangers-Regular.ttf")),
];

const SYSTEM: &[(&str, &str)] = &[
    ("impact", "impact.ttf"),
    ("arial", "arial.ttf"),
    ("arial-bold", "arialbd.ttf"),
    ("arial-black", "ariblk.ttf"),
    ("bahnschrift", "bahnschrift.ttf"),
    ("segoe", "segoeui.ttf"),
    ("segoe-bold", "segoeuib.ttf"),
    ("verdana-bold", "verdanab.ttf"),
    ("georgia", "georgia.ttf"),
    ("times", "times.ttf"),
    ("consolas", "consola.ttf"),
    ("comic", "comic.ttf"),
];

pub fn font_names() -> Vec<String> {
    let mut v: Vec<String> = BUILTIN.iter().map(|(n, _)| n.to_string()).collect();
    for (n, file) in SYSTEM {
        if system_font_path(file).is_some() {
            v.push(n.to_string());
        }
    }
    v
}

fn system_font_path(file: &str) -> Option<std::path::PathBuf> {
    let mut dirs = vec![];
    if let Ok(w) = std::env::var("WINDIR") {
        dirs.push(std::path::PathBuf::from(w).join("Fonts"));
    }
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        dirs.push(std::path::PathBuf::from(l).join("Microsoft/Windows/Fonts"));
    }
    dirs.push("/usr/share/fonts/truetype".into());
    dirs.push("/Library/Fonts".into());
    for d in dirs {
        let p = d.join(file);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

static FONTS: OnceLock<Mutex<HashMap<String, FontArc>>> = OnceLock::new();

pub fn load_font(name: &str) -> Result<FontArc> {
    let key = name.trim().to_lowercase();
    let cache = FONTS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(f) = cache.lock().unwrap().get(&key) {
        return Ok(f.clone());
    }
    let alias = match key.as_str() {
        "bebas neue" | "bebasneue" | "bebas-neue" => "bebas",
        "archivo black" | "archivo-black" | "archivoblack" => "archivo",
        "anton-regular" => "anton",
        k => k,
    };
    let font = if let Some((_, data)) = BUILTIN.iter().find(|(n, _)| *n == alias) {
        FontArc::try_from_slice(data)?
    } else if let Some((_, file)) = SYSTEM.iter().find(|(n, _)| *n == alias) {
        let p = system_font_path(file).ok_or_else(|| anyhow::anyhow!("fuente del sistema '{name}' no encontrada"))?;
        FontArc::try_from_vec(std::fs::read(p)?)?
    } else if std::path::Path::new(name).exists() {
        FontArc::try_from_vec(std::fs::read(name)?)?
    } else if let Some(p) = system_font_path(&format!("{alias}.ttf")).or_else(|| system_font_path(&format!("{alias}.otf"))) {
        FontArc::try_from_vec(std::fs::read(p)?)?
    } else {
        bail!("fuente desconocida '{name}'. Disponibles: {}", font_names().join(", "));
    };
    cache.lock().unwrap().insert(key, font.clone());
    Ok(font)
}

fn fallback_font() -> Option<FontArc> {
    static FB: OnceLock<Option<FontArc>> = OnceLock::new();
    FB.get_or_init(|| {
        for f in ["segoeuib.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"] {
            if let Some(p) = system_font_path(f) {
                if let Ok(d) = std::fs::read(p) {
                    if let Ok(font) = FontArc::try_from_vec(d) {
                        return Some(font);
                    }
                }
            }
        }
        None
    })
    .clone()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone)]
pub struct TextSpec {
    pub text: String,
    pub font: FontArc,
    pub size: f32,
    pub color: [u8; 4],
    pub stroke: Option<([u8; 4], f32)>,
    pub shadow: Option<([u8; 4], f32, f32, f32)>,
    pub glow: Option<([u8; 4], f32)>,
    pub box_color: Option<[u8; 4]>,
    pub box_pad: [f32; 2],
    pub min_width: f32,
    pub hl_color: Option<[u8; 4]>,
    pub hl_boxes: Vec<[u8; 4]>,
    pub letter_spacing: f32,
    pub line_height: f32,
    pub max_width: Option<f32>,
    pub align: Align,
    pub uppercase: bool,
}

#[derive(Clone, Debug)]
struct PGlyph {
    id: GlyphId,
    fb: bool,
    x: f32,
    ch_index: usize,
}

#[derive(Clone, Debug)]
pub struct LWord {
    glyphs: Vec<PGlyph>,
    pub x0: f32,
    pub x1: f32,
    pub line: usize,
    pub span: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub color: Option<[u8; 4]>,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub words: Vec<LWord>,
    pub spans: Vec<Span>,
    pub width: f32,
    pub height: f32,
    pub line_h: f32,
    pub cap_h: f32,
    pub n_chars: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Visible {
    All,
    Words(usize),
    Letters(usize),
}

/// Divide en palabras con su índice de resaltado. Sintaxis: "Texto [RESALTADO] o [VARIAS PALABRAS|#ff0000]".
fn tokenize(text: &str) -> (Vec<(String, Option<usize>)>, Vec<Span>) {
    let mut words = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut cur = String::new();
    let mut in_span: Option<usize> = None;
    let mut chars = text.chars().peekable();
    let flush = |cur: &mut String, words: &mut Vec<(String, Option<usize>)>, sp: Option<usize>| {
        if !cur.is_empty() {
            words.push((std::mem::take(cur), sp));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '[' if in_span.is_none() => {
                flush(&mut cur, &mut words, None);
                // buscar cierre y color opcional
                let mut inner = String::new();
                let mut closed = false;
                for c2 in chars.by_ref() {
                    if c2 == ']' {
                        closed = true;
                        break;
                    }
                    inner.push(c2);
                }
                let (body, color) = match inner.rsplit_once('|') {
                    Some((b, col)) if closed => (b.to_string(), crate::util::parse_color(col).ok()),
                    _ => (inner.clone(), None),
                };
                let idx = spans.len();
                spans.push(Span { color });
                in_span = Some(idx);
                for (li, line) in body.split('\n').enumerate() {
                    if li > 0 {
                        words.push(("\n".to_string(), None));
                    }
                    for w in line.split_whitespace() {
                        words.push((w.to_string(), in_span));
                    }
                }
                in_span = None;
                if !closed {
                    break;
                }
            }
            '\n' => {
                flush(&mut cur, &mut words, None);
                words.push(("\n".to_string(), None));
            }
            c if c.is_whitespace() => flush(&mut cur, &mut words, None),
            c => cur.push(c),
        }
    }
    flush(&mut cur, &mut words, None);
    (words, spans)
}

fn glyph_for(font: &FontArc, fb: &Option<FontArc>, c: char) -> (GlyphId, bool) {
    let id = font.glyph_id(c);
    if id.0 == 0 && !c.is_whitespace() {
        if let Some(f) = fb {
            let id2 = f.glyph_id(c);
            if id2.0 != 0 {
                return (id2, true);
            }
        }
    }
    (id, false)
}

pub fn layout(spec: &TextSpec) -> Layout {
    let fb = fallback_font();
    let scale = PxScale::from(spec.size);
    let sf = spec.font.as_scaled(scale);
    let sfb = fb.as_ref().map(|f| f.as_scaled(scale));
    let ls = spec.letter_spacing * spec.size;
    let text = if spec.uppercase { spec.text.to_uppercase() } else { spec.text.clone() };
    let (tokens, spans) = tokenize(&text);
    let space = sf.h_advance(spec.font.glyph_id(' ')) + ls;
    // medir palabras
    let mut ci = 0usize;
    let mut measured: Vec<(Vec<PGlyph>, f32, Option<usize>)> = Vec::new();
    let mut breaks: Vec<usize> = Vec::new();
    for (w, sp) in &tokens {
        if w == "\n" {
            breaks.push(measured.len());
            continue;
        }
        let mut x = 0.0f32;
        let mut gl = Vec::new();
        let mut prev: Option<(GlyphId, bool)> = None;
        let n = w.chars().count();
        for (i, c) in w.chars().enumerate() {
            let (id, isfb) = glyph_for(&spec.font, &fb, c);
            if let Some((p, pfb)) = prev {
                if pfb == isfb && !isfb {
                    x += sf.kern(p, id);
                }
            }
            gl.push(PGlyph { id, fb: isfb, x, ch_index: ci });
            let adv = if isfb { sfb.as_ref().map(|s| s.h_advance(id)).unwrap_or(0.0) } else { sf.h_advance(id) };
            x += adv;
            if i + 1 < n {
                x += ls;
            }
            ci += 1;
            prev = Some((id, isfb));
        }
        measured.push((gl, x, *sp));
    }
    // cortar en líneas
    let maxw = spec.max_width.unwrap_or(f32::MAX);
    let mut lines: Vec<Vec<usize>> = vec![vec![]];
    let mut lw = 0.0f32;
    for (i, m) in measured.iter().enumerate() {
        let cur = lines.last_mut().unwrap();
        let add = if cur.is_empty() { m.1 } else { space + m.1 };
        if !cur.is_empty() && (lw + add > maxw || breaks.contains(&i)) {
            lines.push(vec![i]);
            lw = m.1;
        } else {
            cur.push(i);
            lw += add;
        }
    }
    let line_widths: Vec<f32> = lines
        .iter()
        .map(|l| {
            l.iter().map(|&i| measured[i].1).sum::<f32>() + space * (l.len().saturating_sub(1)) as f32
        })
        .collect();
    let width = line_widths.iter().cloned().fold(0.0, f32::max);
    let line_h = spec.size * spec.line_height;
    let cap_h = {
        let g = spec.font.glyph_id('H').with_scale_and_position(scale, point(0.0, 0.0));
        spec.font.outline_glyph(g).map(|o| o.px_bounds().height()).unwrap_or(spec.size * 0.7)
    };
    let mut words = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        let off = match spec.align {
            Align::Left => 0.0,
            Align::Center => (width - line_widths[li]) / 2.0,
            Align::Right => width - line_widths[li],
        };
        let mut x = off;
        for &i in l {
            let (gl, w, sp) = &measured[i];
            let glyphs = gl.iter().map(|g| PGlyph { x: g.x + x, ..g.clone() }).collect();
            words.push(LWord { glyphs, x0: x, x1: x + w, line: li, span: *sp });
            x += w + space;
        }
    }
    Layout {
        words,
        spans,
        width,
        height: line_h * lines.len() as f32,
        line_h,
        cap_h,
        n_chars: ci,
    }
}

fn dilate(mask: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let mut offs: Vec<(i32, i32)> = Vec::new();
    let rings = [1.0f32, 0.7, 0.4];
    for rr in rings {
        let rad = r * rr;
        let n = ((rad * 2.0 * std::f32::consts::PI / 1.5) as usize).clamp(8, 48);
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let o = ((a.cos() * rad).round() as i32, (a.sin() * rad).round() as i32);
            if !offs.contains(&o) {
                offs.push(o);
            }
        }
    }
    offs.push((0, 0));
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for &(dx, dy) in &offs {
            let sy = y as i32 - dy;
            if sy < 0 || sy >= h as i32 {
                continue;
            }
            let srow = &mask[sy as usize * w..(sy as usize + 1) * w];
            let x0 = dx.max(0) as usize;
            let x1 = (w as i32 + dx.min(0)) as usize;
            for x in x0..x1 {
                let v = srow[(x as i32 - dx) as usize];
                if v > row[x] {
                    row[x] = v;
                }
            }
        }
    });
    out
}

fn paint_mask(dst: &mut Frame, mask: &[f32], c: [u8; 4], dx: f32, dy: f32) {
    let p = premul(c);
    let (w, h) = (dst.w, dst.h);
    let (ox, oy) = (dx.round() as i64, dy.round() as i64);
    dst.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let sy = y as i64 - oy;
        if sy < 0 || sy >= h as i64 {
            return;
        }
        for x in 0..w {
            let sx = x as i64 - ox;
            if sx < 0 || sx >= w as i64 {
                continue;
            }
            let m = mask[sy as usize * w + sx as usize];
            if m <= 0.0 {
                continue;
            }
            let a = m.min(1.0);
            let o = &mut row[x * 4..x * 4 + 4];
            let inv = 1.0 - a * p[3] as f32 / 255.0;
            for k in 0..4 {
                o[k] = (p[k] as f32 * a + o[k] as f32 * inv).min(255.0) as u8;
            }
        }
    });
}

pub struct TextSprite {
    pub frame: Frame,
}

/// Renderiza el texto a un sprite. `outline_only` dibuja solo el contorno (para ecos).
pub fn render(spec: &TextSpec, lay: &Layout, vis: Visible, outline_only: bool) -> TextSprite {
    let stroke_w = if outline_only {
        (spec.size * 0.03).max(1.5)
    } else {
        spec.stroke.map(|s| s.1).unwrap_or(0.0)
    };
    let shadow_ext = spec.shadow.map(|s| s.1.abs().max(s.2.abs()) + s.3 * 2.0).unwrap_or(0.0);
    let glow_ext = spec.glow.map(|g| g.1 * 2.0).unwrap_or(0.0);
    let bp = spec.box_pad;
    let pad_x = (stroke_w + shadow_ext.max(glow_ext) + bp[0] + spec.size * 0.25).ceil();
    let pad_y = (stroke_w + shadow_ext.max(glow_ext) + bp[1] + spec.size * 0.25).ceil();
    let inner_w = lay.width.max(spec.min_width - 2.0 * bp[0]);
    let w = (inner_w + 2.0 * pad_x).ceil().max(1.0) as usize;
    let h = (lay.height + 2.0 * pad_y).ceil().max(1.0) as usize;
    let ox = pad_x + (inner_w - lay.width) / 2.0;
    let oy = pad_y;
    let scale = PxScale::from(spec.size);
    let fb = fallback_font();
    let visible_word = |i: usize, wd: &LWord| -> bool {
        match vis {
            Visible::All => true,
            Visible::Words(n) => i < n,
            Visible::Letters(n) => wd.glyphs.first().map(|g| g.ch_index < n).unwrap_or(false),
        }
    };
    let mut mask = vec![0f32; w * h];
    let mut fill = Frame::new(w, h);
    let hl_text = spec.hl_color;
    for (wi, wd) in lay.words.iter().enumerate() {
        if !visible_word(wi, wd) {
            continue;
        }
        let col = if wd.span.is_some() { hl_text.unwrap_or(spec.color) } else { spec.color };
        let pcol = premul(col);
        let baseline = oy + wd.line as f32 * lay.line_h + (lay.line_h + lay.cap_h) / 2.0;
        for g in &wd.glyphs {
            if let Visible::Letters(n) = vis {
                if g.ch_index >= n {
                    continue;
                }
            }
            let font = if g.fb { fb.as_ref().unwrap_or(&spec.font) } else { &spec.font };
            let glyph = g.id.with_scale_and_position(scale, point(ox + g.x, baseline));
            if let Some(og) = font.outline_glyph(glyph) {
                let b = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let x = b.min.x as i64 + gx as i64;
                    let y = b.min.y as i64 + gy as i64;
                    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                        return;
                    }
                    let i = y as usize * w + x as usize;
                    let c = cov.min(1.0);
                    if c > mask[i] {
                        mask[i] = c;
                    }
                    let o = &mut fill.data[i * 4..i * 4 + 4];
                    let inv = 1.0 - c * pcol[3] as f32 / 255.0;
                    for k in 0..4 {
                        o[k] = (pcol[k] as f32 * c + o[k] as f32 * inv).min(255.0) as u8;
                    }
                });
            }
        }
    }
    let mut out = Frame::new(w, h);
    if outline_only {
        let st = dilate(&mask, w, h, stroke_w);
        let ring: Vec<f32> = st.iter().zip(mask.iter()).map(|(s, m)| (s - m).max(0.0)).collect();
        paint_mask(&mut out, &ring, spec.color, 0.0, 0.0);
        return TextSprite { frame: out };
    }
    let stroke_mask = spec.stroke.filter(|s| s.1 > 0.2).map(|s| dilate(&mask, w, h, s.1));
    let union: &Vec<f32> = stroke_mask.as_ref().unwrap_or(&mask);
    // caja general
    if let Some(bc) = spec.box_color {
        let any_visible = lay.words.iter().enumerate().any(|(i, wd)| visible_word(i, wd));
        if any_visible || vis == Visible::All {
            fill_rect(
                &mut out,
                ox + (lay.width - inner_w) / 2.0 - bp[0],
                oy - bp[1],
                ox + (lay.width + inner_w) / 2.0 + bp[0],
                oy + lay.height + bp[1],
                bc,
                1.0,
            );
        }
    }
    // sombra
    if let Some((sc, sx, sy, sb)) = spec.shadow {
        let mut sh = Frame::new(w, h);
        paint_mask(&mut sh, union, sc, sx, sy);
        if sb > 0.5 {
            blur(&mut sh, sb);
        }
        composite(&mut out, &sh, 1.0, Blend::Normal);
    }
    // brillo
    if let Some((gc, gr)) = spec.glow {
        let mut gl = Frame::new(w, h);
        paint_mask(&mut gl, union, gc, 0.0, 0.0);
        blur(&mut gl, gr);
        composite(&mut out, &gl, 1.0, Blend::Screen);
        composite(&mut out, &gl, 0.6, Blend::Screen);
    }
    // cajas de resaltado por grupo y línea
    let mut boxes: HashMap<(usize, usize), (f32, f32)> = HashMap::new();
    for (wi, wd) in lay.words.iter().enumerate() {
        if let Some(sp) = wd.span {
            if !visible_word(wi, wd) {
                continue;
            }
            let e = boxes.entry((sp, wd.line)).or_insert((wd.x0, wd.x1));
            e.0 = e.0.min(wd.x0);
            e.1 = e.1.max(wd.x1);
        }
    }
    let mut keys: Vec<_> = boxes.keys().cloned().collect();
    keys.sort();
    for key in keys {
        let (x0, x1) = boxes[&key];
        let (sp, line) = key;
        let color = lay.spans[sp].color.or_else(|| {
            if spec.hl_boxes.is_empty() {
                None
            } else {
                Some(spec.hl_boxes[sp % spec.hl_boxes.len()])
            }
        });
        if let Some(c) = color {
            let baseline = oy + line as f32 * lay.line_h + (lay.line_h + lay.cap_h) / 2.0;
            let px = spec.size * 0.14;
            let top = baseline - lay.cap_h - spec.size * 0.14;
            let bot = baseline + spec.size * 0.14;
            fill_rect(&mut out, ox + x0 - px, top, ox + x1 + px, bot, c, 1.0);
        }
    }
    if let (Some(sm), Some((sc, _))) = (&stroke_mask, spec.stroke) {
        paint_mask(&mut out, sm, sc, 0.0, 0.0);
    }
    composite(&mut out, &fill, 1.0, Blend::Normal);
    TextSprite { frame: out }
}

/// Texto simple para etiquetas internas (hojas de contacto).
pub fn label(text: &str, size: f32, color: [u8; 4], bg: Option<[u8; 4]>) -> Frame {
    let font = load_font("archivo").expect("fuente interna");
    let spec = TextSpec {
        text: text.to_string(),
        font,
        size,
        color,
        stroke: None,
        shadow: None,
        glow: None,
        box_color: bg,
        box_pad: [size * 0.3, size * 0.15],
        min_width: 0.0,
        hl_color: None,
        hl_boxes: vec![],
        letter_spacing: 0.0,
        line_height: 1.1,
        max_width: None,
        align: Align::Left,
        uppercase: false,
    };
    let lay = layout(&spec);
    render(&spec, &lay, Visible::All, false).frame
}
