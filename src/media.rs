//! Acceso a medios a través de ffmpeg/ffprobe: inspección, decodificación de video y audio, codificación.

use crate::frame::Frame;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex, OnceLock};

pub const SR: u32 = 48_000;

#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn no_window(_cmd: &mut Command) {}

pub fn ffmpeg_bin() -> String {
    std::env::var("MONTAJE_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}
pub fn ffprobe_bin() -> String {
    std::env::var("MONTAJE_FFPROBE").unwrap_or_else(|_| "ffprobe".into())
}

pub fn ffmpeg() -> Command {
    let mut c = Command::new(ffmpeg_bin());
    c.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
    no_window(&mut c);
    c
}

#[derive(Clone, Debug, Serialize)]
pub struct MediaInfo {
    pub path: String,
    pub kind: String, // video | image | audio
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
    pub has_video: bool,
    pub audio_sample_rate: u32,
    pub audio_channels: u32,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub size_bytes: u64,
    /// la pista de video tiene canal alfa
    pub alpha: bool,
}

pub fn is_image_path(p: &str) -> bool {
    let l = p.to_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".bmp", ".tif", ".tiff"].iter().any(|e| l.ends_with(e))
}

fn parse_rate(s: &str) -> f64 {
    if let Some((a, b)) = s.split_once('/') {
        let a: f64 = a.parse().unwrap_or(0.0);
        let b: f64 = b.parse().unwrap_or(1.0);
        if b > 0.0 {
            return a / b;
        }
        return 0.0;
    }
    s.parse().unwrap_or(0.0)
}

static PROBE_CACHE: OnceLock<Mutex<HashMap<String, MediaInfo>>> = OnceLock::new();

pub fn probe(path: &str) -> Result<MediaInfo> {
    let cache = PROBE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(m) = cache.lock().unwrap().get(path) {
        return Ok(m.clone());
    }
    if !Path::new(path).exists() {
        bail!("no existe el archivo: {path}");
    }
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut cmd = Command::new(ffprobe_bin());
    no_window(&mut cmd);
    let out = cmd
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", path])
        .output()
        .context("no se pudo ejecutar ffprobe (¿está ffmpeg instalado y en el PATH?)")?;
    if !out.status.success() {
        bail!("ffprobe falló con {path}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let mut info = MediaInfo {
        path: path.to_string(),
        kind: "video".into(),
        duration: 0.0,
        width: 0,
        height: 0,
        fps: 0.0,
        has_audio: false,
        has_video: false,
        audio_sample_rate: 0,
        audio_channels: 0,
        video_codec: None,
        audio_codec: None,
        size_bytes,
        alpha: false,
    };
    if let Some(d) = v["format"]["duration"].as_str() {
        info.duration = d.parse().unwrap_or(0.0);
    }
    let mut attached_pic = false;
    if let Some(streams) = v["streams"].as_array() {
        for s in streams {
            match s["codec_type"].as_str() {
                Some("video") if !info.has_video => {
                    if s["disposition"]["attached_pic"].as_i64() == Some(1) {
                        attached_pic = true;
                        continue;
                    }
                    info.has_video = true;
                    info.width = s["width"].as_u64().unwrap_or(0) as u32;
                    info.height = s["height"].as_u64().unwrap_or(0) as u32;
                    let r = parse_rate(s["avg_frame_rate"].as_str().unwrap_or("0/0"));
                    let r2 = parse_rate(s["r_frame_rate"].as_str().unwrap_or("0/0"));
                    info.fps = if r > 0.0 && r < 240.0 { r } else { r2 };
                    info.video_codec = s["codec_name"].as_str().map(String::from);
                    let pf = s["pix_fmt"].as_str().unwrap_or("");
                    info.alpha = pf.starts_with("yuva") || pf.contains("rgba") || pf.contains("bgra") || pf.contains("argb") || pf.contains("abgr") || pf == "ya8" || pf.starts_with("gbrap") || pf == "pal8" || s["tags"]["alpha_mode"].as_str() == Some("1") || s["tags"]["ALPHA_MODE"].as_str() == Some("1");
                    // rotación (videos de móvil)
                    let mut rot = s["tags"]["rotate"].as_str().and_then(|r| r.parse::<i64>().ok()).unwrap_or(0);
                    if let Some(sd) = s["side_data_list"].as_array() {
                        for d in sd {
                            if let Some(r) = d["rotation"].as_i64() {
                                rot = r;
                            }
                        }
                    }
                    if rot.abs() % 180 == 90 {
                        std::mem::swap(&mut info.width, &mut info.height);
                    }
                    if info.duration <= 0.0 {
                        if let Some(d) = s["duration"].as_str() {
                            info.duration = d.parse().unwrap_or(0.0);
                        }
                    }
                }
                Some("audio") if !info.has_audio => {
                    info.has_audio = true;
                    info.audio_sample_rate =
                        s["sample_rate"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0);
                    info.audio_channels = s["channels"].as_u64().unwrap_or(0) as u32;
                    info.audio_codec = s["codec_name"].as_str().map(String::from);
                }
                _ => {}
            }
        }
    }
    let _ = attached_pic;
    if is_image_path(path) || (info.has_video && info.duration <= 0.05 && !info.has_audio) {
        info.kind = "image".into();
        info.duration = 0.0;
    } else if !info.has_video && info.has_audio {
        info.kind = "audio".into();
    }
    if !info.has_video && !info.has_audio && !is_image_path(path) {
        bail!("{path} no tiene pistas de video ni de audio");
    }
    cache.lock().unwrap().insert(path.to_string(), info.clone());
    Ok(info)
}

/// Cómo encajar la fuente en el lienzo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    Cover,
    Contain,
    Stretch,
    Blur,
}

impl Fit {
    pub fn parse(s: Option<&str>) -> Fit {
        match s.unwrap_or("cover") {
            "contain" | "fit" => Fit::Contain,
            "stretch" | "fill" => Fit::Stretch,
            "blur" | "blur_fill" => Fit::Blur,
            _ => Fit::Cover,
        }
    }
}

/// Recorte normalizado (x, y, w, h) aplicado antes de escalar.
pub type Crop = Option<[f32; 4]>;

pub fn scale_filter(w: usize, h: usize, fit: Fit, crop: Crop) -> String {
    scale_filter_fmt(w, h, fit, crop, false)
}

/// Con `yuv` = true termina en yuv420p BT.709 (más rápido; solo para fuentes opacas sin "contain").
pub fn scale_filter_fmt(w: usize, h: usize, fit: Fit, crop: Crop, yuv: bool) -> String {
    let mut pre = String::new();
    if let Some([cx, cy, cw, ch]) = crop {
        pre = format!(
            "crop=trunc(iw*{cw}/2)*2:trunc(ih*{ch}/2)*2:trunc(iw*{cx}):trunc(ih*{cy}),",
        );
    }
    let f = match fit {
        Fit::Cover => format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase:flags=bicubic,crop={w}:{h},format=rgba"
        ),
        Fit::Contain => format!(
            "scale={w}:{h}:force_original_aspect_ratio=decrease:flags=bicubic,format=rgba,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black@0"
        ),
        Fit::Stretch => format!("scale={w}:{h}:flags=bicubic,format=rgba"),
        Fit::Blur => format!(
            "split[a][b];[a]scale={sw}:{sh}:force_original_aspect_ratio=increase,crop={sw}:{sh},boxblur=12:2,scale={w}:{h},eq=brightness=-0.08[bg];[b]scale={w}:{h}:force_original_aspect_ratio=decrease:flags=bicubic[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2,format=rgba",
            sw = (w / 4).max(2) & !1,
            sh = (h / 4).max(2) & !1
        ),
    };
    if yuv && fit != Fit::Contain {
        let f = f.replacen("flags=bicubic", "flags=bicubic:out_color_matrix=bt709:out_range=tv", 1);
        return format!("{pre}{}", f.replace("format=rgba", "format=yuv420p"));
    }
    format!("{pre}{f}")
}

/// Carga una imagen fija ajustada a (w,h) según `fit`. Si fit=Exact, w/h son el tamaño final.
pub fn load_image(path: &str) -> Result<Frame> {
    let img = image::open(path)
        .or_else(|_| -> Result<image::DynamicImage> {
            // respaldo: decodificar con ffmpeg
            let out = ffmpeg()
                .args(["-i", path, "-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "pipe:1"])
                .output()?;
            Ok(image::load_from_memory(&out.stdout)?)
        })
        .with_context(|| format!("no se pudo abrir la imagen {path}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Ok(Frame::from_rgba_straight(w, h, rgba.into_raw()))
}

pub fn resize(src: &Frame, w: usize, h: usize) -> Frame {
    use crate::frame::{draw_sprite, Affine, Blend};
    let mut out = Frame::new(w, h);
    if src.w == 0 || src.h == 0 {
        return out;
    }
    // reducción de calidad: pre-reducir por factores enteros
    let mut s = src.clone();
    while s.w / 2 >= w && s.h / 2 >= h && s.w > 2 && s.h > 2 {
        s = crate::frame::downscale(&s, 2);
    }
    let m = Affine::scale(w as f32 / s.w as f32, h as f32 / s.h as f32);
    draw_sprite(&mut out, &s, &m, 1.0, Blend::Normal);
    out
}

pub fn fit_image(src: &Frame, w: usize, h: usize, fit: Fit) -> Frame {
    use crate::frame::{draw_sprite, Affine, Blend};
    let (sw, sh) = (src.w as f32, src.h as f32);
    match fit {
        Fit::Stretch => resize(src, w, h),
        Fit::Cover | Fit::Contain | Fit::Blur => {
            let sc = if fit == Fit::Cover {
                (w as f32 / sw).max(h as f32 / sh)
            } else {
                (w as f32 / sw).min(h as f32 / sh)
            };
            let nw = (sw * sc).round().max(1.0) as usize;
            let nh = (sh * sc).round().max(1.0) as usize;
            let scaled = resize(src, nw, nh);
            let mut out = Frame::new(w, h);
            if fit == Fit::Blur {
                let bsc = (w as f32 / sw).max(h as f32 / sh);
                let bg = resize(src, ((sw * bsc) as usize).max(1), ((sh * bsc) as usize).max(1));
                let mut bgc = Frame::new(w, h);
                let m = Affine::translate((w as f32 - bg.w as f32) / 2.0, (h as f32 - bg.h as f32) / 2.0);
                draw_sprite(&mut bgc, &bg, &m, 1.0, Blend::Normal);
                crate::frame::blur(&mut bgc, (w.max(h) as f32) * 0.03);
                crate::frame::mix_color(&mut bgc, [0, 0, 0, 255], 0.25);
                out = bgc;
            }
            let m = Affine::translate(
                ((w as f32 - nw as f32) / 2.0).round(),
                ((h as f32 - nh as f32) / 2.0).round(),
            );
            draw_sprite(&mut out, &scaled, &m, 1.0, Blend::Normal);
            out
        }
    }
}

// ---------------------------------------------------------------- video

pub struct VideoStream {
    path: String,
    w: usize,
    h: usize,
    fps: f64,
    fit: Fit,
    crop: Crop,
    reverse: bool,
    /// rango de la fuente decodificado [a, b]
    a: f64,
    b: f64,
    /// tiempo de la fuente correspondiente al frame índice 0
    origin: f64,
    child: Option<Child>,
    rx: Option<Receiver<Option<Frame>>>,
    idx: i64,
    cur: Option<Arc<Frame>>,
    eof: bool,
    pub spawns: u32,
    alpha: bool,
    decoder: Option<&'static str>,
}

impl VideoStream {
    pub fn new(path: &str, w: usize, h: usize, fps: f64, fit: Fit, crop: Crop, reverse: bool, a: f64, b: f64) -> Self {
        let info = probe(path).ok();
        let alpha = info.as_ref().map(|i| i.alpha).unwrap_or(false);
        let decoder = match (alpha, info.as_ref().and_then(|i| i.video_codec.as_deref())) {
            (true, Some("vp9")) => Some("libvpx-vp9"),
            (true, Some("vp8")) => Some("libvpx"),
            _ => None,
        };
        VideoStream {
            alpha,
            decoder,
            path: path.to_string(),
            w,
            h,
            fps: fps.clamp(1.0, 120.0),
            fit,
            crop,
            reverse,
            a: a.max(0.0),
            b: b.max(a + 0.001),
            origin: 0.0,
            child: None,
            rx: None,
            idx: -1,
            cur: None,
            eof: false,
            spawns: 0,
        }
    }

    fn kill(&mut self) {
        self.rx = None;
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn spawn(&mut self, from: f64) -> Result<()> {
        self.kill();
        self.spawns += 1;
        let yuv = !self.alpha && self.fit != Fit::Contain && self.w % 2 == 0 && self.h % 2 == 0;
        let vf = scale_filter_fmt(self.w, self.h, self.fit, self.crop, yuv);
        let mut cmd = ffmpeg();
        let (ss, dur, filter);
        if self.reverse {
            // decodifica [a, from] invertido: el frame 0 corresponde a `from`
            ss = self.a;
            dur = (from - self.a).max(1.0 / self.fps) + 1.0 / self.fps;
            filter = format!("fps={},{},reverse", self.fps, vf);
            self.origin = from;
        } else {
            ss = from;
            dur = (self.b - from).max(0.0) + 2.0 / self.fps;
            filter = format!("fps={},{}", self.fps, vf);
            self.origin = from;
        }
        cmd.args(["-ss", &format!("{ss:.4}")]);
        if let Some(d) = self.decoder {
            cmd.args(["-c:v", d]);
        }
        cmd.args(["-i", &self.path])
            .args(["-t", &format!("{dur:.4}")])
            .args(["-map", "0:v:0", "-an", "-sn", "-dn"])
            .args(["-vf", &filter])
            .args(["-f", "rawvideo", "-pix_fmt", if yuv { "yuv420p" } else { "rgba" }, "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().context("no se pudo lanzar ffmpeg para decodificar")?;
        let mut stdout = child.stdout.take().unwrap();
        let (w, h) = (self.w, self.h);
        let alpha = self.alpha;
        let (tx, rx) = sync_channel::<Option<Frame>>(6);
        std::thread::spawn(move || {
            let n = if yuv { w * h * 3 / 2 } else { w * h * 4 };
            loop {
                let mut buf = vec![0u8; n];
                if stdout.read_exact(&mut buf).is_err() {
                    let _ = tx.send(None);
                    break;
                }
                // ffmpeg entrega alfa 255 o relleno (0,0,0,0): ya es válido como premultiplicado,
                // salvo fuentes con canal alfa real.
                let f = if yuv {
                    Frame::from_yuv420(w, h, &buf)
                } else if alpha {
                    Frame::from_rgba_straight(w, h, buf)
                } else {
                    Frame { w, h, data: buf }
                };
                if tx.send(Some(f)).is_err() {
                    break;
                }
            }
        });
        self.child = Some(child);
        self.rx = Some(rx);
        self.idx = -1;
        self.eof = false;
        Ok(())
    }

    fn read_one(&mut self) -> bool {
        if self.eof {
            return false;
        }
        match self.rx.as_ref().and_then(|r| r.recv().ok()).flatten() {
            Some(f) => {
                self.cur = Some(Arc::new(f));
                self.idx += 1;
                true
            }
            None => {
                self.eof = true;
                false
            }
        }
    }

    /// Arranca el decodificador en `t` sin esperar (precarga).
    pub fn prime(&mut self, t: f64) -> Result<()> {
        if self.rx.is_none() {
            self.spawn(t.clamp(self.a, self.b))?;
        }
        Ok(())
    }

    /// Frame correspondiente al tiempo `t` de la fuente.
    pub fn get(&mut self, t: f64) -> Result<Option<Arc<Frame>>> {
        let t = t.clamp(self.a, self.b);
        let fps = self.fps;
        let need_spawn = self.rx.is_none();
        let rev = self.reverse;
        let target = move |origin: f64| -> i64 {
            let d = if rev { origin - t } else { t - origin };
            (d * fps + 0.25).floor() as i64
        };
        if need_spawn {
            self.spawn(t)?;
        } else {
            let tg = target(self.origin);
            if tg < self.idx || tg - self.idx > (fps * 4.0) as i64 {
                self.spawn(t)?;
            }
        }
        let tg = target(self.origin).max(0);
        while self.idx < tg {
            if !self.read_one() {
                break;
            }
        }
        if self.cur.is_none() {
            // fuente sin frames en ese punto: retroceder un poco
            if self.spawns < 4 && !self.reverse {
                let back = (t - 0.5).max(0.0);
                self.spawn(back)?;
                while self.read_one() {}
            }
        }
        Ok(self.cur.clone())
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Lee frames uniformemente espaciados de [start, end] a tamaño (w,h). Devuelve (tiempo, frame).
pub fn grab_frames(path: &str, start: f64, end: f64, count: usize, w: usize, h: usize) -> Result<Vec<(f64, Frame)>> {
    let count = count.max(1);
    let span = (end - start).max(0.04);
    let step = span / count as f64;
    let mut cmd = ffmpeg();
    let rate = 1.0 / step;
    cmd.args(["-ss", &format!("{start:.3}"), "-i", path, "-t", &format!("{span:.3}")])
        .args(["-map", "0:v:0", "-an"])
        .args(["-vf", &format!("fps={rate:.6}:start_time=0:round=down,{}", scale_filter(w, h, Fit::Contain, None))])
        .args(["-frames:v", &count.to_string()])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output()?;
    let n = w * h * 4;
    let mut frames = Vec::new();
    for (i, chunk) in out.stdout.chunks_exact(n).enumerate() {
        frames.push((start + i as f64 * step, Frame::from_rgba_straight(w, h, chunk.to_vec())));
    }
    if frames.is_empty() {
        bail!("no se obtuvieron frames de {path}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(frames)
}

// ---------------------------------------------------------------- audio

/// Decodifica audio a estéreo f32 intercalado a 48 kHz.
pub fn decode_audio(path: &str, start: f64, dur: Option<f64>, tempo: f64) -> Result<Vec<f32>> {
    let mut cmd = ffmpeg();
    if start > 0.0 {
        cmd.args(["-ss", &format!("{start:.4}")]);
    }
    cmd.args(["-i", path]);
    if let Some(d) = dur {
        cmd.args(["-t", &format!("{:.4}", d.max(0.01))]);
    }
    cmd.args(["-map", "0:a:0", "-vn", "-sn"]);
    if (tempo - 1.0).abs() > 1e-4 {
        let mut chain = Vec::new();
        let mut t = tempo;
        while t < 0.5 {
            chain.push("atempo=0.5".to_string());
            t /= 0.5;
        }
        while t > 2.0 {
            chain.push("atempo=2.0".to_string());
            t /= 2.0;
        }
        chain.push(format!("atempo={t:.6}"));
        cmd.args(["-af", &chain.join(",")]);
    }
    cmd.args(["-ac", "2", "-ar", &SR.to_string(), "-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().context("no se pudo lanzar ffmpeg para el audio")?;
    if !out.status.success() && out.stdout.is_empty() {
        bail!("ffmpeg no pudo decodificar audio de {path}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let mut v = Vec::with_capacity(out.stdout.len() / 4);
    for c in out.stdout.chunks_exact(4) {
        v.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    Ok(v)
}

/// Mono a una frecuencia dada, para análisis.
pub fn decode_audio_mono(path: &str, start: f64, dur: Option<f64>, sr: u32) -> Result<Vec<f32>> {
    let mut cmd = ffmpeg();
    if start > 0.0 {
        cmd.args(["-ss", &format!("{start:.4}")]);
    }
    cmd.args(["-i", path]);
    if let Some(d) = dur {
        cmd.args(["-t", &format!("{:.4}", d.max(0.01))]);
    }
    cmd.args(["-map", "0:a:0", "-vn", "-ac", "1", "-ar", &sr.to_string(), "-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output()?;
    if out.stdout.is_empty() {
        bail!("sin audio en {path}: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(out.stdout.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

pub fn write_wav(path: &Path, samples: &[f32], channels: u16, sr: u32) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sr.to_le_bytes())?;
    f.write_all(&(sr * channels as u32 * 2).to_le_bytes())?;
    f.write_all(&(channels * 2).to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    let mut buf = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    f.write_all(&buf)?;
    Ok(())
}

// ---------------------------------------------------------------- encoder

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Codec {
    Nvenc,
    Qsv,
    Amf,
    X264,
}

fn test_encoder(name: &str) -> bool {
    let mut c = ffmpeg();
    c.args(["-f", "lavfi", "-i", "color=black:s=256x256:d=0.2", "-c:v", name, "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c.status().map(|s| s.success()).unwrap_or(false)
}

pub fn pick_codec(pref: Option<&str>) -> Codec {
    static CACHE: OnceLock<Codec> = OnceLock::new();
    match pref {
        Some("x264") | Some("cpu") | Some("libx264") => return Codec::X264,
        Some("nvenc") => return Codec::Nvenc,
        Some("qsv") => return Codec::Qsv,
        Some("amf") => return Codec::Amf,
        _ => {}
    }
    *CACHE.get_or_init(|| {
        if test_encoder("h264_nvenc") {
            Codec::Nvenc
        } else if test_encoder("h264_qsv") {
            Codec::Qsv
        } else if test_encoder("h264_amf") {
            Codec::Amf
        } else {
            Codec::X264
        }
    })
}

pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr_path: PathBuf,
    yuv: Vec<u8>,
}

pub struct EncodeOpts<'a> {
    pub w: usize,
    pub h: usize,
    pub fps: f64,
    pub audio: Option<&'a Path>,
    pub out: &'a Path,
    pub quality: u8, // CRF/CQ: menor = mejor
    pub fast: bool,
    pub codec: Codec,
}

impl Encoder {
    pub fn start(o: EncodeOpts) -> Result<Encoder> {
        let mut cmd = Command::new(ffmpeg_bin());
        no_window(&mut cmd);
        cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"]);
        cmd.args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
            .args(["-color_range", "tv", "-colorspace", "bt709"])
            .args(["-s", &format!("{}x{}", o.w, o.h)])
            .args(["-framerate", &format!("{}", o.fps)])
            .args(["-i", "pipe:0"]);
        if let Some(a) = o.audio {
            cmd.arg("-i").arg(a);
        }
        cmd.args(["-map", "0:v:0"]);
        if o.audio.is_some() {
            cmd.args(["-map", "1:a:0"]);
        }
        let q = o.quality.to_string();
        match o.codec {
            Codec::Nvenc => {
                cmd.args(["-c:v", "h264_nvenc", "-preset", if o.fast { "p1" } else { "p5" }]);
                cmd.args(["-tune", "hq", "-rc", "vbr", "-cq", &q, "-b:v", "0", "-spatial-aq", "1"]);
            }
            Codec::Qsv => {
                cmd.args(["-c:v", "h264_qsv", "-global_quality", &q]);
            }
            Codec::Amf => {
                cmd.args(["-c:v", "h264_amf", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q]);
            }
            Codec::X264 => {
                cmd.args(["-c:v", "libx264", "-preset", if o.fast { "ultrafast" } else { "veryfast" }]);
                cmd.args(["-crf", &q]);
            }
        }
        cmd.args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"]);
        if o.audio.is_some() {
            cmd.args(["-c:a", "aac", "-b:a", "256k"]);
        }
        cmd.args(["-movflags", "+faststart", "-shortest"]);
        cmd.arg(o.out);
        let stderr_path = std::env::temp_dir().join(format!("montaje-enc-{}.log", std::process::id()));
        let errf = std::fs::File::create(&stderr_path)?;
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(errf));
        let mut child = cmd.spawn().context("no se pudo lanzar ffmpeg para codificar")?;
        let stdin = child.stdin.take();
        Ok(Encoder { child, stdin, stderr_path, yuv: Vec::new() })
    }

    pub fn write(&mut self, f: &Frame) -> Result<()> {
        f.to_yuv420(&mut self.yuv);
        let s = self.stdin.as_mut().ok_or_else(|| anyhow!("codificador cerrado"))?;
        if let Err(e) = s.write_all(&self.yuv) {
            let log = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
            bail!("ffmpeg dejó de aceptar frames: {e}\n{log}");
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let st = self.child.wait()?;
        let log = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
        let _ = std::fs::remove_file(&self.stderr_path);
        if !st.success() {
            bail!("ffmpeg terminó con error: {log}");
        }
        Ok(())
    }
}
