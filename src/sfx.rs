//! Efectos de sonido sintetizados (sin archivos externos). Salida: estéreo intercalado a 48 kHz.

use crate::audio::{reverb_tail, Biquad};
use crate::media::SR;
use std::f32::consts::{PI, TAU};

pub const SFX: &[(&str, &str)] = &[
    ("boom", "golpe grave cinematográfico (punchlines)"),
    ("vine_boom", "boom con cola larga, estilo meme"),
    ("impact", "impacto seco y contundente"),
    ("hit", "golpe corto"),
    ("bass_hit", "808 distorsionado"),
    ("sub_drop", "caída de subgrave larga"),
    ("whoosh", "barrido de aire (transiciones); `duration`"),
    ("swoosh", "barrido corto y agudo"),
    ("riser", "subida de tensión; `duration` (2 s)"),
    ("reverse_cymbal", "platillo invertido antes de un golpe; `duration`"),
    ("downlifter", "bajada después de un golpe; `duration`"),
    ("glitch", "ráfaga digital"),
    ("beep", "pitido de censura; `duration`"),
    ("scratch", "scratch de vinilo"),
    ("rewind", "rebobinado"),
    ("airhorn", "bocina MLG (corto-corto-largo)"),
    ("ding", "campana"),
    ("pop", "pop corto (aparición de texto)"),
    ("click", "clic"),
    ("shutter", "obturador de cámara (freeze frame)"),
    ("heartbeat", "latido"),
    ("laser", "láser/zap"),
    ("thunder", "trueno"),
    ("explosion", "explosión"),
    ("clap", "palmada"),
    ("tape_stop", "cinta que se detiene"),
    ("static", "estática de TV; `duration`"),
];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

fn n(secs: f32) -> usize {
    (secs * SR as f32) as usize
}
fn t_of(i: usize) -> f32 {
    i as f32 / SR as f32
}

fn stereo(m: &[f32], width: f32, pan_from: f32, pan_to: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(m.len() * 2);
    let len = m.len().max(1) as f32;
    let mut d = 0.0f32;
    for (i, &s) in m.iter().enumerate() {
        let p = pan_from + (pan_to - pan_from) * (i as f32 / len);
        let a = (p.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
        // pequeña decorrelación
        d = d * 0.6 + s * 0.4;
        let l = s * a.cos() * 1.414;
        let r = (s * (1.0 - width) + d * width) * a.sin() * 1.414;
        out.push(l);
        out.push(r);
    }
    out
}

fn normalize(m: &mut [f32], peak: f32) {
    let mx = m.iter().fold(0f32, |a, &b| a.max(b.abs()));
    if mx > 1e-6 {
        let g = peak / mx;
        for s in m.iter_mut() {
            *s *= g;
        }
    }
}

fn fade_edges(m: &mut [f32], fin: usize, fout: usize) {
    let len = m.len();
    for i in 0..fin.min(len) {
        m[i] *= i as f32 / fin as f32;
    }
    for i in 0..fout.min(len) {
        m[len - 1 - i] *= i as f32 / fout as f32;
    }
}

fn boom(len: f32, tail: f32) -> Vec<f32> {
    let mut rng = Rng(0x1234_5678);
    let total = n(len);
    let mut m = vec![0f32; total];
    let mut ph = 0.0f32;
    let mut lp = Biquad::lowpass(1800.0, 0.7);
    for i in 0..total {
        let t = t_of(i);
        let f = 38.0 + 110.0 * (-t * 14.0).exp();
        ph += TAU * f / SR as f32;
        let sub = ph.sin() * (-t * tail).exp();
        let body = (ph * 2.02).sin() * (-t * 9.0).exp() * 0.35;
        let click = lp.run(rng.next()) * (-t * 60.0).exp() * 0.9;
        m[i] = ((sub + body) * 1.6 + click).tanh();
    }
    fade_edges(&mut m, 8, n(0.05));
    normalize(&mut m, 0.95);
    m
}

pub fn synth(name: &str, duration: Option<f32>) -> Option<Vec<f32>> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let out = match name {
        "boom" => stereo(&boom(1.8, 2.6), 0.0, 0.0, 0.0),
        "vine_boom" => {
            let mut m = boom(1.2, 4.0);
            let tail = reverb_tail(&m, 0.85, 0.45, 2.2);
            let st = stereo(&m, 0.0, 0.0, 0.0);
            m.clear();
            mix_tail(st, tail, 0.45)
        }
        "impact" | "punch" => {
            let total = n(1.2);
            let mut m = vec![0f32; total];
            let mut lp = Biquad::lowpass(2500.0, 0.8);
            let mut ph = 0.0f32;
            for i in 0..total {
                let t = t_of(i);
                ph += TAU * (55.0 + 90.0 * (-t * 25.0).exp()) / SR as f32;
                let thump = ph.sin() * (-t * 5.0).exp();
                let crack = lp.run(rng.next()) * (-t * 22.0).exp();
                m[i] = (thump * 1.4 + crack * 0.9).tanh();
            }
            fade_edges(&mut m, 4, n(0.08));
            normalize(&mut m, 0.95);
            let tail = reverb_tail(&m, 0.6, 0.5, 1.2);
            mix_tail(stereo(&m, 0.1, 0.0, 0.0), tail, 0.25)
        }
        "hit" => {
            let total = n(0.35);
            let mut m = vec![0f32; total];
            let mut bp = Biquad::bandpass(900.0, 0.9);
            let mut ph = 0.0f32;
            for i in 0..total {
                let t = t_of(i);
                ph += TAU * (90.0 + 120.0 * (-t * 30.0).exp()) / SR as f32;
                m[i] = (ph.sin() * (-t * 14.0).exp() + bp.run(rng.next()) * (-t * 35.0).exp() * 1.2).tanh();
            }
            fade_edges(&mut m, 4, n(0.03));
            normalize(&mut m, 0.95);
            stereo(&m, 0.05, 0.0, 0.0)
        }
        "bass_hit" | "808" => {
            let total = n(1.4);
            let mut m = vec![0f32; total];
            let mut ph = 0.0f32;
            for i in 0..total {
                let t = t_of(i);
                ph += TAU * (48.0 + 60.0 * (-t * 20.0).exp()) / SR as f32;
                m[i] = (ph.sin() * 3.0).tanh() * (-t * 2.2).exp();
            }
            fade_edges(&mut m, 8, n(0.1));
            normalize(&mut m, 0.95);
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "sub_drop" => {
            let d = duration.unwrap_or(2.2);
            let total = n(d);
            let mut m = vec![0f32; total];
            let mut ph = 0.0f32;
            for i in 0..total {
                let t = t_of(i);
                let f = 28.0 + 50.0 * (-t * 2.0 / d).exp();
                ph += TAU * f / SR as f32;
                m[i] = (ph.sin() * 1.8).tanh() * (1.0 - t / d).powf(1.5);
            }
            fade_edges(&mut m, 16, n(0.05));
            normalize(&mut m, 0.95);
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "whoosh" | "swoosh" | "swipe" => {
            let short = name != "whoosh";
            let d = duration.unwrap_or(if short { 0.35 } else { 0.75 });
            let total = n(d);
            let mut m = vec![0f32; total];
            let mut bp = Biquad::bandpass(500.0, 1.4);
            let mut bp2 = Biquad::bandpass(500.0, 2.0);
            let (lo, hi) = if short { (900.0, 6000.0) } else { (300.0, 3200.0) };
            for i in 0..total {
                let x = i as f32 / total as f32;
                let shape = (x * PI).sin().powf(1.5) * (1.0 - x).powf(0.3);
                let fc = lo + (hi - lo) * (x * PI).sin().powi(2);
                if i % 32 == 0 {
                    bp.set_bandpass(fc, 1.4);
                    bp2.set_bandpass(fc * 1.6, 2.0);
                }
                let nz = rng.next();
                m[i] = (bp.run(nz) * 1.4 + bp2.run(nz) * 0.6) * shape;
            }
            normalize(&mut m, 0.85);
            stereo(&m, 0.4, -0.7, 0.7)
        }
        "riser" => {
            let d = duration.unwrap_or(2.0);
            let total = n(d);
            let mut m = vec![0f32; total];
            let mut hp = Biquad::highpass(200.0, 0.7);
            let mut ph = 0.0f32;
            let mut ph2 = 0.0f32;
            for i in 0..total {
                let x = i as f32 / total as f32;
                if i % 32 == 0 {
                    hp.set_highpass(200.0 + 7000.0 * x * x, 0.9);
                }
                let f = 180.0 * (2f32).powf(x * 2.6);
                ph += f / SR as f32;
                ph2 += f * 1.007 / SR as f32;
                let saw = (ph.fract() * 2.0 - 1.0) + (ph2.fract() * 2.0 - 1.0);
                let amp = x.powf(2.2);
                m[i] = (hp.run(rng.next()) * 0.8 + saw * 0.18) * amp;
            }
            fade_edges(&mut m, n(0.05), n(0.01));
            normalize(&mut m, 0.8);
            stereo(&m, 0.5, 0.0, 0.0)
        }
        "reverse_cymbal" => {
            let d = duration.unwrap_or(1.5);
            let total = n(d);
            let mut m = vec![0f32; total];
            let mut hp = Biquad::highpass(4500.0, 0.7);
            for i in 0..total {
                let x = i as f32 / total as f32;
                m[i] = hp.run(rng.next()) * x.powi(3);
            }
            fade_edges(&mut m, 0, n(0.005));
            normalize(&mut m, 0.8);
            stereo(&m, 0.6, 0.0, 0.0)
        }
        "downlifter" => {
            let d = duration.unwrap_or(1.6);
            let total = n(d);
            let mut m = vec![0f32; total];
            let mut lp = Biquad::lowpass(8000.0, 0.8);
            for i in 0..total {
                let x = i as f32 / total as f32;
                if i % 32 == 0 {
                    lp.set_lowpass(8000.0 * (1.0 - x).powi(2) + 150.0, 1.2);
                }
                m[i] = lp.run(rng.next()) * (1.0 - x).powf(1.5);
            }
            fade_edges(&mut m, n(0.005), n(0.05));
            normalize(&mut m, 0.85);
            stereo(&m, 0.5, 0.0, 0.0)
        }
        "glitch" => {
            let total = n(duration.unwrap_or(0.4));
            let mut m = vec![0f32; total];
            let mut i = 0;
            while i < total {
                let seg = n(0.015 + (rng.next() * 0.5 + 0.5) * 0.045);
                let kind = ((rng.next() * 0.5 + 0.5) * 4.0) as u32;
                let f = 80.0 + (rng.next() * 0.5 + 0.5) * 2200.0;
                let mut ph = 0.0f32;
                let mut hold = 0.0f32;
                for j in 0..seg.min(total - i) {
                    ph += f / SR as f32;
                    let v = match kind {
                        0 => if ph.fract() < 0.5 { 0.7 } else { -0.7 },
                        1 => {
                            if j % 24 == 0 {
                                hold = rng.next();
                            }
                            hold
                        }
                        2 => 0.0,
                        _ => ((ph.fract() * 8.0).floor() / 4.0 - 1.0) * 0.8,
                    };
                    m[i + j] = v;
                }
                i += seg;
            }
            fade_edges(&mut m, 16, n(0.01));
            normalize(&mut m, 0.6);
            stereo(&m, 0.3, 0.0, 0.0)
        }
        "beep" => {
            let total = n(duration.unwrap_or(0.6));
            let mut m: Vec<f32> = (0..total).map(|i| (TAU * 1000.0 * t_of(i)).sin() * 0.5).collect();
            fade_edges(&mut m, n(0.005), n(0.005));
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "scratch" => {
            let total = n(0.55);
            let mut m = vec![0f32; total];
            let mut bp = Biquad::bandpass(1200.0, 1.0);
            let mut ph = 0.0f32;
            for i in 0..total {
                let t = t_of(i);
                let speed = (t * TAU * 3.6).sin(); // adelante/atrás
                let f = 150.0 + speed.abs() * 900.0;
                ph += f / SR as f32;
                let saw = ph.fract() * 2.0 - 1.0;
                if i % 32 == 0 {
                    bp.set_bandpass(400.0 + speed.abs() * 2500.0, 1.5);
                }
                let env = speed.abs().powf(0.6);
                m[i] = (saw * 0.5 + bp.run(rng.next()) * 1.2) * env;
            }
            fade_edges(&mut m, 16, n(0.03));
            normalize(&mut m, 0.85);
            stereo(&m, 0.2, 0.0, 0.0)
        }
        "rewind" => {
            let total = n(duration.unwrap_or(1.0));
            let mut m = vec![0f32; total];
            let mut bp = Biquad::bandpass(1000.0, 2.0);
            let mut ph = 0.0f32;
            for i in 0..total {
                let x = i as f32 / total as f32;
                let wob = (t_of(i) * TAU * 14.0).sin() * 0.5 + 0.5;
                let f = 300.0 + 1800.0 * x + wob * 400.0;
                ph += f / SR as f32;
                if i % 32 == 0 {
                    bp.set_bandpass(f * 1.5, 2.0);
                }
                m[i] = ((ph.fract() * 2.0 - 1.0) * 0.4 + bp.run(rng.next())) * (0.4 + 0.6 * wob) * (x * PI).sin().sqrt();
            }
            normalize(&mut m, 0.8);
            stereo(&m, 0.3, 0.0, 0.0)
        }
        "airhorn" => {
            let pattern = [(0.0f32, 0.16f32), (0.2, 0.16), (0.42, 0.9)];
            let total = n(1.45);
            let mut m = vec![0f32; total];
            let mut lp = Biquad::lowpass(3500.0, 0.8);
            let freqs = [466.0f32, 470.0, 932.0, 699.0];
            let mut ph = [0f32; 4];
            for i in 0..total {
                let t = t_of(i);
                let mut env = 0.0f32;
                for (s, d) in pattern {
                    if t >= s && t < s + d {
                        let lt = t - s;
                        env = (lt / 0.015).min(1.0) * ((s + d - t) / 0.03).min(1.0);
                    }
                }
                let vib = 1.0 + (t * TAU * 5.5).sin() * 0.004;
                let mut v = 0.0;
                for k in 0..4 {
                    ph[k] += freqs[k] * vib / SR as f32;
                    v += (ph[k].fract() * 2.0 - 1.0) * [0.5, 0.5, 0.3, 0.25][k];
                }
                m[i] = (lp.run(v * env) * 2.5).tanh();
            }
            normalize(&mut m, 0.8);
            stereo(&m, 0.2, 0.0, 0.0)
        }
        "ding" | "bell" => {
            let total = n(1.8);
            let partials = [(1.0f32, 1.0f32, 2.0f32), (2.76, 0.5, 3.5), (5.4, 0.3, 5.0), (8.93, 0.15, 7.0)];
            let mut m = vec![0f32; total];
            for i in 0..total {
                let t = t_of(i);
                let mut v = 0.0;
                for (r, a, d) in partials {
                    v += (TAU * 880.0 * r * t).sin() * a * (-t * d).exp();
                }
                m[i] = v;
            }
            fade_edges(&mut m, 32, n(0.05));
            normalize(&mut m, 0.7);
            stereo(&m, 0.2, 0.0, 0.0)
        }
        "pop" => {
            let total = n(0.12);
            let mut ph = 0.0f32;
            let mut m: Vec<f32> = (0..total)
                .map(|i| {
                    let t = t_of(i);
                    ph += TAU * (900.0 * (-t * 40.0).exp() + 180.0) / SR as f32;
                    ph.sin() * (-t * 35.0).exp()
                })
                .collect();
            fade_edges(&mut m, 8, n(0.01));
            normalize(&mut m, 0.8);
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "click" => {
            let total = n(0.03);
            let mut hp = Biquad::highpass(2000.0, 0.7);
            let mut m: Vec<f32> = (0..total).map(|i| hp.run(rng.next()) * (-t_of(i) * 300.0).exp()).collect();
            normalize(&mut m, 0.8);
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "shutter" => {
            let total = n(0.25);
            let mut bp = Biquad::bandpass(3000.0, 0.8);
            let mut m: Vec<f32> = (0..total)
                .map(|i| {
                    let t = t_of(i);
                    let e = (-t * 120.0).exp() + if t > 0.09 { (-(t - 0.09) * 90.0).exp() * 0.8 } else { 0.0 };
                    bp.run(rng.next()) * e
                })
                .collect();
            normalize(&mut m, 0.85);
            stereo(&m, 0.1, 0.0, 0.0)
        }
        "heartbeat" => {
            let total = n(0.9);
            let mut m = vec![0f32; total];
            for i in 0..total {
                let t = t_of(i);
                let beat = |s: f32, a: f32| {
                    if t >= s {
                        let lt = t - s;
                        (TAU * (50.0 + 30.0 * (-lt * 30.0).exp()) * lt).sin() * (-lt * 14.0).exp() * a
                    } else {
                        0.0
                    }
                };
                m[i] = (beat(0.0, 1.0) + beat(0.28, 0.75)) * 1.5;
            }
            for s in m.iter_mut() {
                *s = s.tanh();
            }
            normalize(&mut m, 0.95);
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "laser" | "zap" => {
            let total = n(0.35);
            let mut ph = 0.0f32;
            let mut m: Vec<f32> = (0..total)
                .map(|i| {
                    let t = t_of(i);
                    ph += (2400.0 * (-t * 10.0).exp() + 120.0) / SR as f32;
                    (if ph.fract() < 0.5 { 0.6 } else { -0.6 }) * (-t * 6.0).exp()
                })
                .collect();
            fade_edges(&mut m, 8, n(0.02));
            stereo(&m, 0.2, 0.0, 0.0)
        }
        "thunder" => {
            let total = n(3.0);
            let mut lp = Biquad::lowpass(400.0, 0.7);
            let mut lp2 = Biquad::lowpass(3000.0, 0.7);
            let mut m = vec![0f32; total];
            let mut rumble = 0.0f32;
            for i in 0..total {
                let t = t_of(i);
                if i % 2400 == 0 {
                    rumble = 0.5 + 0.5 * rng.next().abs();
                }
                let nz = rng.next();
                m[i] = lp.run(nz) * 2.5 * rumble * (-t * 1.1).exp() + lp2.run(nz) * (-t * 18.0).exp();
            }
            fade_edges(&mut m, 32, n(0.2));
            normalize(&mut m, 0.9);
            stereo(&m, 0.6, 0.0, 0.0)
        }
        "explosion" => {
            let total = n(2.4);
            let b = boom(2.4, 1.8);
            let mut lp = Biquad::lowpass(1200.0, 0.7);
            let mut m: Vec<f32> = (0..total)
                .map(|i| {
                    let t = t_of(i);
                    if i % 32 == 0 {
                        lp.set_lowpass(200.0 + 3000.0 * (-t * 3.0).exp(), 0.7);
                    }
                    (lp.run(rng.next()) * 1.6 * (-t * 1.6).exp() + b[i] * 0.8).tanh()
                })
                .collect();
            normalize(&mut m, 0.95);
            stereo(&m, 0.5, 0.0, 0.0)
        }
        "clap" => {
            let total = n(0.3);
            let mut bp = Biquad::bandpass(1400.0, 1.1);
            let mut m: Vec<f32> = (0..total)
                .map(|i| {
                    let t = t_of(i);
                    let e = [0.0f32, 0.011, 0.023]
                        .iter()
                        .map(|&s| if t >= s { (-(t - s) * 180.0).exp() } else { 0.0 })
                        .sum::<f32>()
                        + if t > 0.023 { (-(t - 0.023) * 18.0).exp() * 0.4 } else { 0.0 };
                    bp.run(rng.next()) * e
                })
                .collect();
            normalize(&mut m, 0.85);
            stereo(&m, 0.2, 0.0, 0.0)
        }
        "tape_stop" => {
            let total = n(0.9);
            let mut ph = 0.0f32;
            let mut m: Vec<f32> = (0..total)
                .map(|i| {
                    let x = i as f32 / total as f32;
                    let sp = (1.0 - x).powi(2);
                    ph += 220.0 * sp / SR as f32;
                    ((ph.fract() * 2.0 - 1.0) * 0.4 + (ph * 0.5).fract() * 0.3) * sp
                })
                .collect();
            let mut lp = Biquad::lowpass(1500.0, 0.7);
            for s in m.iter_mut() {
                *s = lp.run(*s);
            }
            normalize(&mut m, 0.7);
            stereo(&m, 0.0, 0.0, 0.0)
        }
        "static" | "noise" => {
            let total = n(duration.unwrap_or(0.6));
            let mut m: Vec<f32> = (0..total).map(|_| rng.next() * 0.5).collect();
            fade_edges(&mut m, n(0.01), n(0.02));
            stereo(&m, 0.8, 0.0, 0.0)
        }
        _ => return None,
    };
    Some(out)
}

fn mix_tail(mut dry: Vec<f32>, tail: Vec<f32>, amt: f32) -> Vec<f32> {
    if tail.len() > dry.len() {
        dry.resize(tail.len(), 0.0);
    }
    for (i, t) in tail.iter().enumerate() {
        dry[i] += t * amt;
    }
    dry
}

/// Cambia la velocidad/tono por remuestreo lineal.
pub fn repitch(st: &[f32], pitch: f32) -> Vec<f32> {
    if (pitch - 1.0).abs() < 1e-3 || pitch <= 0.0 {
        return st.to_vec();
    }
    let frames = st.len() / 2;
    let out_frames = (frames as f32 / pitch) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let pos = i as f32 * pitch;
        let i0 = pos.floor() as usize;
        let fr = pos - i0 as f32;
        let i1 = (i0 + 1).min(frames - 1);
        for c in 0..2 {
            let a = st[i0 * 2 + c];
            let b = st[i1 * 2 + c];
            out.push(a + (b - a) * fr);
        }
    }
    out
}
