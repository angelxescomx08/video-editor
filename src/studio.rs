//! Editor visual (`montaje studio proyecto.json`): vista previa con audio, forma de onda, bucle
//! y ajuste de cada palabra por fotogramas. Las palabras se editan en la transcripción (tiempo de
//! fuente), así que el subtítulo y todo lo anclado con "@palabra" se mueven juntos.

use crate::anchors::{self, OutWord};
use crate::api;
use crate::frame::Frame;
use crate::render::Renderer;
use crate::timeline::Timeline;
use crate::transcribe::{self, Transcript};
use anyhow::{anyhow, bail, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui::{self, Align2, Color32, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const SR: f64 = 48_000.0;

// ------------------------------------------------------------ reproducción de audio

struct Player {
    buf: Mutex<Arc<Vec<f32>>>,
    /// posición en frames de 48 kHz (f64 en bits)
    pos: AtomicU64,
    playing: AtomicBool,
    looping: AtomicBool,
    la: AtomicU64,
    lb: AtomicU64,
}

impl Player {
    fn time(&self) -> f64 {
        f64::from_bits(self.pos.load(Relaxed)) / SR
    }
    fn seek(&self, t: f64) {
        self.pos.store((t.max(0.0) * SR).to_bits(), Relaxed);
    }
    fn set_loop(&self, a: f64, b: f64) {
        self.la.store(a.to_bits(), Relaxed);
        self.lb.store(b.to_bits(), Relaxed);
    }
}

fn start_audio(p: Arc<Player>) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or_else(|| anyhow!("no hay dispositivo de salida de audio"))?;
    let sup = dev.default_output_config()?;
    let cfg = sup.config();
    match sup.sample_format() {
        cpal::SampleFormat::F32 => build_stream::<f32>(&dev, cfg, p),
        cpal::SampleFormat::I16 => build_stream::<i16>(&dev, cfg, p),
        cpal::SampleFormat::U16 => build_stream::<u16>(&dev, cfg, p),
        other => bail!("formato de audio no soportado: {other:?}"),
    }
}

fn build_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(dev: &cpal::Device, cfg: cpal::StreamConfig, p: Arc<Player>) -> Result<cpal::Stream> {
    let ch = cfg.channels as usize;
    let step = SR / cfg.sample_rate as f64;
    let stream = dev.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let silence = T::from_sample(0.0f32);
            if !p.playing.load(Relaxed) {
                for s in data.iter_mut() {
                    *s = silence;
                }
                return;
            }
            let buf = p.buf.lock().map(|b| b.clone()).unwrap_or_default();
            let n = buf.len() / 2;
            let mut pos = f64::from_bits(p.pos.load(Relaxed));
            let lp = p.looping.load(Relaxed);
            let la = f64::from_bits(p.la.load(Relaxed)) * SR;
            let lb = f64::from_bits(p.lb.load(Relaxed)) * SR;
            for fr in data.chunks_mut(ch.max(1)) {
                if lp && lb > la + 100.0 && pos >= lb {
                    pos = la;
                }
                let i = pos as usize;
                let (l, r) = if i + 1 < n {
                    let f = (pos - i as f64) as f32;
                    (buf[i * 2] + (buf[i * 2 + 2] - buf[i * 2]) * f, buf[i * 2 + 1] + (buf[i * 2 + 3] - buf[i * 2 + 1]) * f)
                } else {
                    (0.0, 0.0)
                };
                if ch == 1 {
                    fr[0] = T::from_sample((l + r) * 0.5);
                } else {
                    fr[0] = T::from_sample(l);
                    fr[1] = T::from_sample(r);
                    for s in fr.iter_mut().skip(2) {
                        *s = silence;
                    }
                }
                pos += step;
            }
            if !lp && pos as usize >= n {
                p.playing.store(false, Relaxed);
            }
            p.pos.store(pos.to_bits(), Relaxed);
        },
        |e| eprintln!("audio: {e}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

// ------------------------------------------------------------ render en segundo plano

enum RCmd {
    Timeline(Arc<Timeline>),
    Seek(f64),
}

fn render_thread(rx: mpsc::Receiver<RCmd>, out: Arc<Mutex<Option<(f64, Frame)>>>, warn: Arc<Mutex<Vec<String>>>, player: Arc<Player>, ctx: egui::Context, width: usize) {
    let mut current: Option<Arc<Timeline>> = None;
    let mut target = 0.0f64;
    let mut pending: Vec<RCmd> = Vec::new();
    'outer: loop {
        let tl = match current.clone() {
            Some(t) => t,
            None => match rx.recv() {
                Ok(RCmd::Timeline(t)) => {
                    current = Some(t);
                    continue;
                }
                Ok(RCmd::Seek(t)) => {
                    target = t;
                    continue;
                }
                Err(_) => return,
            },
        };
        let scale = (width as f32 / tl.w as f32).clamp(0.15, 1.0);
        let mut r = Renderer::new(&tl, scale);
        warn.lock().unwrap().clear();
        let mut last: Option<i64> = None;
        let mut dirty = true;
        loop {
            while let Ok(c) = rx.try_recv() {
                pending.push(c);
            }
            for c in pending.drain(..) {
                match c {
                    RCmd::Timeline(t) => {
                        current = Some(t);
                        continue 'outer;
                    }
                    RCmd::Seek(t) => {
                        target = t;
                        dirty = true;
                    }
                }
            }
            let playing = player.playing.load(Relaxed);
            let t = if playing { player.time() } else { target };
            // reproduciendo: el fotograma en curso; en pausa: el primero en el que t ya es visible
            let fi = if playing { (t * tl.fps).floor() as i64 } else { frame_at(t, tl.fps) };
            if dirty || (playing && Some(fi) != last) {
                let tt = (fi.max(0) as f64 / tl.fps).min((tl.duration - 1e-3).max(0.0));
                match r.render_frame(tt, fi.max(0) as u64) {
                    Ok(f) => {
                        *out.lock().unwrap() = Some((tt, f));
                        // capas que fallan al dibujarse (imagen que no existe, etc.)
                        let mut w = warn.lock().unwrap();
                        if w.len() != r.warnings.len() {
                            *w = r.warnings.clone();
                        }
                        drop(w);
                        ctx.request_repaint();
                    }
                    Err(e) => eprintln!("vista previa: {e}"),
                }
                last = Some(fi);
                dirty = false;
            } else if playing {
                std::thread::sleep(Duration::from_millis(3));
            } else {
                match rx.recv_timeout(Duration::from_millis(120)) {
                    Ok(c) => pending.push(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => return,
                }
            }
        }
    }
}

// ------------------------------------------------------------ aplicación

#[derive(Clone, Copy, PartialEq, Debug)]
struct WKey {
    tr: usize,
    seg: usize,
    idx: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Start,
    End,
    Move,
}

struct Drag {
    kind: DragKind,
    key: WKey,
    grab_t: f64,
    orig: (f64, f64),
    cur: (f64, f64),
}

enum Job {
    Aligned(usize, Transcript, String),
    Mix(Vec<f32>),
    Rendered(String),
    Failed(String),
}

struct App {
    project_path: PathBuf,
    stamp: Vec<Option<SystemTime>>,
    tl: Arc<Timeline>,
    trs: Vec<(String, Transcript)>,
    dirty: Vec<bool>,
    words: Vec<OutWord>,
    selected: Option<WKey>,
    player: Arc<Player>,
    _stream: Option<cpal::Stream>,
    mix: Arc<Vec<f32>>,
    peaks: Vec<(f32, f32, f32)>,
    rtx: mpsc::Sender<RCmd>,
    slot: Arc<Mutex<Option<(f64, Frame)>>>,
    render_warnings: Arc<Mutex<Vec<String>>>,
    tex: Option<egui::TextureHandle>,
    view_start: f64,
    view_len: f64,
    loop_a: f64,
    loop_b: f64,
    status: String,
    seg_text: String,
    seg_text_key: Option<(usize, usize)>,
    drag: Option<Drag>,
    jobs_tx: mpsc::Sender<Job>,
    jobs_rx: mpsc::Receiver<Job>,
    busy: Option<String>,
    mix_due: Option<Instant>,
    mix_running: bool,
    model: String,
    last_poll: Instant,
}

const PEAK_HZ: f64 = 200.0;

/// (mínimo, máximo, RMS) por cubeta.
fn peaks_of(mix: &[f32]) -> Vec<(f32, f32, f32)> {
    let bucket = (SR / PEAK_HZ) as usize;
    let n = mix.len() / 2;
    (0..n.div_ceil(bucket))
        .map(|b| {
            let mut lo = 0f32;
            let mut hi = 0f32;
            let mut sq = 0f32;
            let end = ((b + 1) * bucket).min(n);
            for i in b * bucket..end {
                let v = (mix[i * 2] + mix[i * 2 + 1]) * 0.5;
                lo = lo.min(v);
                hi = hi.max(v);
                sq += v * v;
            }
            (lo, hi, (sq / (end - b * bucket).max(1) as f32).sqrt())
        })
        .collect()
}

/// Primer fotograma cuyo tiempo es >= t. Es el criterio del render (`t >= inicio`), así que
/// al saltar al inicio de una palabra se ve el primer fotograma en el que aparece.
fn frame_at(t: f64, fps: f64) -> i64 {
    ((t * fps - 1e-6).ceil() as i64).max(0)
}

fn fmt_tc(t: f64, fps: f64) -> String {
    let f = frame_at(t, fps);
    let secs = t.max(0.0);
    format!("{}:{:05.2}  ·  f {}", (secs / 60.0).floor() as i64, secs % 60.0, f)
}

fn score_color(s: Option<f32>, locked: bool) -> Color32 {
    if locked {
        return Color32::from_rgb(70, 110, 200);
    }
    match s {
        Some(x) if x < 0.4 => Color32::from_rgb(190, 60, 50),
        Some(x) if x < 0.65 => Color32::from_rgb(200, 140, 40),
        _ => Color32::from_rgb(55, 140, 85),
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, project: PathBuf, model: String) -> Result<App> {
        let (tl, trs) = Self::load(&project)?;
        let tl = Arc::new(tl);
        let mix = crate::mix::build_mix(&tl)?.unwrap_or_else(|| vec![0.0; (tl.duration * SR) as usize * 2]);
        let player = Arc::new(Player {
            buf: Mutex::new(Arc::new(mix.clone())),
            pos: AtomicU64::new(0f64.to_bits()),
            playing: AtomicBool::new(false),
            looping: AtomicBool::new(false),
            la: AtomicU64::new(0f64.to_bits()),
            lb: AtomicU64::new(0f64.to_bits()),
        });
        let (stream, status) = match start_audio(player.clone()) {
            Ok(s) => (Some(s), String::from("Listo. Espacio: reproducir · flechas izq./der.: mover inicio 1 fotograma · Mayús: fin · Ctrl: palabra entera · L: bucle · Ctrl+S: guardar")),
            Err(e) => (None, format!("Sin audio: {e}")),
        };
        let (rtx, rrx) = mpsc::channel();
        let slot = Arc::new(Mutex::new(None));
        let render_warnings = Arc::new(Mutex::new(Vec::new()));
        {
            let slot = slot.clone();
            let warn = render_warnings.clone();
            let player = player.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || render_thread(rrx, slot, warn, player, ctx, 960));
        }
        let _ = rtx.send(RCmd::Timeline(tl.clone()));
        let _ = rtx.send(RCmd::Seek(0.0));
        let words = anchors::output_words(&tl.clips, &trs);
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let n = trs.len();
        let peaks = peaks_of(&mix);
        let dur = tl.duration;
        let mut app = App {
            stamp: Vec::new(),
            project_path: project,
            tl,
            trs,
            dirty: vec![false; n],
            words,
            selected: None,
            player,
            _stream: stream,
            mix: Arc::new(mix),
            peaks,
            rtx,
            slot,
            render_warnings,
            tex: None,
            view_start: 0.0,
            view_len: dur.clamp(2.0, 20.0),
            loop_a: 0.0,
            loop_b: 0.0,
            status,
            seg_text: String::new(),
            seg_text_key: None,
            drag: None,
            jobs_tx,
            jobs_rx,
            busy: None,
            mix_due: None,
            mix_running: false,
            model,
            last_poll: Instant::now(),
        };
        app.stamp = app.disk_stamp();
        Ok(app)
    }

    fn load(project: &Path) -> Result<(Timeline, Vec<(String, Transcript)>)> {
        let l = api::load_project_file(&project.to_string_lossy())?;
        let tl = api::prepare(&l)?;
        let v: serde_json::Value = serde_json::from_str(std::fs::read_to_string(project)?.trim_start_matches('\u{feff}'))?;
        let mut trs = anchors::project_transcripts(&v, &l.base)?;
        // también las transcripciones usadas solo en capas captions
        for ly in &l.project.layers {
            if let Some(t) = &ly.transcript {
                if l.project.transcripts.contains_key(t) {
                    continue;
                }
                let p = if Path::new(t).is_absolute() { PathBuf::from(t) } else { l.base.join(t) };
                let ps = p.to_string_lossy().to_string();
                if !trs.iter().any(|(x, _)| *x == ps) {
                    if let Ok(tr) = transcribe::load_transcript(&p) {
                        trs.push((ps, tr));
                    }
                }
            }
        }
        Ok((tl, trs))
    }

    fn word_mut(&mut self, k: WKey) -> Option<&mut transcribe::Word> {
        let seg = self.trs.get_mut(k.tr)?.1.segments.get_mut(k.seg)?;
        if seg.words.is_empty() {
            seg.words = transcribe::proportional_words(seg);
        }
        seg.words.get_mut(k.idx)
    }

    /// Fechas de modificación del proyecto y de sus transcripciones.
    fn disk_stamp(&self) -> Vec<Option<SystemTime>> {
        let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        std::iter::once(mtime(&self.project_path)).chain(self.trs.iter().map(|(p, _)| mtime(Path::new(p)))).collect()
    }

    fn out_word(&self, k: WKey) -> Option<&OutWord> {
        self.words.iter().find(|w| w.tr == k.tr && w.seg == k.seg && w.idx == k.idx)
    }

    /// Fija el tiempo de salida de una palabra (lo convierte a tiempo de fuente con su clip).
    fn set_word_out(&mut self, k: WKey, start: f64, end: f64) {
        let Some(ow) = self.out_word(k).cloned() else { return };
        let c = self.tl.clips[ow.clip].clone();
        let to_src = |t: f64| c.in_ + (t - c.start) * c.speed;
        let (s, e) = (to_src(start), to_src(end.max(start + 0.02)));
        // milisegundos hacia abajo: un tiempo encajado en un fotograma no debe pasar al siguiente
        let ms = |x: f64| (x * 1000.0 + 1e-6).floor() / 1000.0;
        if let Some(w) = self.word_mut(k) {
            w.start = ms(s);
            w.end = ms(e);
            w.locked = true;
        }
        // mantener el segmento coherente
        if let Some(seg) = self.trs.get_mut(k.tr).and_then(|t| t.1.segments.get_mut(k.seg)) {
            if let (Some(f), Some(l)) = (seg.words.first(), seg.words.last()) {
                seg.start = f.start.min(seg.start);
                seg.end = l.end.max(seg.end);
            }
        }
        self.dirty[k.tr] = true;
        self.refresh();
    }

    /// Recalcula la línea de tiempo con las transcripciones editadas (en memoria).
    fn refresh(&mut self) {
        for (path, t) in &self.trs {
            transcribe::set_override(Path::new(path), t.clone());
        }
        match Self::load(&self.project_path) {
            Ok((tl, _)) => {
                self.tl = Arc::new(tl);
                self.words = anchors::output_words(&self.tl.clips, &self.trs);
                let _ = self.rtx.send(RCmd::Timeline(self.tl.clone()));
                let _ = self.rtx.send(RCmd::Seek(self.player.time()));
                self.mix_due = Some(Instant::now() + Duration::from_millis(500));
            }
            Err(e) => self.status = format!("Error: {e:#}"),
        }
    }

    fn reload_from_disk(&mut self) {
        match Self::load(&self.project_path) {
            Ok((tl, trs)) => {
                self.tl = Arc::new(tl);
                self.dirty = vec![false; trs.len()];
                self.trs = trs;
                for (path, t) in &self.trs {
                    transcribe::set_override(Path::new(path), t.clone());
                }
                self.words = anchors::output_words(&self.tl.clips, &self.trs);
                let _ = self.rtx.send(RCmd::Timeline(self.tl.clone()));
                let _ = self.rtx.send(RCmd::Seek(self.player.time()));
                self.mix_due = Some(Instant::now());
                self.stamp = self.disk_stamp();
                self.status = "Proyecto recargado desde el disco".into();
            }
            Err(e) => self.status = format!("Error al recargar: {e:#}"),
        }
    }

    fn save(&mut self) {
        let mut n = 0;
        for (i, (path, t)) in self.trs.iter().enumerate() {
            if !self.dirty[i] {
                continue;
            }
            let res = serde_json::to_string_pretty(t)
                .map_err(anyhow::Error::from)
                .and_then(|s| std::fs::write(path, s).map_err(anyhow::Error::from))
                .and_then(|_| std::fs::write(Path::new(path).with_extension("").with_extension("srt"), transcribe::to_srt(t)).map_err(anyhow::Error::from));
            match res {
                Ok(_) => n += 1,
                Err(e) => {
                    self.status = format!("No se pudo guardar {path}: {e}");
                    return;
                }
            }
        }
        self.dirty.iter_mut().for_each(|d| *d = false);
        // lo escrito por el propio estudio no cuenta como cambio externo
        self.stamp = self.disk_stamp();
        self.status = if n > 0 { format!("Guardado ({n} transcripción/es)") } else { "No había cambios".into() };
    }

    fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, self.tl.duration);
        self.player.seek(t);
        let _ = self.rtx.send(RCmd::Seek(t));
    }

    fn toggle_play(&mut self) {
        let p = !self.player.playing.load(Relaxed);
        if p && self.player.time() >= self.tl.duration - 0.05 {
            self.seek(0.0);
        }
        self.player.playing.store(p, Relaxed);
        if !p {
            let _ = self.rtx.send(RCmd::Seek(self.player.time()));
        }
    }

    fn loop_word(&mut self) {
        if let Some(w) = self.selected.and_then(|k| self.out_word(k)).cloned() {
            self.loop_a = (w.start - 0.6).max(0.0);
            self.loop_b = (w.end + 0.6).min(self.tl.duration);
            self.player.set_loop(self.loop_a, self.loop_b);
            self.player.looping.store(true, Relaxed);
            self.seek(self.loop_a);
            self.player.playing.store(true, Relaxed);
        }
    }

    fn select(&mut self, k: Option<WKey>) {
        self.selected = k;
        if let Some(k) = k {
            if self.seg_text_key != Some((k.tr, k.seg)) {
                self.seg_text = self.trs[k.tr].1.segments[k.seg].text.clone();
                self.seg_text_key = Some((k.tr, k.seg));
            }
        }
    }

    fn select_rel(&mut self, d: i64) {
        let cur = self.selected.and_then(|k| self.words.iter().position(|w| w.tr == k.tr && w.seg == k.seg && w.idx == k.idx));
        let t = self.player.time();
        let i = match cur {
            Some(i) => (i as i64 + d).clamp(0, self.words.len() as i64 - 1) as usize,
            None => self.words.iter().position(|w| w.start >= t).unwrap_or(0),
        };
        if let Some(w) = self.words.get(i).cloned() {
            self.select(Some(WKey { tr: w.tr, seg: w.seg, idx: w.idx }));
            self.seek(w.start);
            if w.start < self.view_start || w.end > self.view_start + self.view_len {
                self.view_start = (w.start - self.view_len * 0.3).max(0.0);
            }
        }
    }

    fn nudge(&mut self, which: DragKind, frames: i64) {
        let Some(k) = self.selected else { return };
        let Some(w) = self.out_word(k).cloned() else { return };
        // se mueve por fotogramas enteros: el primer toque encaja la palabra en la rejilla
        let fps = self.tl.fps;
        let (fs, fe) = (frame_at(w.start, fps), frame_at(w.end, fps));
        let at = |f: i64| f.max(0) as f64 / fps;
        let (s, e) = match which {
            DragKind::Start => (at((fs + frames).min(fe - 1)), w.end),
            DragKind::End => (w.start, at((fe + frames).max(fs + 1))),
            DragKind::Move => (at(fs + frames), at(fe + frames)),
        };
        self.set_word_out(k, s, e);
        if which != DragKind::End {
            self.seek(s);
        }
    }

    fn realign(&mut self, only_seg: bool) {
        let Some(k) = self.selected.or_else(|| self.words.first().map(|w| WKey { tr: w.tr, seg: w.seg, idx: w.idx })) else {
            self.status = "Selecciona una palabra del segmento a realinear".into();
            return;
        };
        if self.busy.is_some() {
            return;
        }
        // aplicar el texto editado
        if only_seg {
            if let Some(seg) = self.trs[k.tr].1.segments.get_mut(k.seg) {
                let new_text = self.seg_text.split_whitespace().collect::<Vec<_>>().join(" ");
                if new_text != seg.text {
                    seg.text = new_text;
                    seg.words.clear();
                }
            }
        }
        let mut t = self.trs[k.tr].1.clone();
        let tx = self.jobs_tx.clone();
        let model = self.model.clone();
        let seg = k.seg;
        let tr = k.tr;
        self.busy = Some(if only_seg { "Realineando segmento…".into() } else { "Realineando todo…".into() });
        std::thread::spawn(move || {
            let log = |m: &str| eprintln!("{m}");
            let o = transcribe::Opts { model: &model, lang: None, log: &log };
            let src = t.source.clone();
            let only = [seg];
            let r = transcribe::align(&src, &mut t, if only_seg { Some(&only[..]) } else { None }, &o);
            let _ = tx.send(match r {
                Ok(_) => Job::Aligned(tr, t, if only_seg { format!("Segmento {seg} realineado") } else { "Transcripción realineada".into() }),
                Err(e) => Job::Failed(format!("{e:#}")),
            });
        });
    }

    fn render_draft(&mut self) {
        if self.busy.is_some() {
            return;
        }
        self.save();
        let path = self.project_path.to_string_lossy().to_string();
        let tx = self.jobs_tx.clone();
        self.busy = Some("Renderizando borrador…".into());
        std::thread::spawn(move || {
            let r = api::load_project_file(&path).and_then(|l| {
                api::render(&l, api::RenderArgs { output: None, draft: true, scale: None, from: None, to: None, codec: None, quiet: true, progress: None })
            });
            let _ = tx.send(match r {
                Ok(v) => Job::Rendered(v["output"].as_str().unwrap_or("").to_string()),
                Err(e) => Job::Failed(format!("{e:#}")),
            });
        });
    }

    fn poll_jobs(&mut self) {
        while let Ok(j) = self.jobs_rx.try_recv() {
            match j {
                Job::Aligned(tr, t, msg) => {
                    self.trs[tr].1 = t;
                    self.dirty[tr] = true;
                    self.busy = None;
                    self.status = msg;
                    self.refresh();
                }
                Job::Mix(m) => {
                    self.peaks = peaks_of(&m);
                    let m = Arc::new(m);
                    *self.player.buf.lock().unwrap() = m.clone();
                    self.mix = m;
                    self.mix_running = false;
                }
                Job::Rendered(p) => {
                    self.busy = None;
                    self.status = format!("Borrador listo: {p}");
                }
                Job::Failed(e) => {
                    self.busy = None;
                    self.mix_running = false;
                    self.status = format!("Error: {e}");
                }
            }
        }
        if let Some(due) = self.mix_due {
            if Instant::now() >= due && !self.mix_running {
                self.mix_due = None;
                self.mix_running = true;
                let tl = self.tl.clone();
                let tx = self.jobs_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(match crate::mix::build_mix(&tl) {
                        Ok(m) => Job::Mix(m.unwrap_or_else(|| vec![0.0; (tl.duration * SR) as usize * 2])),
                        Err(e) => Job::Failed(format!("{e:#}")),
                    });
                });
            }
        }
        // recargar si la IA (u otro programa) cambió el proyecto o una transcripción en disco
        if self.last_poll.elapsed() > Duration::from_secs(1) {
            self.last_poll = Instant::now();
            let s = self.disk_stamp();
            if s != self.stamp {
                self.stamp = s;
                if !self.dirty.iter().any(|d| *d) {
                    self.reload_from_disk();
                } else {
                    self.status = "El proyecto cambió en disco; guarda y pulsa Recargar".into();
                }
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (space, left, right, tab, l, s, home, m) = ctx.input(|i| {
            (
                i.key_pressed(Key::Space),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Tab),
                i.key_pressed(Key::L),
                i.key_pressed(Key::S),
                i.key_pressed(Key::Home),
                i.modifiers,
            )
        });
        if space {
            self.toggle_play();
        }
        if s && m.ctrl {
            self.save();
        }
        if home {
            self.seek(0.0);
        }
        if l {
            self.loop_word();
        }
        if tab {
            self.select_rel(if m.shift { -1 } else { 1 });
        }
        let d = if left { -1 } else if right { 1 } else { 0 };
        if d != 0 {
            if self.selected.is_some() {
                let kind = if m.ctrl {
                    DragKind::Move
                } else if m.shift {
                    DragKind::End
                } else {
                    DragKind::Start
                };
                self.nudge(kind, d);
            } else {
                let t = self.player.time() + d as f64 / self.tl.fps;
                self.seek(t);
            }
        }
    }

    // ---------------- dibujo

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.player.playing.load(Relaxed);
            if ui.button(if playing { "⏸ Pausa" } else { "▶ Reproducir" }).clicked() {
                self.toggle_play();
            }
            if ui.button("⏮").on_hover_text("Inicio").clicked() {
                self.seek(0.0);
            }
            let t = self.player.time();
            ui.monospace(fmt_tc(t, self.tl.fps));
            ui.separator();
            let mut lp = self.player.looping.load(Relaxed);
            if ui.checkbox(&mut lp, "Bucle").changed() {
                self.player.looping.store(lp, Relaxed);
            }
            if ui.button("A").on_hover_text("Inicio del bucle en el cursor").clicked() {
                self.loop_a = t;
                self.player.set_loop(self.loop_a, self.loop_b);
            }
            if ui.button("B").on_hover_text("Fin del bucle en el cursor").clicked() {
                self.loop_b = t;
                self.player.set_loop(self.loop_a, self.loop_b);
                self.player.looping.store(true, Relaxed);
            }
            ui.label(format!("{:.2}–{:.2}", self.loop_a, self.loop_b));
            ui.separator();
            if ui.button("−").on_hover_text("Alejar").clicked() {
                self.view_len = (self.view_len * 1.5).min(self.tl.duration.max(1.0));
            }
            if ui.button("+").on_hover_text("Acercar").clicked() {
                self.view_len = (self.view_len / 1.5).max(0.3);
            }
            ui.separator();
            let any_dirty = self.dirty.iter().any(|d| *d);
            if ui.add_enabled(any_dirty, egui::Button::new("💾 Guardar")).clicked() {
                self.save();
            }
            if ui.button("⟳ Recargar").on_hover_text("Volver a leer el proyecto y las transcripciones del disco").clicked() {
                self.reload_from_disk();
            }
            if ui.add_enabled(self.busy.is_none(), egui::Button::new("🎬 Render borrador")).clicked() {
                self.render_draft();
            }
            if let Some(b) = &self.busy {
                ui.spinner();
                ui.label(b.clone());
            }
        });
    }

    fn side(&mut self, ui: &mut egui::Ui) {
        ui.heading("Palabra");
        let fps = self.tl.fps;
        if let Some(k) = self.selected {
            if let Some(w) = self.out_word(k).cloned() {
                let (score, locked) = {
                    let tw = &self.trs[k.tr].1.segments[k.seg].words.get(k.idx);
                    (tw.and_then(|x| x.score), tw.map(|x| x.locked).unwrap_or(false))
                };
                ui.label(egui::RichText::new(&w.word).size(22.0).strong());
                ui.label(format!(
                    "confianza: {}",
                    score.map(|s| format!("{:.0}%", s.clamp(0.0, 1.0) * 100.0)).unwrap_or_else(|| "—".into())
                ));
                egui::Grid::new("wgrid").num_columns(4).show(ui, |ui| {
                    ui.label("Inicio");
                    ui.monospace(format!("{:.3}s  f{}", w.start, frame_at(w.start, fps)));
                    if ui.small_button("−1f").clicked() {
                        self.nudge(DragKind::Start, -1);
                    }
                    if ui.small_button("+1f").clicked() {
                        self.nudge(DragKind::Start, 1);
                    }
                    ui.end_row();
                    ui.label("Fin");
                    ui.monospace(format!("{:.3}s  f{}", w.end, frame_at(w.end, fps)));
                    if ui.small_button("−1f").clicked() {
                        self.nudge(DragKind::End, -1);
                    }
                    if ui.small_button("+1f").clicked() {
                        self.nudge(DragKind::End, 1);
                    }
                    ui.end_row();
                    ui.label("Mover");
                    ui.label("");
                    if ui.small_button("−1f").clicked() {
                        self.nudge(DragKind::Move, -1);
                    }
                    if ui.small_button("+1f").clicked() {
                        self.nudge(DragKind::Move, 1);
                    }
                    ui.end_row();
                });
                let mut lk = locked;
                if ui.checkbox(&mut lk, "Fijada (la realineación no la mueve)").changed() {
                    if let Some(tw) = self.word_mut(k) {
                        tw.locked = lk;
                    }
                    self.dirty[k.tr] = true;
                }
                if ui.button("🔁 Escuchar en bucle (L)").clicked() {
                    self.loop_word();
                }
            }
        } else {
            ui.label("Haz clic en una palabra de la línea de tiempo (o pulsa Tab).");
        }
        ui.separator();
        ui.heading("Texto del segmento");
        ui.label("Corrige el texto y realinea: los tiempos se recalculan con la voz.");
        ui.add(egui::TextEdit::multiline(&mut self.seg_text).desired_rows(4).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.add_enabled(self.busy.is_none() && self.selected.is_some(), egui::Button::new("Realinear segmento")).clicked() {
                self.realign(true);
            }
            if ui.add_enabled(self.busy.is_none() && !self.trs.is_empty(), egui::Button::new("Realinear todo")).clicked() {
                self.realign(false);
            }
        });
        ui.separator();
        ui.small(format!(
            "{} palabras · {} transcripción/es · {:.1} s · {} fps",
            self.words.len(),
            self.trs.len(),
            self.tl.duration,
            self.tl.fps
        ));
        let mut avisos = self.tl.warnings.clone();
        avisos.extend(self.render_warnings.lock().unwrap().iter().cloned());
        for a in avisos.iter().take(4) {
            ui.colored_label(Color32::from_rgb(220, 160, 60), format!("Aviso: {a}"));
        }
        if avisos.len() > 4 {
            ui.small(format!("… y {} aviso(s) más (montaje check)", avisos.len() - 4));
        }
        ui.separator();
        ui.small("Atajos: Espacio reproducir · Tab/Mayús+Tab palabra siguiente/anterior · flechas izq./der. inicio ±1 fotograma · Mayús+flechas fin · Ctrl+flechas mover palabra · L bucle · Ctrl+S guardar · rueda: desplazar · Ctrl+rueda: zoom · arrastra los bordes de una palabra para ajustarla");
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        if let Some((_, f)) = self.slot.lock().unwrap().take() {
            let img = egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.data);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ui.ctx().load_texture("preview", img, egui::TextureOptions::LINEAR)),
            }
        }
        let avail = ui.available_size();
        let aspect = self.tl.w as f32 / self.tl.h as f32;
        let (w, h) = if avail.x / avail.y.max(1.0) > aspect { (avail.y * aspect, avail.y) } else { (avail.x, avail.x / aspect) };
        let (rect, _) = ui.allocate_exact_size(avail, Sense::hover());
        let r = Rect::from_center_size(rect.center(), Vec2::new(w, h));
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 0.0, Color32::from_gray(12));
        if let Some(t) = &self.tex {
            p.image(t.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_size();
        let (resp, painter) = ui.allocate_painter(Vec2::new(avail.x, avail.y.max(160.0)), Sense::click_and_drag());
        let rect = resp.rect;
        let fps = self.tl.fps;
        let dur = self.tl.duration.max(0.1);
        self.view_len = self.view_len.clamp(0.3, dur.max(0.3));
        self.view_start = self.view_start.clamp(0.0, (dur - self.view_len).max(0.0));
        let (v0, vl) = (self.view_start, self.view_len);
        let x_of = |t: f64| rect.left() + ((t - v0) / vl) as f32 * rect.width();
        let t_of = |x: f32| v0 + ((x - rect.left()) / rect.width()) as f64 * vl;
        let ruler = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.top() + 22.0));
        let wave = Rect::from_min_max(Pos2::new(rect.left(), ruler.bottom()), Pos2::new(rect.right(), ruler.bottom() + (rect.height() - 22.0 - 46.0 - 46.0).max(50.0)));
        let lane_w = Rect::from_min_max(Pos2::new(rect.left(), wave.bottom() + 13.0), Pos2::new(rect.right(), wave.bottom() + 46.0));
        let lane_l = Rect::from_min_max(Pos2::new(rect.left(), lane_w.bottom() + 4.0), Pos2::new(rect.right(), rect.bottom()));
        painter.rect_filled(rect, 0.0, Color32::from_gray(22));
        painter.rect_filled(ruler, 0.0, Color32::from_gray(34));
        // regla: segundos y fotogramas
        let px_per_s = rect.width() as f64 / vl;
        let step = [0.04, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| s * px_per_s >= 70.0).unwrap_or(120.0);
        let mut s = (v0 / step).floor() * step;
        while s <= v0 + vl {
            let x = x_of(s);
            painter.vline(x, ruler.y_range(), Stroke::new(1.0, Color32::from_gray(90)));
            painter.text(Pos2::new(x + 3.0, ruler.top() + 3.0), Align2::LEFT_TOP, format!("{:.2}", s), FontId::monospace(11.0), Color32::from_gray(200));
            s += step;
        }
        let px_per_frame = px_per_s / fps;
        if px_per_frame >= 6.0 {
            let f0 = (v0 * fps).floor() as i64;
            let f1 = ((v0 + vl) * fps).ceil() as i64;
            for f in f0..=f1 {
                let x = x_of(f as f64 / fps);
                painter.vline(x, (ruler.bottom() - 5.0)..=ruler.bottom(), Stroke::new(1.0, Color32::from_gray(130)));
                if px_per_frame >= 14.0 {
                    painter.vline(x, wave.y_range(), Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 12)));
                }
            }
        }
        // bucle
        if self.player.looping.load(Relaxed) && self.loop_b > self.loop_a {
            let r = Rect::from_min_max(Pos2::new(x_of(self.loop_a), ruler.top()), Pos2::new(x_of(self.loop_b), rect.bottom()));
            painter.rect_filled(r.intersect(rect), 0.0, Color32::from_rgba_unmultiplied(80, 140, 255, 28));
        }
        // forma de onda
        let mid = wave.center().y;
        let amp = wave.height() * 0.48;
        let n = self.mix.len() / 2;
        let cols = rect.width() as usize;
        for c in 0..cols {
            let ta = t_of(rect.left() + c as f32);
            let tb = t_of(rect.left() + c as f32 + 1.0);
            let (mut lo, mut hi, mut rms) = (0f32, 0f32, 0f32);
            let span_buckets = (tb - ta) * PEAK_HZ;
            if span_buckets >= 1.0 {
                let a = (ta * PEAK_HZ).max(0.0) as usize;
                let b = ((tb * PEAK_HZ).ceil() as usize).min(self.peaks.len());
                for p in &self.peaks[a.min(b)..b] {
                    lo = lo.min(p.0);
                    hi = hi.max(p.1);
                    rms = rms.max(p.2);
                }
            } else {
                let a = (ta * SR).max(0.0) as usize;
                let b = ((tb * SR).ceil() as usize).min(n);
                for i in a.min(b)..b {
                    let v = (self.mix[i * 2] + self.mix[i * 2 + 1]) * 0.5;
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                rms = hi.max(-lo) * 0.7;
            }
            let x = rect.left() + c as f32 + 0.5;
            // pico tenue + energía (RMS) brillante: así se ven las sílabas aunque la mezcla esté saturada
            painter.vline(x, (mid - hi * amp)..=(mid - lo * amp + 1.0), Stroke::new(1.0, Color32::from_rgb(45, 85, 120)));
            painter.vline(x, (mid - rms * amp)..=(mid + rms * amp + 1.0), Stroke::new(1.0, Color32::from_rgb(110, 190, 245)));
        }
        // capas (cutaways, textos, etc. que no son subtítulos)
        painter.text(Pos2::new(lane_l.left() + 3.0, lane_l.top()), Align2::LEFT_TOP, "capas", FontId::proportional(10.0), Color32::from_gray(120));
        let mut row = 0;
        for l in &self.tl.layers {
            if l.l.word_times.is_some() || l.l.kind == "particles" {
                continue;
            }
            if l.end < v0 || l.start > v0 + vl {
                continue;
            }
            let name = match l.l.kind.as_str() {
                "text" => l.l.text.clone().unwrap_or_default(),
                _ => l.path.as_ref().and_then(|p| Path::new(p).file_name().map(|s| s.to_string_lossy().to_string())).unwrap_or_else(|| l.l.kind.clone()),
            };
            let y = lane_l.top() + 12.0 + (row % 2) as f32 * 14.0;
            row += 1;
            let r = Rect::from_min_max(Pos2::new(x_of(l.start).max(rect.left()), y), Pos2::new(x_of(l.end).min(rect.right()), y + 12.0));
            let col = if l.l.base.unwrap_or(false) { Color32::from_rgb(150, 90, 200) } else { Color32::from_rgb(110, 110, 120) };
            painter.rect_filled(r, 2.0, col);
            painter.text(Pos2::new(r.left() + 2.0, r.center().y), Align2::LEFT_CENTER, name, FontId::proportional(10.0), Color32::WHITE);
        }
        // palabras
        let hover = resp.hover_pos();
        let mut hovered: Option<(WKey, DragKind)> = None;
        for w in &self.words {
            let (mut s, mut e) = (w.start, w.end);
            let key = WKey { tr: w.tr, seg: w.seg, idx: w.idx };
            if let Some(d) = &self.drag {
                if d.key == key {
                    s = d.cur.0;
                    e = d.cur.1;
                }
            }
            if e < v0 || s > v0 + vl {
                continue;
            }
            let r = Rect::from_min_max(Pos2::new(x_of(s), lane_w.top()), Pos2::new(x_of(e).max(x_of(s) + 2.0), lane_w.bottom()));
            let tw = self.trs[w.tr].1.segments.get(w.seg).and_then(|sg| sg.words.get(w.idx));
            let mut col = score_color(tw.and_then(|x| x.score), tw.map(|x| x.locked).unwrap_or(false));
            let sel = self.selected == Some(key);
            if sel {
                col = Color32::from_rgb(235, 200, 60);
            }
            painter.rect(r, 3.0, col.gamma_multiply(if sel { 1.0 } else { 0.85 }), Stroke::new(1.0, Color32::from_gray(15)), StrokeKind::Inside);
            painter.vline(r.left(), wave.y_range(), Stroke::new(1.0, col.gamma_multiply(0.45)));
            if r.width() > 14.0 {
                let txt_col = if sel { Color32::BLACK } else { Color32::WHITE };
                painter.with_clip_rect(r).text(Pos2::new(r.left() + 3.0, r.center().y), Align2::LEFT_CENTER, &w.word, FontId::proportional(12.0), txt_col);
            }
            if let Some(h) = hover {
                if r.expand2(Vec2::new(4.0, 0.0)).contains(h) && hovered.is_none() {
                    let kind = if (h.x - r.left()).abs() <= 5.0 {
                        DragKind::Start
                    } else if (h.x - r.right()).abs() <= 5.0 {
                        DragKind::End
                    } else {
                        DragKind::Move
                    };
                    hovered = Some((key, kind));
                }
            }
        }
        painter.text(Pos2::new(lane_w.left() + 3.0, lane_w.top() - 12.0), Align2::LEFT_TOP, "palabras", FontId::proportional(10.0), Color32::from_gray(150));
        // cursor
        let t = self.player.time();
        let xp = x_of(t);
        if xp >= rect.left() && xp <= rect.right() {
            painter.vline(xp, rect.y_range(), Stroke::new(2.0, Color32::from_rgb(240, 70, 70)));
        }
        // seguir el cursor al reproducir
        if self.player.playing.load(Relaxed) && self.drag.is_none() && (t > v0 + vl * 0.9 || t < v0) {
            self.view_start = (t - vl * 0.1).max(0.0);
        }
        // cursores del ratón
        if let Some((_, kind)) = hovered {
            ui.ctx().set_cursor_icon(if kind == DragKind::Move { egui::CursorIcon::Grab } else { egui::CursorIcon::ResizeHorizontal });
        }
        // interacción
        let ptr = resp.interact_pointer_pos();
        if resp.drag_started() {
            if let (Some(p), Some((key, kind))) = (ptr, hovered) {
                if lane_w.expand(4.0).contains(p) {
                    if let Some(w) = self.out_word(key).cloned() {
                        self.select(Some(key));
                        self.drag = Some(Drag { kind, key, grab_t: t_of(p.x), orig: (w.start, w.end), cur: (w.start, w.end) });
                    }
                }
            }
        }
        if resp.dragged() {
            if let (Some(p), Some(d)) = (ptr, self.drag.as_mut()) {
                // ajustar a la rejilla de fotogramas
                let snap = |x: f64| (x * fps).round() / fps;
                let dt = t_of(p.x) - d.grab_t;
                d.cur = match d.kind {
                    DragKind::Start => (snap(d.orig.0 + dt).min(d.orig.1 - 1.0 / fps), d.orig.1),
                    DragKind::End => (d.orig.0, snap(d.orig.1 + dt).max(d.orig.0 + 1.0 / fps)),
                    DragKind::Move => {
                        let s = snap(d.orig.0 + dt);
                        (s, s + (d.orig.1 - d.orig.0))
                    }
                };
            } else if let Some(p) = ptr {
                if ruler.union(wave).contains(p) {
                    self.seek(t_of(p.x));
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(d) = self.drag.take() {
                if (d.cur.0 - d.orig.0).abs() > 1e-6 || (d.cur.1 - d.orig.1).abs() > 1e-6 {
                    self.set_word_out(d.key, d.cur.0, d.cur.1);
                    self.status = format!("Palabra ajustada a {:.3}–{:.3} s (fijada)", d.cur.0, d.cur.1);
                }
            }
        }
        if resp.clicked() {
            if let Some(p) = ptr {
                if let Some((key, _)) = hovered.filter(|_| lane_w.expand(4.0).contains(p)) {
                    self.select(Some(key));
                    if let Some(w) = self.out_word(key).cloned() {
                        self.seek(w.start);
                    }
                } else {
                    self.seek(t_of(p.x));
                }
            }
        }
        // rueda: desplazar; Ctrl+rueda: zoom alrededor del ratón
        if resp.hovered() {
            let (scroll, zoom, ctrl) = ui.input(|i| (i.smooth_scroll_delta(), i.zoom_delta(), i.modifiers.ctrl));
            if (zoom - 1.0).abs() > 1e-3 || (ctrl && scroll.y.abs() > 0.0) {
                let f = if (zoom - 1.0).abs() > 1e-3 { 1.0 / zoom as f64 } else if scroll.y > 0.0 { 0.85 } else { 1.18 };
                let anchor = hover.map(|h| t_of(h.x)).unwrap_or(v0 + vl / 2.0);
                let nl = (vl * f).clamp(0.3, dur.max(0.3));
                self.view_start = (anchor - (anchor - v0) * nl / vl).max(0.0);
                self.view_len = nl;
            } else if scroll.x.abs() + scroll.y.abs() > 0.0 {
                let d = -(scroll.x + scroll.y) as f64 / rect.width() as f64 * vl;
                self.view_start = (v0 + d).max(0.0);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_jobs();
        self.keys(&ctx);
        egui::Panel::top("barra").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("estado").show(ui, |ui| {
            ui.small(&self.status);
        });
        egui::Panel::bottom("linea").resizable(true).default_size(300.0).min_size(180.0).show(ui, |ui| {
            self.timeline(ui);
        });
        egui::Panel::right("lado").resizable(true).default_size(330.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.side(ui));
        });
        egui::CentralPanel::default().show(ui, |ui| {
            self.preview(ui);
        });
        if self.player.playing.load(Relaxed) || self.busy.is_some() || self.mix_running {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

pub fn run(project: &str, model: &str) -> Result<()> {
    let path = PathBuf::from(project);
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    // validar antes de abrir la ventana
    let _ = App::load(&path)?;
    let title = format!("montaje studio — {}", path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_min_inner_size([900.0, 560.0]).with_maximized(true).with_title(title),
        ..Default::default()
    };
    let model = model.to_string();
    eframe::run_native(
        "montaje studio",
        opts,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let app = App::new(cc, path.clone(), model.clone()).map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() })?;
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow!("no se pudo abrir la ventana: {e}"))
}
