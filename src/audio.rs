//! Motor de audio: filtros, reverberación, efectos por rango, limitador y mezcla final.

use crate::media::SR;
use crate::project::AudioFx;
use std::f32::consts::PI;

#[derive(Clone, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

#[derive(Clone, Copy)]
enum Kind {
    Low,
    High,
    Band,
}

impl Biquad {
    fn new(kind: Kind, f: f32, q: f32) -> Biquad {
        let mut b = Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 };
        b.set(kind, f, q);
        b
    }
    fn set(&mut self, kind: Kind, f: f32, q: f32) {
        let f = f.clamp(10.0, SR as f32 * 0.45);
        let w0 = 2.0 * PI * f / SR as f32;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q.max(0.05));
        let (b0, b1, b2) = match kind {
            Kind::Low => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0),
            Kind::High => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0),
            Kind::Band => (alpha, 0.0, -alpha),
        };
        let a0 = 1.0 + alpha;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - alpha) / a0;
    }
    pub fn lowpass(f: f32, q: f32) -> Biquad {
        Biquad::new(Kind::Low, f, q)
    }
    pub fn highpass(f: f32, q: f32) -> Biquad {
        Biquad::new(Kind::High, f, q)
    }
    pub fn bandpass(f: f32, q: f32) -> Biquad {
        Biquad::new(Kind::Band, f, q)
    }
    pub fn set_lowpass(&mut self, f: f32, q: f32) {
        self.set(Kind::Low, f, q)
    }
    pub fn set_highpass(&mut self, f: f32, q: f32) {
        self.set(Kind::High, f, q)
    }
    pub fn set_bandpass(&mut self, f: f32, q: f32) {
        self.set(Kind::Band, f, q)
    }
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    fb: f32,
    damp: f32,
    store: f32,
}
impl Comb {
    fn run(&mut self, x: f32) -> f32 {
        let y = self.buf[self.i];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.i] = x + self.store * self.fb;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}
struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
impl Allpass {
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        let y = -x + b;
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

/// Reverberación tipo Freeverb sobre una señal mono. Devuelve estéreo (solo húmedo), con cola.
pub fn reverb_tail(mono: &[f32], room: f32, damp: f32, tail_secs: f32) -> Vec<f32> {
    let scale = SR as f32 / 44_100.0;
    let combs = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    let aps = [556, 441, 341, 225];
    let mk = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
        (
            combs
                .iter()
                .map(|&l| Comb {
                    buf: vec![0.0; ((l + spread) as f32 * scale) as usize],
                    i: 0,
                    fb: room.clamp(0.0, 1.0) * 0.28 + 0.7,
                    damp: damp.clamp(0.0, 1.0) * 0.4,
                    store: 0.0,
                })
                .collect(),
            aps.iter().map(|&l| Allpass { buf: vec![0.0; ((l + spread) as f32 * scale) as usize], i: 0 }).collect(),
        )
    };
    let (mut cl, mut al) = mk(0);
    let (mut cr, mut ar) = mk(23);
    let total = mono.len() + (tail_secs * SR as f32) as usize;
    let mut out = Vec::with_capacity(total * 2);
    for i in 0..total {
        let x = if i < mono.len() { mono[i] * 0.015 } else { 0.0 };
        let mut l = 0.0;
        let mut r = 0.0;
        for c in cl.iter_mut() {
            l += c.run(x);
        }
        for c in cr.iter_mut() {
            r += c.run(x);
        }
        for a in al.iter_mut() {
            l = a.run(l);
        }
        for a in ar.iter_mut() {
            r = a.run(r);
        }
        out.push(l);
        out.push(r);
    }
    out
}

#[inline]
pub fn frames(buf: &[f32]) -> usize {
    buf.len() / 2
}

pub fn t2f(t: f64) -> i64 {
    (t * SR as f64).round() as i64
}

/// Suma `src` (estéreo) en `dst` a partir del frame `at`, con ganancia y fundidos (en frames).
pub fn mix_into(dst: &mut [f32], src: &[f32], at: i64, gain: f32, fade_in: usize, fade_out: usize, pan: f32) {
    let n = frames(src) as i64;
    let dn = frames(dst) as i64;
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    let (gl, gr) = if pan.abs() > 1e-3 { (a.cos() * 1.414, a.sin() * 1.414) } else { (1.0, 1.0) };
    for i in 0..n {
        let o = at + i;
        if o < 0 {
            continue;
        }
        if o >= dn {
            break;
        }
        let mut g = gain;
        if fade_in > 0 && (i as usize) < fade_in {
            g *= i as f32 / fade_in as f32;
        }
        let rem = (n - 1 - i) as usize;
        if fade_out > 0 && rem < fade_out {
            g *= rem as f32 / fade_out as f32;
        }
        let si = (i * 2) as usize;
        let di = (o * 2) as usize;
        dst[di] += src[si] * g * gl;
        dst[di + 1] += src[si + 1] * g * gr;
    }
}

fn edge_weight(i: usize, s: usize, e: usize, ramp: usize) -> f32 {
    let a = if i < s + ramp { (i - s) as f32 / ramp.max(1) as f32 } else { 1.0 };
    let b = if i + ramp > e { (e - i) as f32 / ramp.max(1) as f32 } else { 1.0 };
    a.min(b).clamp(0.0, 1.0)
}

/// Aplica un efecto de audio a un bus en [start, end] (segundos).
pub fn apply_fx(buf: &mut Vec<f32>, fx: &AudioFx, start: f64, end: f64) {
    let total = frames(buf);
    let s = (t2f(start).max(0) as usize).min(total);
    let e = (t2f(end).max(0) as usize).min(total);
    if e <= s {
        return;
    }
    let ramp = (0.012 * SR as f32) as usize;
    match fx.kind.as_str() {
        "lowpass" | "highpass" | "bandpass" | "telephone" | "muffle" => {
            let kind = match fx.kind.as_str() {
                "highpass" => Kind::High,
                "bandpass" | "telephone" => Kind::Band,
                _ => Kind::Low,
            };
            let def = match fx.kind.as_str() {
                "highpass" => 1200.0,
                "telephone" => 1500.0,
                "bandpass" => 1000.0,
                _ => 500.0,
            };
            let c0 = fx.cutoff.unwrap_or(def);
            let c1 = fx.cutoff_to.unwrap_or(c0);
            let q = fx.q.unwrap_or(if fx.kind == "telephone" { 0.8 } else { 0.707 });
            let mut fl = Biquad::new(kind, c0, q);
            let mut fr = Biquad::new(kind, c0, q);
            let len = (e - s).max(1);
            for i in s..e {
                if (i - s) % 64 == 0 {
                    let x = (i - s) as f32 / len as f32;
                    let c = c0 * (c1 / c0).powf(x);
                    fl.set(kind, c, q);
                    fr.set(kind, c, q);
                }
                let w = edge_weight(i, s, e, ramp);
                let gain = if fx.kind == "telephone" { 2.0 } else { 1.0 };
                let l = fl.run(buf[i * 2]) * gain;
                let r = fr.run(buf[i * 2 + 1]) * gain;
                buf[i * 2] = buf[i * 2] * (1.0 - w) + l * w;
                buf[i * 2 + 1] = buf[i * 2 + 1] * (1.0 - w) + r * w;
            }
        }
        "gain" | "volume" => {
            let g0 = fx.db.map(|d| 10f32.powf(d / 20.0)).or(fx.volume).unwrap_or(1.0);
            let g1 = fx.volume_to.unwrap_or(g0);
            let len = (e - s).max(1);
            for i in s..e {
                let x = (i - s) as f32 / len as f32;
                let g = g0 + (g1 - g0) * x;
                let w = edge_weight(i, s, e, ramp);
                let gg = 1.0 + (g - 1.0) * w;
                buf[i * 2] *= gg;
                buf[i * 2 + 1] *= gg;
            }
        }
        "mute" | "beep" | "censor" => {
            let r = (0.005 * SR as f32) as usize;
            for i in s..e {
                let w = edge_weight(i, s, e, r);
                buf[i * 2] *= 1.0 - w;
                buf[i * 2 + 1] *= 1.0 - w;
                if fx.kind != "mute" {
                    let f = fx.frequency.unwrap_or(1000.0);
                    let v = (2.0 * PI * f * (i - s) as f32 / SR as f32).sin() * 0.35 * w;
                    buf[i * 2] += v;
                    buf[i * 2 + 1] += v;
                }
            }
        }
        "echo" | "delay" => {
            let d = (fx.delay.unwrap_or(0.22) * SR as f32) as usize;
            let fb = fx.feedback.unwrap_or(0.45).clamp(0.0, 0.95);
            let mix = fx.mix.unwrap_or(0.6);
            let seg: Vec<f32> = (s..e)
                .flat_map(|i| {
                    let w = edge_weight(i, s, e, ramp);
                    [buf[i * 2] * w, buf[i * 2 + 1] * w]
                })
                .collect();
            let mut g = mix;
            let mut k = 1;
            let mut lp_l = Biquad::lowpass(6000.0, 0.7);
            let mut lp_r = Biquad::lowpass(6000.0, 0.7);
            let mut cur = seg.clone();
            while g > 0.01 && k < 40 {
                let off = s + d * k;
                if off >= total {
                    break;
                }
                // cada repetición un poco más oscura; rebote estéreo
                for j in 0..frames(&cur) {
                    cur[j * 2] = lp_l.run(cur[j * 2]);
                    cur[j * 2 + 1] = lp_r.run(cur[j * 2 + 1]);
                }
                let pan = if k % 2 == 0 { 0.35 } else { -0.35 };
                mix_into(buf, &cur, off as i64, g, 0, 0, pan);
                g *= fb;
                k += 1;
            }
        }
        "reverb" => {
            let mono: Vec<f32> = (s..e)
                .map(|i| {
                    let w = edge_weight(i, s, e, ramp);
                    (buf[i * 2] + buf[i * 2 + 1]) * 0.5 * w
                })
                .collect();
            let room = fx.size.unwrap_or(0.85);
            let tail = reverb_tail(&mono, room, 0.4, 1.5 + room * 2.5);
            mix_into(buf, &tail, s as i64, fx.mix.unwrap_or(0.45) * 3.0, 0, (0.3 * SR as f32) as usize, 0.0);
        }
        "tape_stop" => {
            let len = e - s;
            let src: Vec<f32> = buf[s * 2..(s + len) * 2].to_vec();
            let mut pos = 0.0f32;
            for i in 0..len {
                let x = i as f32 / len as f32;
                let sp = (1.0 - x).powf(1.6);
                let i0 = (pos as usize).min(len - 1);
                let fr = pos - i0 as f32;
                let i1 = (i0 + 1).min(len - 1);
                for c in 0..2 {
                    let a = src[i0 * 2 + c];
                    let b = src[i1 * 2 + c];
                    buf[(s + i) * 2 + c] = (a + (b - a) * fr) * (1.0 - x * x * 0.5);
                }
                pos += sp;
            }
        }
        "stutter" | "repeat" => {
            let slice = ((fx.slice.unwrap_or(0.12) * SR as f32) as usize).max(64);
            let reps = fx.repeats.unwrap_or(4).max(1) as usize;
            if s + slice > total {
                return;
            }
            let seg: Vec<f32> = buf[s * 2..(s + slice) * 2].to_vec();
            let r = (0.004 * SR as f32) as usize;
            for k in 1..reps {
                let off = s + slice * k;
                for j in 0..slice {
                    let o = off + j;
                    if o >= total {
                        break;
                    }
                    let w = edge_weight(j, 0, slice, r);
                    buf[o * 2] = seg[j * 2] * w;
                    buf[o * 2 + 1] = seg[j * 2 + 1] * w;
                }
            }
        }
        "reverse" => {
            let seg: Vec<f32> = buf[s * 2..e * 2].to_vec();
            let n = e - s;
            for i in 0..n {
                let w = edge_weight(i, 0, n, ramp);
                let j = n - 1 - i;
                for c in 0..2 {
                    buf[(s + i) * 2 + c] = buf[(s + i) * 2 + c] * (1.0 - w) + seg[j * 2 + c] * w;
                }
            }
        }
        "bitcrush" | "lofi" => {
            let bits = fx.bits.unwrap_or(6).clamp(2, 16);
            let ds = fx.downsample.unwrap_or(4).max(1) as usize;
            let q = (1u32 << (bits - 1)) as f32;
            let mut hold = [0f32; 2];
            for i in s..e {
                if (i - s) % ds == 0 {
                    hold = [buf[i * 2], buf[i * 2 + 1]];
                }
                let w = edge_weight(i, s, e, ramp);
                for c in 0..2 {
                    let v = (hold[c] * q).round() / q;
                    buf[i * 2 + c] = buf[i * 2 + c] * (1.0 - w) + v * w;
                }
            }
        }
        "distortion" | "saturate" => {
            let d = fx.drive.unwrap_or(4.0).max(1.0);
            let norm = d.tanh();
            for i in s..e {
                let w = edge_weight(i, s, e, ramp);
                for c in 0..2 {
                    let x = buf[i * 2 + c];
                    let v = (x * d).tanh() / norm * 0.8;
                    buf[i * 2 + c] = x * (1.0 - w) + v * w;
                }
            }
        }
        _ => {}
    }
}

/// Envolvente (0..1 aprox.) del nivel de un bus, para ducking.
pub fn envelope(buf: &[f32]) -> Vec<f32> {
    let n = frames(buf);
    let mut env = vec![0f32; n];
    let att = (-1.0 / (0.02 * SR as f32)).exp();
    let rel = (-1.0 / (0.35 * SR as f32)).exp();
    let mut e = 0.0f32;
    for i in 0..n {
        let x = buf[i * 2].abs().max(buf[i * 2 + 1].abs());
        let c = if x > e { att } else { rel };
        e = x + (e - x) * c;
        env[i] = e;
    }
    env
}

pub fn loudness_lufs(buf: &[f32]) -> Option<f32> {
    let block = (0.4 * SR as f32) as usize;
    let n = frames(buf);
    if n < block {
        return None;
    }
    let mut ms = Vec::new();
    let mut i = 0;
    while i + block <= n {
        let mut acc = 0.0f64;
        for j in i..i + block {
            let l = buf[j * 2] as f64;
            let r = buf[j * 2 + 1] as f64;
            acc += l * l + r * r;
        }
        ms.push(acc / block as f64);
        i += block / 4;
    }
    let to_l = |m: f64| -0.691 + 10.0 * m.max(1e-12).log10();
    let abs: Vec<f64> = ms.iter().cloned().filter(|&m| to_l(m) > -70.0).collect();
    if abs.is_empty() {
        return None;
    }
    let mean = abs.iter().sum::<f64>() / abs.len() as f64;
    let thr = to_l(mean) - 10.0;
    let rel: Vec<f64> = abs.into_iter().filter(|&m| to_l(m) > thr).collect();
    if rel.is_empty() {
        return None;
    }
    Some(to_l(rel.iter().sum::<f64>() / rel.len() as f64) as f32)
}

/// Limitador con anticipación: techo en `ceiling` (lineal).
pub fn limit(buf: &mut [f32], ceiling: f32) {
    let n = frames(buf);
    if n == 0 {
        return;
    }
    let look = (0.003 * SR as f32) as usize;
    let mut g: Vec<f32> = (0..n)
        .map(|i| {
            let p = buf[i * 2].abs().max(buf[i * 2 + 1].abs());
            if p > ceiling {
                ceiling / p
            } else {
                1.0
            }
        })
        .collect();
    // ataque (hacia atrás)
    for i in (0..n - 1).rev() {
        let lim = g[i + 1] + (1.0 - g[i + 1]) / look as f32;
        if g[i] > lim {
            g[i] = lim;
        }
    }
    // relajación (hacia delante)
    let rel = 1.0 - (-1.0 / (0.12 * SR as f32)).exp();
    for i in 1..n {
        let lim = g[i - 1] + (1.0 - g[i - 1]) * rel;
        if g[i] > lim {
            g[i] = lim;
        }
    }
    for i in 0..n {
        buf[i * 2] *= g[i];
        buf[i * 2 + 1] *= g[i];
    }
}

/// Lee una muestra estéreo con interpolación lineal en la posición `pos` (frames).
#[inline]
pub fn read_interp(buf: &[f32], pos: f64) -> Option<(f32, f32)> {
    let n = frames(buf);
    if pos < 0.0 || n == 0 || pos > (n - 1) as f64 {
        return None;
    }
    let i0 = pos.floor() as usize;
    let fr = (pos - i0 as f64) as f32;
    let i1 = (i0 + 1).min(n - 1);
    let l = buf[i0 * 2] + (buf[i1 * 2] - buf[i0 * 2]) * fr;
    let r = buf[i0 * 2 + 1] + (buf[i1 * 2 + 1] - buf[i0 * 2 + 1]) * fr;
    Some((l, r))
}
