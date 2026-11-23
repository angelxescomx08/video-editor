//! Render de frames y del video completo.

use crate::ease::{out_bounce, Ease};
use crate::frame::*;
use crate::fx::{self, Ctx};
use crate::media::{self, Codec, EncodeOpts, Encoder, Fit, VideoStream};
use crate::project::{Anim, Layer};
use crate::text::{self, Align, Layout, TextSpec, Visible};
use crate::timeline::*;
use crate::transitions;
use crate::util::*;
use anyhow::Result;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub struct RenderOpts {
    /// 1.0 = resolución completa; 0.5 = vista previa rápida
    pub scale: f32,
    /// codificación rápida (borrador)
    pub draft: bool,
    pub codec: Option<String>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub quiet: bool,
    /// callback de progreso (frame actual, total)
    pub progress: Option<Box<dyn Fn(u64, u64) + Send>>,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts { scale: 1.0, draft: false, codec: None, start: None, end: None, quiet: false, progress: None }
    }
}

struct TextState {
    spec: TextSpec,
    layout: Layout,
    /// sprites ya inclinados (cursiva) por estado de revelado
    cache: HashMap<Visible, Arc<Frame>>,
    outline: Option<Arc<Frame>>,
    skew: f32,
    n_words: usize,
    word_times: Vec<f64>,
}

#[derive(Default)]
struct LayerState {
    text: Option<TextState>,
    sprite: Option<Arc<Frame>>,
    stream: Option<VideoStream>,
    vsize: (usize, usize),
    failed: bool,
}

pub struct Renderer<'a> {
    tl: &'a Timeline,
    pub w: usize,
    pub h: usize,
    pub k: f32,
    streams: HashMap<usize, VideoStream>,
    stills: HashMap<usize, Arc<Frame>>,
    layers: Vec<LayerState>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct AnimState {
    dx: f32,
    dy: f32,
    sx: f32,
    sy: f32,
    rot: f32,
    opacity: f32,
    blur: f32,
    glitch: f32,
    wipe: Option<f32>,
    letters: Option<f32>,
}

impl Default for AnimState {
    fn default() -> Self {
        AnimState { dx: 0.0, dy: 0.0, sx: 1.0, sy: 1.0, rot: 0.0, opacity: 1.0, blur: 0.0, glitch: 0.0, wipe: None, letters: None }
    }
}

fn even(x: f32) -> usize {
    ((x.round() as usize).max(2)) & !1
}

impl<'a> Renderer<'a> {
    pub fn new(tl: &'a Timeline, scale: f32) -> Renderer<'a> {
        let w = even(tl.w as f32 * scale);
        let h = even(tl.h as f32 * scale);
        let k = w as f32 / tl.w as f32;
        Renderer {
            tl,
            w,
            h,
            k,
            streams: HashMap::new(),
            stills: HashMap::new(),
            layers: (0..tl.layers.len()).map(|_| LayerState::default()).collect(),
            warnings: Vec::new(),
        }
    }

    fn ctx(&self, t: f64, frame: u64) -> Ctx {
        Ctx { w: self.w, h: self.h, k: self.k, fps: self.tl.fps, frame, time: t }
    }

    fn decode_range(c: &TClip) -> (f64, f64) {
        let cands = [c.src_time(-c.pre).0, c.src_time(c.dur + c.post).0, c.in_, c.out];
        let lo = cands.iter().cloned().fold(f64::MAX, f64::min);
        let hi = cands.iter().cloned().fold(f64::MIN, f64::max);
        (lo, hi)
    }

    fn stream_for(&mut self, ci: usize) -> &mut VideoStream {
        let tl = self.tl;
        let (w, h) = (self.w, self.h);
        self.streams.entry(ci).or_insert_with(|| {
            let c = &tl.clips[ci];
            let path = match &c.src {
                ClipSrc::Video { path } => path.clone(),
                _ => String::new(),
            };
            let (lo, hi) = Self::decode_range(c);
            let src_fps = c.src_fps.min(60.0);
            let dec_fps = if c.speed < 1.0 || c.reverse { src_fps } else { src_fps.min(tl.fps * c.speed).max(1.0) };
            VideoStream::new(&path, w, h, dec_fps, c.fit, c.crop, c.reverse, lo, hi + 0.05)
        })
    }

    fn still_for(&mut self, ci: usize) -> Result<Arc<Frame>> {
        if let Some(f) = self.stills.get(&ci) {
            return Ok(f.clone());
        }
        let c = &self.tl.clips[ci];
        let f = match &c.src {
            ClipSrc::Image(p) => {
                let img = media::load_image(p)?;
                let fit = if c.fit == Fit::Cover && false { Fit::Contain } else { c.fit };
                let img = if let Some([cx, cy, cw, ch]) = c.crop {
                    crop_frame(&img, cx, cy, cw, ch)
                } else {
                    img
                };
                media::fit_image(&img, self.w, self.h, fit)
            }
            ClipSrc::Color(col) => Frame::filled(self.w, self.h, *col),
            ClipSrc::Gradient(cols, ang) => gradient(self.w, self.h, cols, *ang),
            ClipSrc::Video { .. } => unreachable!(),
        };
        let f = Arc::new(f);
        self.stills.insert(ci, f.clone());
        Ok(f)
    }

    /// Frame de un clip en el tiempo global t (con sus efectos).
    fn clip_frame(&mut self, ci: usize, t: f64, frame_no: u64) -> Result<Frame> {
        let c = &self.tl.clips[ci];
        let tau = t - c.start;
        let base: Arc<Frame> = match &c.src {
            ClipSrc::Video { .. } => {
                let (s, _) = c.src_time(tau);
                let st = self.stream_for(ci);
                match st.get(s)? {
                    Some(f) => f,
                    None => Arc::new(Frame::new(self.w, self.h)),
                }
            }
            _ => self.still_for(ci)?,
        };
        let mut f = (*base).clone();
        let c = &self.tl.clips[ci];
        if !c.effects.is_empty() {
            let ctx = self.ctx(t, frame_no);
            fx::apply_all(&mut f, &c.effects, tau, &ctx, Edge::Mirror);
        }
        Ok(f)
    }

    fn base_frame(&mut self, t: f64, frame_no: u64) -> Result<Option<Frame>> {
        let Some(i) = self.tl.clip_at(t) else { return Ok(None) };
        let n = self.tl.clips.len();
        let c = &self.tl.clips[i];
        if let Some(tr) = c.trans_in.clone() {
            if tr.duration > 0.0 && t < c.start + tr.duration / 2.0 && i > 0 {
                let p = ((t - (c.start - tr.duration / 2.0)) / tr.duration) as f32;
                let a = self.clip_frame(i - 1, t, frame_no)?;
                let b = self.clip_frame(i, t, frame_no)?;
                let ctx = self.ctx(t, frame_no);
                return Ok(Some(transitions::apply(&tr, a, b, p, &ctx)));
            }
        }
        if i + 1 < n {
            let nx = &self.tl.clips[i + 1];
            if let Some(tr) = nx.trans_in.clone() {
                if tr.duration > 0.0 && t >= nx.start - tr.duration / 2.0 {
                    let p = ((t - (nx.start - tr.duration / 2.0)) / tr.duration) as f32;
                    let a = self.clip_frame(i, t, frame_no)?;
                    let b = self.clip_frame(i + 1, t, frame_no)?;
                    let ctx = self.ctx(t, frame_no);
                    return Ok(Some(transitions::apply(&tr, a, b, p, &ctx)));
                }
            }
        }
        Ok(Some(self.clip_frame(i, t, frame_no)?))
    }

    /// Libera decodificadores que ya no se usarán y precarga los próximos.
    fn housekeeping(&mut self, t: f64) {
        let tl = self.tl;
        self.streams.retain(|&ci, _| {
            let c = &tl.clips[ci];
            t <= c.end() + c.post + 0.05 && t >= c.start - c.pre - 2.0
        });
        // precarga del siguiente clip de video 0.8 s antes
        if let Some(i) = tl.clip_at(t) {
            for j in [i + 1, i + 2] {
                if j < tl.clips.len() {
                    let c = &tl.clips[j];
                    if matches!(c.src, ClipSrc::Video { .. }) && c.start - c.pre - t < 0.8 && !self.streams.contains_key(&j) {
                        let s0 = c.src_time(-c.pre).0;
                        let st = self.stream_for(j);
                        let _ = st.prime(s0);
                    }
                }
            }
        }
        for (li, l) in tl.layers.iter().enumerate() {
            if t > l.end + 0.1 {
                if let Some(ls) = self.layers.get_mut(li) {
                    ls.stream = None;
                    if ls.text.is_some() {
                        ls.text = None;
                    }
                }
            }
        }
    }

    pub fn render_frame(&mut self, t: f64, frame_no: u64) -> Result<Frame> {
        let tl = self.tl;
        let ctx = self.ctx(t, frame_no);
        let prof = std::env::var("MONTAJE_PROFILE").is_ok();
        let t0 = Instant::now();
        let mut canvas = match self.base_frame(t, frame_no)? {
            Some(base) if is_opaque(&base) => base,
            Some(base) => {
                let mut c = Frame::filled(self.w, self.h, tl.background);
                composite(&mut c, &base, 1.0, Blend::Normal);
                c
            }
            None => Frame::filled(self.w, self.h, tl.background),
        };
        let t1 = Instant::now();
        fx::apply_all(&mut canvas, &tl.fx_video, t, &ctx, Edge::Mirror);
        let t2 = Instant::now();
        if prof && frame_no % 30 == 0 {
            eprintln!("[prof] t={t:.2} base={:.1}ms fx_video={:.1}ms", (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3);
        }
        for li in 0..tl.layers.len() {
            let l = &tl.layers[li];
            if t >= l.start && t < l.end {
                let a = Instant::now();
                let r = self.draw_layer(li, &mut canvas, t, frame_no);
                if prof && frame_no % 30 == 0 {
                    eprintln!("[prof]   layer {li} {} {:.1}ms", l.l.kind, a.elapsed().as_secs_f64() * 1e3);
                }
                if let Err(e) = r {
                    let msg = format!("layers[{li}]: {e}");
                    if !self.warnings.contains(&msg) {
                        self.warnings.push(msg);
                    }
                    self.layers[li].failed = true;
                }
            }
        }
        fx::apply_all(&mut canvas, &tl.fx_all, t, &ctx, Edge::Clamp);
        fx::opaque_over(&mut canvas, tl.background);
        self.housekeeping(t);
        Ok(canvas)
    }

    // ------------------------------------------------------------ capas

    fn text_spec(&self, l: &Layer) -> Result<TextSpec> {
        let k = self.k;
        let u = self.tl.unit;
        let font = text::load_font(l.font.as_deref().unwrap_or("anton"))?;
        let size = l.size.unwrap_or(90.0 * u) * k;
        let pw = self.tl.w as f32;
        let full = l.box_full_width.unwrap_or(false);
        let mut hl_boxes: Vec<[u8; 4]> = l
            .highlight_colors
            .as_ref()
            .map(|v| v.iter().filter_map(|c| parse_color(c).ok()).collect())
            .unwrap_or_default();
        if let Some(b) = &l.highlight_box {
            if let Ok(c) = parse_color(b) {
                hl_boxes = vec![c];
            }
        }
        Ok(TextSpec {
            text: l.text.clone().unwrap_or_default(),
            font,
            size,
            color: color_or(&l.color, [255, 255, 255, 255]),
            stroke: l.stroke_color.as_ref().and_then(|c| parse_color(c).ok()).map(|c| (c, l.stroke_width.unwrap_or(4.0 * u) * k)),
            shadow: l.shadow_color.as_ref().and_then(|c| parse_color(c).ok()).map(|c| {
                let o = l.shadow_offset.unwrap_or([0.0, 4.0 * u]);
                (c, o[0] * k, o[1] * k, l.shadow_blur.unwrap_or(6.0 * u) * k)
            }),
            glow: l.glow_color.as_ref().and_then(|c| parse_color(c).ok()).map(|c| (c, l.glow_radius.unwrap_or(20.0 * u) * k)),
            box_color: l.box_color.as_ref().and_then(|c| parse_color(c).ok()),
            box_pad: l.box_padding.map(|p| [p[0] * k, p[1] * k]).unwrap_or([24.0 * u * k, 12.0 * u * k]),
            min_width: if full { pw * k * 1.1 } else { 0.0 },
            hl_color: l.highlight_color.as_ref().and_then(|c| parse_color(c).ok()),
            hl_boxes,
            letter_spacing: l.letter_spacing.unwrap_or(0.0),
            line_height: l.line_height.unwrap_or(1.15),
            max_width: l.max_width.map(|m| m.px(pw) * k),
            align: match l.align.as_deref() {
                Some("left") => Align::Left,
                Some("right") => Align::Right,
                _ => Align::Center,
            },
            uppercase: l.uppercase.unwrap_or(false),
        })
    }

    fn ensure_layer(&mut self, li: usize) -> Result<()> {
        let tl = self.tl;
        let tly = &tl.layers[li];
        let l = &tly.l;
        let k = self.k;
        match l.kind.as_str() {
            "text" => {
                if self.layers[li].text.is_none() {
                    let spec = self.text_spec(l)?;
                    let layout = text::layout(&spec);
                    let n_words = layout.words.len();
                    let life = tly.end - tly.start;
                    let word_times: Vec<f64> = if let Some(wt) = &l.word_times {
                        wt.iter().map(|t| t.0).collect()
                    } else if l.reveal.as_deref() == Some("words") {
                        let rd = l.reveal_duration.unwrap_or((life * 0.75).min(n_words as f64 * 0.32));
                        (0..n_words).map(|i| tly.start + rd * i as f64 / n_words.max(1) as f64).collect()
                    } else {
                        vec![]
                    };
                    let skew = text_skew(l);
                    let outline = if l.echo.is_some() {
                        let outline_only = l.echo.as_ref().and_then(|e| e.outline).unwrap_or(true);
                        let s = text::render(&spec, &layout, Visible::All, outline_only);
                        Some(Arc::new(shear_sprite(&s.frame, skew)))
                    } else {
                        None
                    };
                    self.layers[li].text = Some(TextState { spec, layout, cache: HashMap::new(), outline, n_words, word_times, skew });
                }
            }
            "image" => {
                if self.layers[li].sprite.is_none() {
                    let path = tly.path.clone().unwrap_or_default();
                    let mut img = media::load_image(&path)?;
                    if let Some([cx, cy, cw, ch]) = l.crop {
                        img = crop_frame(&img, cx, cy, cw, ch);
                    }
                    if let Some(key) = &l.chroma_key {
                        let c = parse_color(key).unwrap_or([0, 255, 0, 255]);
                        fx::chroma_key(&mut img, c, l.chroma_similarity.unwrap_or(0.12), l.chroma_smoothness.unwrap_or(0.08));
                    }
                    let (tw, th) = self.target_size(l, img.w as f32, img.h as f32);
                    let mut fitted = if l.width.is_some() && l.height.is_some() {
                        media::fit_image(&img, tw, th, Fit::parse(Some(l.fit.as_deref().unwrap_or("contain"))))
                    } else {
                        media::resize(&img, tw, th)
                    };
                    if let Some(r) = l.radius {
                        round_corners(&mut fitted, r * k);
                    }
                    self.layers[li].sprite = Some(Arc::new(fitted));
                }
            }
            "rect" => {
                if self.layers[li].sprite.is_none() {
                    let pw = tl.w as f32;
                    let ph = tl.h as f32;
                    let rw = l.width.map(|x| x.px(pw)).unwrap_or(pw) * k;
                    let rh = l.height.map(|x| x.px(ph)).unwrap_or(ph * 0.1) * k;
                    let mut f = Frame::filled(rw.max(1.0) as usize, rh.max(1.0) as usize, color_or(&l.color, [255, 255, 255, 255]));
                    if let Some(r) = l.radius {
                        round_corners(&mut f, r * k);
                    }
                    self.layers[li].sprite = Some(Arc::new(f));
                }
            }
            "video" => {
                if self.layers[li].stream.is_none() {
                    let info = tly.info.clone().unwrap();
                    let (tw, th) = self.target_size(l, info.width as f32, info.height as f32);
                    let (tw, th) = ((tw.max(2)) & !1, (th.max(2)) & !1);
                    let fit = if l.width.is_some() && l.height.is_some() {
                        Fit::parse(Some(l.fit.as_deref().unwrap_or("contain")))
                    } else {
                        Fit::Stretch
                    };
                    let a = l.in_.map(|t| t.0).unwrap_or(0.0);
                    let b = l.out.map(|t| t.0).unwrap_or(info.duration);
                    let fps = info.fps.min(60.0).max(1.0);
                    self.layers[li].stream = Some(VideoStream::new(&tly.path.clone().unwrap(), tw, th, fps, fit, l.crop, false, a, b));
                    self.layers[li].vsize = (tw, th);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn target_size(&self, l: &Layer, nw: f32, nh: f32) -> (usize, usize) {
        let k = self.k;
        let pw = self.tl.w as f32;
        let ph = self.tl.h as f32;
        let aspect = nw / nh.max(1.0);
        let (w, h) = match (l.width, l.height) {
            (Some(w), Some(h)) => (w.px(pw), h.px(ph)),
            (Some(w), None) => (w.px(pw), w.px(pw) / aspect),
            (None, Some(h)) => (h.px(ph) * aspect, h.px(ph)),
            (None, None) => {
                // por defecto: tamaño natural limitado al lienzo
                let s = (pw / nw).min(ph / nh).min(1.0);
                (nw * s, nh * s)
            }
        };
        ((w * k).round().max(1.0) as usize, (h * k).round().max(1.0) as usize)
    }

    fn anim_state(&self, l: &Layer, lt: f64, life: f64, seed: u64, frame: u64) -> AnimState {
        let mut st = AnimState::default();
        let (w, h, k) = (self.w as f32, self.h as f32, self.k);
        let apply = |st: &mut AnimState, a: &Anim, q: f32, entering: bool| {
            let i = a.intensity.unwrap_or(1.0);
            let q = q.clamp(0.0, 1.0);
            let e = |def: Ease| a.ease.unwrap_or(def).apply(q);
            match a.kind.as_str() {
                "fade" => st.opacity *= q,
                "pop" => {
                    let s = if entering { Ease::OutBack.apply(q) } else { Ease::OutQuad.apply(q) };
                    st.sx *= s.max(0.0);
                    st.sy *= s.max(0.0);
                    st.opacity *= (q * 4.0).min(1.0);
                }
                "zoom_in" => {
                    let s = lerp(1.0 + 1.2 * i, 1.0, e(Ease::OutExpo));
                    st.sx *= s;
                    st.sy *= s;
                    st.opacity *= q;
                }
                "zoom_out" => {
                    let s = lerp(0.3, 1.0, e(Ease::OutExpo));
                    st.sx *= s;
                    st.sy *= s;
                    st.opacity *= q;
                }
                "slide_up" => {
                    st.dy += (1.0 - e(Ease::OutCubic)) * 0.12 * h * i;
                    st.opacity *= q;
                }
                "slide_down" => {
                    st.dy -= (1.0 - e(Ease::OutCubic)) * 0.12 * h * i;
                    st.opacity *= q;
                }
                "slide_left" => {
                    st.dx += (1.0 - e(Ease::OutExpo)) * 0.35 * w * i;
                    st.opacity *= (q * 2.0).min(1.0);
                }
                "slide_right" => {
                    st.dx -= (1.0 - e(Ease::OutExpo)) * 0.35 * w * i;
                    st.opacity *= (q * 2.0).min(1.0);
                }
                "blur" => {
                    st.blur += (1.0 - e(Ease::OutCubic)) * 28.0 * k * i;
                    let s = lerp(1.12, 1.0, e(Ease::OutCubic));
                    st.sx *= s;
                    st.sy *= s;
                    st.opacity *= q;
                }
                "glitch" => {
                    let g = 1.0 - q;
                    st.glitch = st.glitch.max(g * i);
                    let step = (lt * 30.0) as u64;
                    if rand01(seed ^ 0xABCD, step) < g * 0.45 {
                        st.opacity *= 0.15;
                    }
                    st.dx += rands(seed, step) * g * 0.04 * w * i;
                }
                "wipe" => st.wipe = Some(e(Ease::OutCubic)),
                "spin" => {
                    st.rot += (1.0 - e(Ease::OutCubic)) * -180.0 * i;
                    st.sx *= q;
                    st.sy *= q;
                }
                "bounce" => {
                    st.dy -= (1.0 - out_bounce(q)) * 0.35 * h * i;
                }
                "stretch" => {
                    let qq = e(Ease::OutExpo);
                    st.sx *= lerp(3.0, 1.0, qq);
                    st.sy *= lerp(0.25, 1.0, qq);
                    st.opacity *= q;
                }
                "flicker" => {
                    if rand01(seed, (lt * 30.0) as u64) > q {
                        st.opacity *= 0.0;
                    }
                }
                "shake" => {
                    let a = (1.0 - q) * 30.0 * k * i;
                    st.dx += noise1(seed, lt as f32 * 25.0) * a;
                    st.dy += noise1(seed + 9, lt as f32 * 25.0) * a;
                }
                "typewriter" => st.letters = Some(q),
                _ => {}
            }
            let _ = frame;
        };
        if let Some(a) = &l.animate_in {
            let d = a.duration.unwrap_or(0.3).min(life * 0.6).max(0.01);
            let q = (lt / d) as f32;
            if q < 1.0 || a.kind == "typewriter" {
                apply(&mut st, a, q, true);
            }
        }
        if let Some(a) = &l.animate_out {
            let d = a.duration.unwrap_or(0.25).min(life * 0.4).max(0.01);
            let q = ((life - lt) / d) as f32;
            if q < 1.0 {
                apply(&mut st, a, q, false);
            }
        }
        if let Some(a) = &l.animate {
            let i = a.intensity.unwrap_or(1.0);
            let ltf = lt as f32;
            match a.kind.as_str() {
                "wiggle" => st.rot += (ltf * std::f32::consts::TAU * 1.5).sin() * 4.0 * i,
                "pulse" => {
                    let s = 1.0 + 0.05 * i * (ltf * std::f32::consts::TAU * 2.0).sin();
                    st.sx *= s;
                    st.sy *= s;
                }
                "float" => st.dy += (ltf * std::f32::consts::TAU * 0.5).sin() * 12.0 * k * i,
                "shake" => {
                    st.dx += noise1(seed + 21, ltf * 20.0) * 8.0 * k * i;
                    st.dy += noise1(seed + 22, ltf * 20.0) * 8.0 * k * i;
                    st.rot += noise1(seed + 23, ltf * 12.0) * 1.5 * i;
                }
                "flicker" => st.opacity *= 0.7 + 0.3 * rand01(seed, (ltf * 24.0) as u64),
                "spin" => st.rot += ltf * 90.0 * i,
                "glitch" => {
                    if rand01(seed + 77, (ltf * 10.0) as u64) < 0.15 * i {
                        st.glitch = st.glitch.max(0.6);
                    }
                }
                _ => {}
            }
        }
        st
    }

    fn draw_layer(&mut self, li: usize, canvas: &mut Frame, t: f64, frame_no: u64) -> Result<()> {
        if self.layers[li].failed {
            return Ok(());
        }
        let tl = self.tl;
        let tly = &tl.layers[li];
        let l = &tly.l;
        let lt = t - tly.start;
        let life = tly.end - tly.start;
        let seed = hash64(li as u64 * 31 + 17);
        let ctx = self.ctx(t, frame_no);
        if l.kind == "particles" {
            draw_particles(canvas, l, lt as f32, seed, &ctx, tl.unit);
            return Ok(());
        }
        self.ensure_layer(li)?;
        let st = self.anim_state(l, lt, life, seed, frame_no);
        if st.opacity <= 0.002 || st.sx.abs() < 1e-3 || st.sy.abs() < 1e-3 {
            return Ok(());
        }
        // sprite y origen
        let (sprite, outline): (Arc<Frame>, Option<Arc<Frame>>) = match l.kind.as_str() {
            "text" => {
                let ts = self.layers[li].text.as_mut().unwrap();
                let vis = if let Some(q) = st.letters {
                    Visible::Letters((q * ts.layout.n_chars as f32).ceil() as usize)
                } else if !ts.word_times.is_empty() {
                    let n = ts.word_times.iter().filter(|&&wt| wt <= t + 1e-6).count().max(1);
                    if n >= ts.n_words {
                        Visible::All
                    } else {
                        Visible::Words(n)
                    }
                } else {
                    Visible::All
                };
                let spr = if let Some(s) = ts.cache.get(&vis) {
                    s.clone()
                } else {
                    let s = text::render(&ts.spec, &ts.layout, vis, false);
                    let s = Arc::new(shear_sprite(&s.frame, ts.skew));
                    if ts.cache.len() > 64 {
                        ts.cache.clear();
                    }
                    ts.cache.insert(vis, s.clone());
                    s
                };
                (spr, ts.outline.clone())
            }
            "image" | "rect" => (self.layers[li].sprite.clone().unwrap(), None),
            "video" => {
                let a = l.in_.map(|x| x.0).unwrap_or(0.0);
                let info = tly.info.clone().unwrap();
                let b = l.out.map(|x| x.0).unwrap_or(info.duration);
                let span = (b - a).max(0.01);
                let mut s = lt * l.speed.unwrap_or(1.0);
                if l.loop_.unwrap_or(false) {
                    s %= span;
                }
                let s = (a + s).min(b);
                let (tw, th) = self.layers[li].vsize;
                let fr = self.layers[li].stream.as_mut().unwrap().get(s)?;
                let mut f = fr.map(|x| (*x).clone()).unwrap_or_else(|| Frame::new(tw, th));
                if let Some(key) = &l.chroma_key {
                    let c = parse_color(key).unwrap_or([0, 255, 0, 255]);
                    fx::chroma_key(&mut f, c, l.chroma_similarity.unwrap_or(0.12), l.chroma_smoothness.unwrap_or(0.08));
                }
                if let Some(r) = l.radius {
                    round_corners(&mut f, r * self.k);
                }
                (Arc::new(f), None)
            }
            _ => return Ok(()),
        };
        let sprite = if let Some(wf) = st.wipe { Arc::new(wipe_crop(&sprite, wf)) } else { sprite };
        // transformación
        let (cw, ch) = (self.w as f32, self.h as f32);
        let anchor = l.anchor.unwrap_or([0.5, 0.5]);
        let mut x = l.x.unwrap_or(0.5) * cw;
        let mut y = l.y.unwrap_or(0.5) * ch;
        if let Some(d) = l.drift {
            let q = (lt / life) as f32;
            x += d[0] * cw * q;
            y += d[1] * ch * q;
        }
        let mut s = l.scale.unwrap_or(1.0);
        if let Some(z) = l.zoom_to {
            s *= lerp(1.0, z, (lt / life) as f32);
        }
        // en texto la cursiva ya viene aplicada en el sprite
        let skew = if l.kind == "text" { 0.0 } else { text_skew(l) };
        let rot = l.rotation.unwrap_or(0.0) + st.rot;
        let m = Affine::translate(-anchor[0] * sprite.w as f32, -anchor[1] * sprite.h as f32)
            .then(&Affine::shear_x(skew.to_radians().tan()))
            .then(&Affine::scale(s * st.sx, s * st.sy))
            .then(&Affine::rotate(rot.to_radians()))
            .then(&Affine::translate(x + st.dx, y + st.dy));
        let opacity = l.opacity.unwrap_or(1.0) * st.opacity;
        let blend = Blend::parse(l.blend.as_deref());
        let needs_post = st.blur > 0.3 || st.glitch > 0.01 || !tly.effects.is_empty();
        let draw_echo = |dst: &mut Frame, offx: f32, offy: f32| {
            if let (Some(e), Some(ol)) = (&l.echo, &outline) {
                let n = e.count.unwrap_or(3) as i32;
                let spacing = e.spacing.unwrap_or(1.0) * sprite.h as f32 * 0.55;
                let base_op = e.opacity.unwrap_or(0.45);
                let dir = e.direction.as_deref().unwrap_or("vertical");
                for i in 1..=n {
                    let fade = base_op * (1.0 - (i as f32 - 1.0) / n as f32);
                    for sgn in [-1.0f32, 1.0] {
                        let (ox, oy, sc) = match dir {
                            "horizontal" => (sgn * i as f32 * spacing * 2.5, 0.0, 1.0),
                            "depth" => (0.0, 0.0, 1.0 + sgn.abs() * 0.12 * i as f32),
                            _ => (0.0, sgn * i as f32 * spacing, 1.0),
                        };
                        if dir == "depth" && sgn < 0.0 {
                            continue;
                        }
                        let me = Affine::translate(-anchor[0] * ol.w as f32, -anchor[1] * ol.h as f32)
                            .then(&Affine::shear_x(skew.to_radians().tan()))
                            .then(&Affine::scale(s * st.sx * sc, s * st.sy * sc))
                            .then(&Affine::rotate(rot.to_radians()))
                            .then(&Affine::translate(x + st.dx + ox + offx, y + st.dy + oy + offy));
                        draw_sprite(dst, ol, &me, opacity * fade, Blend::Normal);
                    }
                }
            }
        };
        if needs_post {
            // búfer local del tamaño de la capa (más margen), no del lienzo completo
            let (x0, y0, x1, y1) = if l.echo.is_some() {
                (0i64, 0i64, self.w as i64, self.h as i64)
            } else {
                let cs = [m.apply(0.0, 0.0), m.apply(sprite.w as f32, 0.0), m.apply(0.0, sprite.h as f32), m.apply(sprite.w as f32, sprite.h as f32)];
                let mx = st.blur * 3.0 + 12.0 * self.k + if st.glitch > 0.01 { 0.04 * cw } else { 0.0 };
                let minx = cs.iter().map(|c| c.0).fold(f32::MAX, f32::min) - mx;
                let maxx = cs.iter().map(|c| c.0).fold(f32::MIN, f32::max) + mx;
                let miny = cs.iter().map(|c| c.1).fold(f32::MAX, f32::min) - mx;
                let maxy = cs.iter().map(|c| c.1).fold(f32::MIN, f32::max) + mx;
                (
                    (minx.floor() as i64).max(0),
                    (miny.floor() as i64).max(0),
                    (maxx.ceil() as i64).min(self.w as i64),
                    (maxy.ceil() as i64).min(self.h as i64),
                )
            };
            if x1 <= x0 || y1 <= y0 {
                return Ok(());
            }
            let mut tmp = Frame::new((x1 - x0) as usize, (y1 - y0) as usize);
            let (fx0, fy0) = (x0 as f32, y0 as f32);
            draw_echo(&mut tmp, -fx0, -fy0);
            draw_sprite(&mut tmp, &sprite, &m.then(&Affine::translate(-fx0, -fy0)), 1.0, Blend::Normal);
            if st.blur > 0.3 {
                blur(&mut tmp, st.blur);
            }
            if st.glitch > 0.01 {
                let step = (t * 20.0) as u64;
                fx::glitch(&mut tmp, st.glitch, seed.wrapping_add(step * 131), self.k);
            }
            if !tly.effects.is_empty() {
                let lctx = Ctx { w: tmp.w, h: tmp.h, ..ctx };
                fx::apply_all(&mut tmp, &tly.effects, lt, &lctx, Edge::Transparent);
            }
            draw_sprite(canvas, &tmp, &Affine::translate(fx0, fy0), opacity, blend);
        } else {
            draw_echo(canvas, 0.0, 0.0);
            draw_sprite(canvas, &sprite, &m, opacity, blend);
        }
        Ok(())
    }
}

fn text_skew(l: &Layer) -> f32 {
    l.skew.unwrap_or(if l.italic.unwrap_or(false) { -11.0 } else { 0.0 })
}

fn is_opaque(f: &Frame) -> bool {
    f.data.par_chunks(CHUNK).all(|ch| ch.chunks_exact(4).all(|p| p[3] == 255))
}

pub fn crop_frame(f: &Frame, cx: f32, cy: f32, cw: f32, ch: f32) -> Frame {
    let x0 = ((cx.clamp(0.0, 1.0)) * f.w as f32) as usize;
    let y0 = ((cy.clamp(0.0, 1.0)) * f.h as f32) as usize;
    let w = ((cw.clamp(0.0, 1.0) * f.w as f32) as usize).min(f.w - x0).max(1);
    let h = ((ch.clamp(0.0, 1.0) * f.h as f32) as usize).min(f.h - y0).max(1);
    let mut out = Frame::new(w, h);
    for y in 0..h {
        let s = ((y0 + y) * f.w + x0) * 4;
        out.data[y * w * 4..(y + 1) * w * 4].copy_from_slice(&f.data[s..s + w * 4]);
    }
    out
}

fn wipe_crop(f: &Frame, frac: f32) -> Frame {
    let mut out = f.clone();
    let cut = (frac.clamp(0.0, 1.0) * f.w as f32) as usize;
    let soft = (f.w / 30).max(1);
    for y in 0..f.h {
        for x in cut.saturating_sub(soft)..f.w {
            let i = (y * f.w + x) * 4;
            let m = if x >= cut { 0.0 } else { (cut - x) as f32 / soft as f32 };
            for c in 0..4 {
                out.data[i + c] = (out.data[i + c] as f32 * m) as u8;
            }
        }
    }
    out
}

pub fn round_corners(f: &mut Frame, r: f32) {
    if r <= 0.5 {
        return;
    }
    let (w, h) = (f.w as f32, f.h as f32);
    let r = r.min(w / 2.0).min(h / 2.0);
    let wi = f.w;
    f.data.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        let py = y as f32 + 0.5;
        for x in 0..wi {
            let px = x as f32 + 0.5;
            let qx = (px - r).min(0.0).abs().max((px - (w - r)).max(0.0));
            let qy = (py - r).min(0.0).abs().max((py - (h - r)).max(0.0));
            if qx > 0.0 && qy > 0.0 {
                let d = (qx * qx + qy * qy).sqrt() - r;
                let a = (0.5 - d).clamp(0.0, 1.0);
                for c in 0..4 {
                    row[x * 4 + c] = (row[x * 4 + c] as f32 * a) as u8;
                }
            }
        }
    });
}

pub fn gradient(w: usize, h: usize, cols: &[[u8; 4]], angle_deg: f32) -> Frame {
    let mut f = Frame::new(w, h);
    let a = angle_deg.to_radians();
    let (s, c) = a.sin_cos();
    // ángulo 90 = de arriba a abajo
    let (dx, dy) = (c, s);
    let n = cols.len();
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let ny = y as f32 / h as f32 - 0.5;
        for x in 0..w {
            let nx = x as f32 / w as f32 - 0.5;
            let tt = ((nx * dx + ny * dy) / (dx.abs() * 0.5 + dy.abs() * 0.5).max(1e-3) * 0.5 + 0.5).clamp(0.0, 1.0);
            let pos = tt * (n - 1) as f32;
            let i0 = (pos.floor() as usize).min(n - 1);
            let i1 = (i0 + 1).min(n - 1);
            let fr = pos - i0 as f32;
            let col = [
                lerp(cols[i0][0] as f32, cols[i1][0] as f32, fr) as u8,
                lerp(cols[i0][1] as f32, cols[i1][1] as f32, fr) as u8,
                lerp(cols[i0][2] as f32, cols[i1][2] as f32, fr) as u8,
                255,
            ];
            row[x * 4..x * 4 + 4].copy_from_slice(&col);
        }
    });
    f
}

fn draw_dot(f: &mut Frame, cx: f32, cy: f32, r: f32, c: [u8; 4], op: f32) {
    let r = r.max(0.6);
    let x0 = (cx - r).floor().max(0.0) as usize;
    let x1 = ((cx + r).ceil() as usize).min(f.w);
    let y0 = (cy - r).floor().max(0.0) as usize;
    let y1 = ((cy + r).ceil() as usize).min(f.h);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt() / r;
            if d >= 1.0 {
                continue;
            }
            let a = (1.0 - d).powf(1.6) * op;
            let i = (y * f.w + x) * 4;
            for k in 0..3 {
                let dv = f.data[i + k] as f32;
                let s = c[k] as f32 * a;
                f.data[i + k] = (dv + s - dv * s / 255.0).min(255.0) as u8;
            }
            f.data[i + 3] = f.data[i + 3].max((a * 255.0) as u8);
        }
    }
}

fn draw_particles(f: &mut Frame, l: &Layer, lt: f32, seed: u64, ctx: &Ctx, unit: f32) {
    let kind = l.particle.as_deref().unwrap_or("embers");
    let (w, h) = (ctx.w as f32, ctx.h as f32);
    let k = ctx.k * unit;
    let def_colors: Vec<[u8; 4]> = match kind {
        "embers" => vec![[255, 120, 30, 255], [255, 70, 20, 255], [255, 180, 60, 255]],
        "sparks" => vec![[255, 220, 150, 255], [255, 160, 60, 255]],
        "confetti" => vec![[255, 60, 90, 255], [255, 210, 0, 255], [0, 200, 255, 255], [120, 255, 80, 255], [190, 90, 255, 255]],
        "bokeh" => vec![[120, 140, 255, 255], [255, 120, 200, 255], [255, 220, 160, 255]],
        "rain" => vec![[170, 190, 220, 255]],
        _ => vec![[255, 255, 255, 255]],
    };
    let colors: Vec<[u8; 4]> = l
        .colors
        .as_ref()
        .map(|v| v.iter().filter_map(|c| parse_color(c).ok()).collect())
        .filter(|v: &Vec<[u8; 4]>| !v.is_empty())
        .or_else(|| l.color.as_ref().and_then(|c| parse_color(c).ok()).map(|c| vec![c]))
        .unwrap_or(def_colors);
    let count = l.count.unwrap_or(match kind {
        "stars" => 160,
        "bokeh" => 18,
        "rain" => 220,
        "confetti" => 90,
        _ => 70,
    });
    let size = l.particle_size.unwrap_or(1.0);
    let speed = l.particle_speed.unwrap_or(1.0);
    let op = l.opacity.unwrap_or(1.0);
    let fade_in = (lt / 0.4).min(1.0);
    let seed = l.seed.unwrap_or(seed);
    for i in 0..count as u64 {
        let r0 = rand01(seed, i * 11);
        let r1 = rand01(seed, i * 11 + 1);
        let r2 = rand01(seed, i * 11 + 2);
        let r3 = rand01(seed, i * 11 + 3);
        let col = colors[(rand01(seed, i * 11 + 4) * colors.len() as f32) as usize % colors.len()];
        let wrap = |v: f32| v.rem_euclid(1.1) - 0.05;
        match kind {
            "embers" | "sparks" => {
                let sp = (0.05 + r2 * 0.12) * speed * if kind == "sparks" { 3.0 } else { 1.0 };
                let y = wrap(r1 - sp * lt);
                let x = wrap(r0 + noise1(seed + i, lt * 0.8 + r3 * 10.0) * 0.03 + if kind == "sparks" { (r3 - 0.5) * sp * lt } else { 0.0 });
                let flick = 0.55 + 0.45 * noise1(seed + i * 3, lt * 6.0).abs();
                let rad = (2.0 + r3 * 5.0) * k * size * if kind == "sparks" { 0.6 } else { 1.0 };
                draw_dot(f, x * w, y * h, rad, col, op * flick * fade_in);
                draw_dot(f, x * w, y * h, rad * 3.0, col, op * flick * fade_in * 0.15);
            }
            "snow" | "dust" => {
                let sp = (0.03 + r2 * 0.06) * speed * if kind == "snow" { 1.6 } else { 0.25 };
                let y = wrap(r1 + sp * lt * if kind == "dust" { (r3 - 0.5) * 2.0 } else { 1.0 });
                let x = wrap(r0 + noise1(seed + i, lt * 0.3 + r3 * 10.0) * 0.04);
                let rad = (1.5 + r3 * 3.5) * k * size;
                let a = if kind == "dust" { 0.35 } else { 0.85 };
                draw_dot(f, x * w, y * h, rad, col, op * a * fade_in);
            }
            "stars" => {
                let tw = 0.4 + 0.6 * (lt * (1.0 + r2 * 3.0) + r3 * 10.0).sin().abs();
                let rad = (0.8 + r3.powi(3) * 3.0) * k * size;
                draw_dot(f, r0 * w, r1 * h, rad, col, op * tw * fade_in);
            }
            "bokeh" => {
                let x = wrap(r0 + noise1(seed + i, lt * 0.1) * 0.08);
                let y = wrap(r1 + noise1(seed + i + 99, lt * 0.1) * 0.08);
                let rad = (25.0 + r3 * 70.0) * k * size;
                draw_dot(f, x * w, y * h, rad, col, op * 0.22 * fade_in);
            }
            "confetti" => {
                let sp = (0.08 + r2 * 0.12) * speed;
                let y = wrap(r1 + sp * lt);
                let x = wrap(r0 + (lt * 2.0 + r3 * 6.0).sin() * 0.02);
                let sw = (6.0 + r3 * 6.0) * k * size * (lt * 5.0 + r2 * 7.0).cos().abs().max(0.15);
                let sh = (10.0 + r3 * 6.0) * k * size;
                fill_rect(f, x * w - sw / 2.0, y * h - sh / 2.0, x * w + sw / 2.0, y * h + sh / 2.0, col, op * fade_in);
            }
            "rain" => {
                let sp = (1.2 + r2 * 0.8) * speed;
                let y = wrap(r1 + sp * lt);
                let x = r0;
                let len = (20.0 + r3 * 30.0) * k * size;
                fill_rect(f, x * w, y * h - len, x * w + (1.2 * k).max(1.0), y * h, col, op * 0.35 * fade_in);
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------ render completo

#[derive(serde::Serialize)]
pub struct RenderStats {
    pub output: String,
    pub frames: u64,
    pub duration: f64,
    pub width: usize,
    pub height: usize,
    pub seconds: f64,
    pub fps: f64,
    pub codec: String,
    pub warnings: Vec<String>,
}

pub fn render(tl: &Timeline, out: &Path, o: &RenderOpts) -> Result<RenderStats> {
    let t0 = Instant::now();
    let start = o.start.unwrap_or(0.0).max(0.0);
    let end = o.end.unwrap_or(tl.duration).min(tl.duration);
    if end <= start {
        anyhow::bail!("rango vacío: {start:.2}–{end:.2}");
    }
    let mut r = Renderer::new(tl, o.scale);
    // audio
    let mix = crate::mix::build_mix(tl)?;
    let wav = std::env::temp_dir().join(format!("montaje-{}-{}.wav", std::process::id(), t0.elapsed().as_nanos()));
    let has_audio = mix.is_some();
    if let Some(m) = &mix {
        let s0 = (start * media::SR as f64) as usize * 2;
        let s1 = ((end * media::SR as f64) as usize * 2).min(m.len());
        media::write_wav(&wav, &m[s0.min(s1)..s1], 2, media::SR)?;
    }
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let codec = media::pick_codec(o.codec.as_deref());
    let enc = Encoder::start(EncodeOpts {
        w: r.w,
        h: r.h,
        fps: tl.fps,
        audio: if has_audio { Some(&wav) } else { None },
        out,
        quality: if o.draft { tl.quality.max(26) } else { tl.quality },
        fast: o.draft,
        codec,
    })?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Frame>(4);
    let writer = std::thread::spawn(move || -> Result<()> {
        let mut enc = enc;
        for f in rx {
            enc.write(&f)?;
        }
        enc.finish()
    });
    let n = ((end - start) * tl.fps).round().max(1.0) as u64;
    let mut last = Instant::now();
    let mut err = None;
    let prof = std::env::var("MONTAJE_PROFILE").is_ok();
    let (mut t_render, mut t_send) = (0f64, 0f64);
    let t_loop = Instant::now();
    for i in 0..n {
        let t = start + i as f64 / tl.fps;
        let a = Instant::now();
        let f = match r.render_frame(t, i) {
            Ok(f) => f,
            Err(e) => {
                err = Some(e);
                break;
            }
        };
        let b = Instant::now();
        if tx.send(f).is_err() {
            break;
        }
        t_render += (b - a).as_secs_f64();
        t_send += b.elapsed().as_secs_f64();
        if prof && i + 1 == n {
            eprintln!(
                "[prof] setup={:.2}s render={:.1}ms/f send(wait encoder)={:.1}ms/f",
                (t_loop - t0).as_secs_f64(),
                t_render * 1e3 / n as f64,
                t_send * 1e3 / n as f64
            );
        }
        if last.elapsed().as_secs_f32() > 1.0 {
            last = Instant::now();
            if let Some(p) = &o.progress {
                p(i + 1, n);
            }
            if o.quiet {
                continue;
            }
            let el = t0.elapsed().as_secs_f64();
            let fps = (i + 1) as f64 / el;
            eprintln!(
                "  frame {}/{} ({:.0}%) · {:.1} fps · faltan ~{:.0} s",
                i + 1,
                n,
                (i + 1) as f64 / n as f64 * 100.0,
                fps,
                (n - i - 1) as f64 / fps.max(0.01)
            );
        }
    }
    drop(tx);
    let wres = writer.join().map_err(|_| anyhow::anyhow!("hilo de escritura falló"))?;
    let _ = std::fs::remove_file(&wav);
    if let Some(e) = err {
        return Err(e);
    }
    wres?;
    let secs = t0.elapsed().as_secs_f64();
    let mut warnings = tl.warnings.clone();
    warnings.extend(r.warnings.clone());
    Ok(RenderStats {
        output: out.to_string_lossy().to_string(),
        frames: n,
        duration: end - start,
        width: r.w,
        height: r.h,
        seconds: (secs * 100.0).round() / 100.0,
        fps: ((n as f64 / secs) * 10.0).round() / 10.0,
        codec: match codec {
            Codec::Nvenc => "h264_nvenc",
            Codec::Qsv => "h264_qsv",
            Codec::Amf => "h264_amf",
            Codec::X264 => "libx264",
        }
        .into(),
        warnings,
    })
}

/// Renderiza frames sueltos (para vista previa).
pub fn render_stills(tl: &Timeline, times: &[f64], scale: f32) -> Result<(Vec<(f64, Frame)>, Vec<String>)> {
    let mut r = Renderer::new(tl, scale);
    let mut idx: Vec<usize> = (0..times.len()).collect();
    idx.sort_by(|a, b| times[*a].partial_cmp(&times[*b]).unwrap());
    let mut out: Vec<Option<(f64, Frame)>> = (0..times.len()).map(|_| None).collect();
    for i in idx {
        let t = times[i].clamp(0.0, (tl.duration - 1e-3).max(0.0));
        let f = r.render_frame(t, (t * tl.fps) as u64)?;
        out[i] = Some((t, f));
    }
    let mut w = tl.warnings.clone();
    w.extend(r.warnings);
    Ok((out.into_iter().flatten().collect(), w))
}
