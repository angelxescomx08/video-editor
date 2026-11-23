//! Análisis para que la IA "escuche" y "vea" sin abrir el archivo: tempo, beats, golpes, picos, silencios y cortes de escena.

use crate::media;
use anyhow::Result;
use rustfft::{num_complex::Complex, FftPlanner};
use serde::Serialize;
use std::process::Stdio;

const ASR: u32 = 22_050;
const HOP: usize = 512;
const WIN: usize = 1024;

#[derive(Serialize)]
pub struct Peak {
    pub time: f64,
    pub db: f32,
}

#[derive(Serialize)]
pub struct Analysis {
    pub path: String,
    pub start: f64,
    pub duration: f64,
    pub tempo_bpm: Option<f32>,
    pub beats: Vec<f64>,
    /// golpes/transientes fuertes (segundos)
    pub onsets: Vec<f64>,
    /// momentos más fuertes (gritos del público, punchlines, drops)
    pub loudest: Vec<Peak>,
    pub silences: Vec<[f64; 2]>,
    /// nivel 0-9 cada `energy_step` segundos
    pub energy: String,
    pub energy_step: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenes: Option<Vec<f64>>,
    pub notes: String,
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

pub fn analyze(path: &str, start: f64, end: Option<f64>, scenes: bool) -> Result<Analysis> {
    let info = media::probe(path)?;
    let end = end.unwrap_or(info.duration).min(info.duration.max(0.0));
    let dur = (end - start).max(0.0);
    let samples = if info.has_audio { media::decode_audio_mono(path, start, Some(dur), ASR)? } else { vec![] };
    let hop_t = HOP as f64 / ASR as f64;
    let nfr = if samples.len() > WIN { (samples.len() - WIN) / HOP + 1 } else { 0 };

    // RMS y flujo espectral
    let mut rms = vec![0f32; nfr];
    let mut flux = vec![0f32; nfr];
    if nfr > 0 {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(WIN);
        let hann: Vec<f32> = (0..WIN).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos()).collect();
        let mut prev = vec![0f32; WIN / 2];
        let mut buf = vec![Complex::new(0f32, 0f32); WIN];
        for f in 0..nfr {
            let s = &samples[f * HOP..f * HOP + WIN];
            let mut acc = 0.0f32;
            for i in 0..WIN {
                acc += s[i] * s[i];
                buf[i] = Complex::new(s[i] * hann[i], 0.0);
            }
            rms[f] = (acc / WIN as f32).sqrt();
            fft.process(&mut buf);
            let mut fl = 0.0f32;
            for b in 1..WIN / 2 {
                let m = (1.0 + 100.0 * buf[b].norm()).ln();
                let d = m - prev[b];
                if d > 0.0 {
                    fl += d;
                }
                prev[b] = m;
            }
            flux[f] = fl;
        }
    }
    let db: Vec<f32> = rms.iter().map(|r| 20.0 * r.max(1e-6).log10()).collect();

    // onsets: máximos locales sobre umbral adaptativo
    let mut onsets = Vec::new();
    let w = (0.5 / hop_t) as usize;
    let mean_flux = flux.iter().sum::<f32>() / flux.len().max(1) as f32;
    let mut last = -1.0f64;
    for i in 1..nfr.saturating_sub(1) {
        if flux[i] < flux[i - 1] || flux[i] < flux[i + 1] {
            continue;
        }
        let a = i.saturating_sub(w);
        let b = (i + w).min(nfr);
        let mut win: Vec<f32> = flux[a..b].to_vec();
        win.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let med = win[win.len() / 2];
        if flux[i] > med * 1.5 + mean_flux * 0.35 {
            let t = start + i as f64 * hop_t;
            if t - last > 0.1 {
                onsets.push((t, flux[i]));
                last = t;
            }
        }
    }

    // tempo por autocorrelación del flujo
    let mut tempo = None;
    let mut beats = Vec::new();
    if nfr > (8.0 / hop_t) as usize {
        let env: Vec<f32> = flux.iter().map(|f| f - mean_flux).collect();
        let min_lag = (60.0 / 180.0 / hop_t) as usize;
        let max_lag = (60.0 / 65.0 / hop_t) as usize;
        let mut best = (0usize, f32::MIN);
        for lag in min_lag..=max_lag.min(nfr / 2) {
            let mut acc = 0.0f32;
            for i in 0..nfr - lag {
                acc += env[i] * env[i + lag];
            }
            let bpm = 60.0 / (lag as f64 * hop_t);
            // preferencia suave por 80-140 bpm
            let wgt = (-(((bpm - 100.0) / 45.0).powi(2)) as f32 * 0.5).exp();
            let sc = acc / (nfr - lag) as f32 * wgt;
            if sc > best.1 {
                best = (lag, sc);
            }
        }
        if best.0 > 0 && best.1 > 0.0 {
            let lag = best.0;
            let bpm = 60.0 / (lag as f64 * hop_t);
            tempo = Some((bpm * 10.0).round() as f32 / 10.0);
            // fase óptima
            let mut bp = (0usize, f32::MIN);
            for ph in 0..lag {
                let mut acc = 0.0;
                let mut i = ph;
                while i < nfr {
                    acc += flux[i];
                    i += lag;
                }
                if acc > bp.1 {
                    bp = (ph, acc);
                }
            }
            let mut i = bp.0;
            let tol = (lag / 8).max(1);
            while i < nfr {
                let a = i.saturating_sub(tol);
                let b = (i + tol).min(nfr - 1);
                let mut m = i;
                for j in a..=b {
                    if flux[j] > flux[m] {
                        m = j;
                    }
                }
                beats.push(r3(start + m as f64 * hop_t));
                i += lag;
            }
        }
    }

    // picos de volumen (ventana de 0.5 s, separados >= 2 s)
    let sw = (0.5 / hop_t) as usize;
    let smooth: Vec<f32> = (0..nfr)
        .map(|i| {
            let a = i.saturating_sub(sw / 2);
            let b = (i + sw / 2).min(nfr);
            let m: f32 = rms[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a).max(1) as f32;
            10.0 * m.max(1e-12).log10()
        })
        .collect();
    let mut idx: Vec<usize> = (0..nfr).collect();
    idx.sort_by(|a, b| smooth[*b].partial_cmp(&smooth[*a]).unwrap());
    let mut loudest: Vec<Peak> = Vec::new();
    for i in idx {
        let t = start + i as f64 * hop_t;
        if loudest.iter().all(|p| (p.time - t).abs() > 2.0) {
            loudest.push(Peak { time: r3(t), db: (smooth[i] * 10.0).round() / 10.0 });
        }
        if loudest.len() >= 12 {
            break;
        }
    }
    loudest.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap());

    // silencios
    let maxdb = db.iter().cloned().fold(-120.0f32, f32::max);
    let thr = (maxdb - 35.0).max(-60.0);
    let mut silences = Vec::new();
    let mut run: Option<usize> = None;
    for i in 0..=nfr {
        let quiet = i < nfr && db[i] < thr;
        match (quiet, run) {
            (true, None) => run = Some(i),
            (false, Some(s)) => {
                if (i - s) as f64 * hop_t >= 0.3 {
                    silences.push([r3(start + s as f64 * hop_t), r3(start + i as f64 * hop_t)]);
                }
                run = None;
            }
            _ => {}
        }
    }

    // energía compacta
    let step = if dur > 300.0 { 2.0 } else if dur > 120.0 { 1.0 } else { 0.5 };
    let per = (step / hop_t) as usize;
    let mut levels = Vec::new();
    let mut i = 0;
    while i < nfr {
        let b = (i + per).min(nfr);
        let m: f32 = rms[i..b].iter().map(|x| x * x).sum::<f32>() / (b - i).max(1) as f32;
        levels.push(10.0 * m.max(1e-12).log10());
        i = b;
    }
    let mut sorted = levels.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lo = sorted.get(sorted.len() / 20).cloned().unwrap_or(-60.0);
    let hi = sorted.last().cloned().unwrap_or(0.0);
    let energy: String = levels
        .iter()
        .map(|l| {
            let v = ((l - lo) / (hi - lo).max(1.0) * 9.0).round().clamp(0.0, 9.0) as u8;
            (b'0' + v) as char
        })
        .collect();

    // onsets más fuertes, máximo 150
    let mut strong = onsets.clone();
    strong.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    strong.truncate(150);
    strong.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    if beats.len() > 400 {
        beats.truncate(400);
    }

    let scenes = if scenes && info.has_video { Some(detect_scenes(path, start, dur)?) } else { None };

    Ok(Analysis {
        path: path.to_string(),
        start,
        duration: r3(dur),
        tempo_bpm: tempo,
        beats,
        onsets: strong.into_iter().map(|o| r3(o.0)).collect(),
        loudest,
        silences,
        energy,
        energy_step: step,
        scenes,
        notes: "Tiempos en segundos de la fuente. 'loudest' suele marcar reacciones del público/punchlines; 'beats' sirve para sincronizar cortes y zooms; 'energy' es un dígito por energy_step.".into(),
    })
}

pub fn detect_scenes(path: &str, start: f64, dur: f64) -> Result<Vec<f64>> {
    let mut cmd = std::process::Command::new(media::ffmpeg_bin());
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "info"]);
    if start > 0.0 {
        cmd.args(["-ss", &format!("{start:.3}")]);
    }
    cmd.args(["-i", path, "-t", &format!("{dur:.3}")])
        .args(["-map", "0:v:0", "-an", "-vf", "scale=256:-2,select='gt(scene,0.32)',showinfo", "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let out = cmd.output()?;
    let err = String::from_utf8_lossy(&out.stderr);
    let mut v = Vec::new();
    for line in err.lines() {
        if let Some(p) = line.find("pts_time:") {
            let rest = &line[p + 9..];
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            if let Ok(t) = num.parse::<f64>() {
                v.push(r3(start + t));
            }
        }
    }
    Ok(v)
}
