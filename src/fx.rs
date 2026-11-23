//! Efectos de video. Cada efecto actúa sobre un Frame RGBA premultiplicado.

use crate::ease::Ease;
use crate::frame::*;
use crate::project::Effect;
use crate::util::*;
use rayon::prelude::*;
use serde_json::Value;

/// Contexto de render: tamaño real del lienzo y escala de píxeles respecto al proyecto.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    pub w: usize,
    pub h: usize,
    /// píxeles reales por píxel de proyecto (1.0 a resolución completa, 0.5 en vista previa)
    pub k: f32,
    #[allow(dead_code)]
    pub fps: f64,
    /// índice de frame global (para ruido)
    pub frame: u64,
    /// tiempo global de salida
    #[allow(dead_code)]
    pub time: f64,
}

/// Efecto ya resuelto en el tiempo.
#[derive(Clone, Debug)]
pub struct TFx {
    pub e: Effect,
    pub start: f64,
    pub end: f64,
    pub seed: u64,
}

impl TFx {
    pub fn active(&self, t: f64) -> bool {
        t >= self.start && t < self.end
    }
    pub fn progress(&self, t: f64) -> f32 {
        let d = (self.end - self.start).max(1e-6);
        (((t - self.start) / d) as f32).clamp(0.0, 1.0)
    }
    pub fn env(&self, t: f64) -> f32 {
        let mut v = 1.0f32;
        if let Some(fi) = self.e.fade_in {
            if fi > 0.0 {
                v = v.min(((t - self.start) / fi) as f32);
            }
        }
        if let Some(fo) = self.e.fade_out {
            if fo > 0.0 {
                v = v.min(((self.end - t) / fo) as f32);
            }
        }
        v.clamp(0.0, 1.0)
    }
    fn ease(&self, def: Ease) -> Ease {
        self.e.ease.unwrap_or(def)
    }
}

pub fn default_duration(kind: &str) -> f64 {
    match kind {
        "flash" => 0.15,
        "zoom_punch" => 0.5,
        "shake" => 0.45,
        "glitch" => 0.35,
        "rgb_split" => 0.3,
        "strobe" => 0.5,
        "zoom_blur" | "motion_blur" => 0.25,
        "blur" => 0.4,
        "zoom" => 0.6,
        "punch" | "impact" => 0.6,
        "glitch_hit" => 0.35,
        "beat" => 0.25,
        "whip" => 0.3,
        "bw_hit" => 1.0,
        _ => 1.0,
    }
}

pub fn is_geometric(kind: &str) -> bool {
    matches!(kind, "zoom" | "zoom_punch" | "shake" | "move" | "rotate")
}

pub fn default_apply_all(kind: &str) -> bool {
    matches!(kind, "flash" | "fade" | "letterbox" | "vignette" | "grain" | "strobe" | "scanlines" | "vhs")
}

fn vnum(v: &Option<Value>) -> Option<f32> {
    v.as_ref().and_then(|x| x.as_f64()).map(|x| x as f32)
}
fn vxy(v: &Option<Value>) -> Option<[f32; 2]> {
    let a = v.as_ref()?.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
}

/// Transformación geométrica combinada de los efectos activos (src->dst).
pub fn geometric(fxs: &[TFx], t: f64, ctx: &Ctx) -> (Affine, bool) {
    let (w, h) = (ctx.w as f32, ctx.h as f32);
    let mut m = Affine::IDENTITY;
    let mut shrinks = false;
    for f in fxs.iter().filter(|f| f.active(t) && is_geometric(&f.e.kind)) {
        let p = f.progress(t);
        let env = f.env(t);
        let c = f.e.center.unwrap_or([0.5, 0.5]);
        let center = (c[0] * w, c[1] * h);
        match f.e.kind.as_str() {
            "zoom" => {
                let a = vnum(&f.e.from).unwrap_or(1.0);
                let b = vnum(&f.e.to).or(f.e.scale).unwrap_or(1.2);
                let s = lerp(a, b, f.ease(Ease::OutCubic).apply(p));
                let s = lerp(1.0, s, env);
                if s < 1.0 {
                    shrinks = true;
                }
                m = m.then(&Affine::about(center, &Affine::scale(s, s)));
            }
            "zoom_punch" => {
                let target = f.e.scale.or(vnum(&f.e.to)).unwrap_or(1.25);
                let dur = (f.end - f.start) as f32;
                let lt = (t - f.start) as f32;
                let attack = (dur * 0.15).min(0.07).max(0.01);
                let s = if lt < attack {
                    lerp(1.0, target, Ease::OutQuad.apply(lt / attack))
                } else if f.e.hold.unwrap_or(false) {
                    target
                } else {
                    let q = (lt - attack) / (dur - attack).max(1e-3);
                    lerp(target, 1.0, f.ease(Ease::OutCubic).apply(q))
                };
                if s < 1.0 {
                    shrinks = true;
                }
                m = m.then(&Affine::about(center, &Affine::scale(s, s)));
            }
            "rotate" => {
                let a = vnum(&f.e.from).unwrap_or(0.0);
                let b = vnum(&f.e.to).or(f.e.angle).unwrap_or(5.0);
                let r = lerp(a, b, f.ease(Ease::InOutCubic).apply(p)) * env;
                m = m.then(&Affine::about(center, &Affine::rotate(r.to_radians())));
                if r.abs() > 0.01 {
                    let ov = 1.0 + (r.to_radians().abs().sin()) * 0.9;
                    m = m.then(&Affine::about((w / 2.0, h / 2.0), &Affine::scale(ov, ov)));
                }
            }
            "move" => {
                let a = vxy(&f.e.from).unwrap_or([0.0, 0.0]);
                let b = vxy(&f.e.to).unwrap_or([0.0, 0.0]);
                let q = f.ease(Ease::InOutCubic).apply(p);
                let dx = lerp(a[0], b[0], q) * w * env;
                let dy = lerp(a[1], b[1], q) * h * env;
                m = m.then(&Affine::translate(dx, dy));
            }
            "shake" => {
                let intensity = f.e.intensity.or(f.e.amount).unwrap_or(25.0) * ctx.k;
                let freq = f.e.frequency.unwrap_or(18.0);
                let decay = if f.e.decay.unwrap_or(true) { (1.0 - p).powf(1.4) } else { 1.0 };
                let a = intensity * env * decay;
                let tt = (t as f32) * freq;
                let dx = noise1(f.seed, tt) * a;
                let dy = noise1(f.seed.wrapping_add(7), tt) * a;
                let rdeg = f.e.rotation.unwrap_or(1.2) * env * decay;
                let r = noise1(f.seed.wrapping_add(13), tt * 0.8) * rdeg;
                let ov = 1.0 + 2.2 * a / w.min(h) + r.to_radians().abs() * 0.8;
                let mm = Affine::about((w / 2.0, h / 2.0), &Affine::scale(ov, ov).then(&Affine::rotate(r.to_radians())))
                    .then(&Affine::translate(dx, dy));
                m = m.then(&mm);
            }
            _ => {}
        }
    }
    (m, shrinks)
}

/// Aplica los efectos activos (geométricos combinados primero, luego los de píxel en orden).
pub fn apply_all(frame: &mut Frame, fxs: &[TFx], t: f64, ctx: &Ctx, edge_default: Edge) {
    if fxs.iter().all(|f| !f.active(t)) {
        return;
    }
    let (m, shrinks) = geometric(fxs, t, ctx);
    if !m.is_identity() {
        let edge = if shrinks { Edge::Transparent } else { edge_default };
        warp_in_place(frame, &m, edge);
    }
    let prof = ctx.frame % 60 == 0 && std::env::var("MONTAJE_PROFILE").is_ok();
    for f in fxs.iter().filter(|f| f.active(t) && !is_geometric(&f.e.kind)) {
        let a = std::time::Instant::now();
        apply_pixel(frame, f, t, ctx);
        if prof {
            eprintln!("[prof]   fx {} {:.2}ms", f.e.kind, a.elapsed().as_secs_f64() * 1e3);
        }
    }
}

pub fn apply_pixel(frame: &mut Frame, f: &TFx, t: f64, ctx: &Ctx) {
    let p = f.progress(t);
    let env = f.env(t);
    let e = &f.e;
    let k = ctx.k;
    match e.kind.as_str() {
        "flash" => {
            let c = color_or(&e.color, [255, 255, 255, 255]);
            let op = e.opacity.unwrap_or(0.9);
            let a = if e.fade_in.is_some() { env } else { (1.0 - p).powi(2) };
            mix_color(frame, c, a * op);
        }
        "fade" => {
            let c = color_or(&e.color, [0, 0, 0, 255]);
            let a = vnum(&e.from).unwrap_or(0.0);
            let b = vnum(&e.to).unwrap_or(1.0);
            mix_color(frame, c, lerp(a, b, f.ease(Ease::Linear).apply(p)).clamp(0.0, 1.0));
        }
        "bw" | "grayscale" => {
            let amt = e.amount.unwrap_or(1.0) * env;
            let g = Grade { saturation: 1.0 - amt, contrast: 1.0 + 0.1 * amt, ..Grade::default() };
            grade(frame, &g);
        }
        "grade" | "color" => {
            let g = Grade {
                brightness: e.brightness.unwrap_or(0.0) * env,
                contrast: 1.0 + (e.contrast.unwrap_or(1.0) - 1.0) * env,
                saturation: 1.0 + (e.saturation.unwrap_or(1.0) - 1.0) * env,
                exposure: e.exposure.unwrap_or(0.0) * env,
                gamma: 1.0 + (e.gamma.unwrap_or(1.0) - 1.0) * env,
                hue: e.hue.unwrap_or(0.0) * env,
                temperature: e.temperature.unwrap_or(0.0) * env,
                tint: e.tint.as_ref().and_then(|c| parse_color(c).ok()),
                tint_amount: e.tint_amount.unwrap_or(0.25) * env,
            };
            grade(frame, &g);
        }
        "invert" => {
            let amt = e.amount.unwrap_or(1.0) * env;
            invert(frame, amt);
        }
        "rgb_split" | "chromatic" => {
            let base = e.amount.unwrap_or(12.0) * k;
            let a = if e.fade_out.is_none() && e.fade_in.is_none() { base * (1.0 - p * 0.7) } else { base * env };
            rgb_split(frame, a, e.angle.unwrap_or(0.0));
        }
        "glitch" => {
            let i = e.intensity.or(e.amount).unwrap_or(0.7) * env;
            let rate = e.rate.unwrap_or(14.0);
            let step = (t * rate as f64).floor() as u64;
            glitch(frame, i, f.seed.wrapping_add(step.wrapping_mul(7919)), k);
        }
        "blur" => {
            let r = if e.from.is_some() || e.to.is_some() {
                let a = vnum(&e.from).unwrap_or(0.0);
                let b = vnum(&e.to).unwrap_or(20.0);
                lerp(a, b, f.ease(Ease::Linear).apply(p))
            } else {
                e.radius.or(e.amount).unwrap_or(12.0) * env
            };
            blur(frame, r * k);
        }
        "zoom_blur" | "radial_blur" => {
            let a = e.amount.unwrap_or(0.35) * env * if e.fade_in.is_none() && e.fade_out.is_none() { 1.0 - p } else { 1.0 };
            let c = e.center.unwrap_or([0.5, 0.5]);
            zoom_blur(frame, a, c);
        }
        "motion_blur" | "directional_blur" => {
            let a = e.amount.unwrap_or(40.0) * k * env;
            directional_blur(frame, a, e.angle.unwrap_or(0.0));
        }
        "vignette" => vignette(frame, e.amount.unwrap_or(0.45) * env, e.softness.unwrap_or(0.55)),
        "letterbox" => {
            let c = color_or(&e.color, [0, 0, 0, 255]);
            let bar = if let Some(s) = e.size {
                s * ctx.h as f32
            } else {
                let ratio = e.ratio.unwrap_or(2.35);
                ((ctx.h as f32 - ctx.w as f32 / ratio) / 2.0).max(0.0)
            };
            let bar = (bar * Ease::OutCubic.apply(env)).round();
            fill_rect(frame, 0.0, 0.0, ctx.w as f32, bar, c, 1.0);
            fill_rect(frame, 0.0, ctx.h as f32 - bar, ctx.w as f32, ctx.h as f32, c, 1.0);
        }
        "pixelate" => {
            let s = if e.from.is_some() || e.to.is_some() {
                lerp(vnum(&e.from).unwrap_or(1.0), vnum(&e.to).unwrap_or(40.0), p)
            } else {
                e.size.or(e.amount).unwrap_or(24.0) * env.max(0.05)
            };
            pixelate(frame, (s * k).max(1.0) as usize);
        }
        "grain" | "noise" => grain(frame, e.amount.unwrap_or(0.06) * env, ctx.frame.wrapping_add(f.seed)),
        "scanlines" => scanlines(frame, e.amount.unwrap_or(0.25) * env, k),
        "vhs" => {
            let a = e.amount.unwrap_or(1.0) * env;
            vhs(frame, a, t, f.seed, k, ctx.frame);
        }
        "glow" | "bloom" => glow(
            frame,
            e.amount.unwrap_or(0.6) * env,
            e.radius.unwrap_or(30.0) * k,
            e.threshold.unwrap_or(0.6),
            e.color.as_ref().and_then(|c| parse_color(c).ok()),
        ),
        "light_leak" => {
            let cols: Vec<[u8; 4]> = e
                .colors
                .as_ref()
                .map(|v| v.iter().filter_map(|c| parse_color(c).ok()).collect())
                .unwrap_or_else(|| {
                    vec![color_or(&e.color, [255, 120, 40, 255]), [255, 40, 120, 255], [80, 120, 255, 255]]
                });
            light_leak(frame, e.amount.unwrap_or(0.5) * env, t as f32 * e.rate.unwrap_or(0.35), f.seed, &cols);
        }
        "mirror" | "flip" => mirror(frame, e.axis.as_deref().unwrap_or("x")),
        "strobe" => {
            let rate = e.rate.unwrap_or(8.0) as f64;
            if ((t - f.start) * rate * 2.0).floor() as i64 % 2 == 0 {
                let c = color_or(&e.color, [255, 255, 255, 255]);
                mix_color(frame, c, e.opacity.unwrap_or(0.7) * env);
            }
        }
        "duotone" => {
            let cols: Vec<[u8; 4]> = e
                .colors
                .as_ref()
                .map(|v| v.iter().filter_map(|c| parse_color(c).ok()).collect())
                .unwrap_or_default();
            let a = cols.first().copied().unwrap_or([20, 0, 60, 255]);
            let b = cols.get(1).copied().unwrap_or([0, 220, 255, 255]);
            duotone(frame, a, b, e.amount.unwrap_or(1.0) * env);
        }
        "posterize" => posterize(frame, e.amount.unwrap_or(5.0).max(2.0) as u32),
        "sharpen" => sharpen(frame, e.amount.unwrap_or(0.6) * env, k),
        "color_pop" => {
            let c = color_or(&e.color, [230, 30, 40, 255]);
            color_pop(frame, c, e.threshold.unwrap_or(0.12), env);
        }
        _ => {}
    }
}

// ------------------------------------------------------------ implementaciones

#[derive(Clone, Debug)]
pub struct Grade {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub exposure: f32,
    pub gamma: f32,
    pub hue: f32,
    pub temperature: f32,
    pub tint: Option<[u8; 4]>,
    pub tint_amount: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Grade {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            exposure: 0.0,
            gamma: 1.0,
            hue: 0.0,
            temperature: 0.0,
            tint: None,
            tint_amount: 0.0,
        }
    }
}

pub fn grade(f: &mut Frame, g: &Grade) {
    // LUT por canal
    let mut lut = [[0u8; 256]; 3];
    let ex = 2f32.powf(g.exposure);
    let temp = [1.0 + g.temperature * 0.25, 1.0 + g.temperature * 0.03, 1.0 - g.temperature * 0.25];
    for c in 0..3 {
        for i in 0..256 {
            let mut v = i as f32 / 255.0;
            v *= ex * temp[c];
            v += g.brightness;
            v = (v - 0.5) * g.contrast + 0.5;
            v = v.clamp(0.0, 1.0);
            if (g.gamma - 1.0).abs() > 1e-3 {
                v = v.powf(1.0 / g.gamma.max(0.05));
            }
            lut[c][i] = (v * 255.0 + 0.5) as u8;
        }
    }
    // matriz saturación + tono
    let s = g.saturation;
    let (lr, lg, lb) = (0.2126f32, 0.7152f32, 0.0722f32);
    let mut mat = [
        [lr * (1.0 - s) + s, lg * (1.0 - s), lb * (1.0 - s)],
        [lr * (1.0 - s), lg * (1.0 - s) + s, lb * (1.0 - s)],
        [lr * (1.0 - s), lg * (1.0 - s), lb * (1.0 - s) + s],
    ];
    if g.hue.abs() > 0.01 {
        let a = g.hue.to_radians();
        let (sn, cs) = a.sin_cos();
        let k = 1.0 / 3.0f32;
        let sq = (1.0f32 / 3.0).sqrt();
        let h = [
            [cs + (1.0 - cs) * k, k * (1.0 - cs) - sq * sn, k * (1.0 - cs) + sq * sn],
            [k * (1.0 - cs) + sq * sn, cs + k * (1.0 - cs), k * (1.0 - cs) - sq * sn],
            [k * (1.0 - cs) - sq * sn, k * (1.0 - cs) + sq * sn, cs + k * (1.0 - cs)],
        ];
        let mut r = [[0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = (0..3).map(|q| h[i][q] * mat[q][j]).sum();
            }
        }
        mat = r;
    }
    // el tinte también es lineal: final = T · M · lut(entrada)
    let ta = g.tint_amount.clamp(0.0, 1.0);
    if let Some(c) = g.tint.filter(|_| ta > 0.001) {
        let lw = [0.2126f32, 0.7152, 0.0722];
        let mut t = [[0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                let id = if i == j { 1.0 - ta } else { 0.0 };
                t[i][j] = id + c[i] as f32 / 255.0 * 1.6 * ta * lw[j];
            }
        }
        let mut r = [[0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = (0..3).map(|q| t[i][q] * mat[q][j]).sum();
            }
        }
        mat = r;
    }
    let ident = (0..3).all(|i| (0..3).all(|j| (mat[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-4));
    if ident {
        f.data.par_chunks_mut(CHUNK).for_each(|ch| {
            for px in ch.chunks_exact_mut(4) {
                let a = px[3];
                px[0] = lut[0][px[0] as usize].min(a);
                px[1] = lut[1][px[1] as usize].min(a);
                px[2] = lut[2][px[2] as usize].min(a);
            }
        });
        return;
    }
    // tablas: tab[salida][entrada][valor] = M[salida][entrada] · lut[entrada][valor] (punto fijo ×256)
    let mut tab = vec![[[0i32; 256]; 3]; 3];
    for o in 0..3 {
        for i in 0..3 {
            for v in 0..256 {
                tab[o][i][v] = (mat[o][i] * lut[i][v] as f32 * 256.0).round() as i32;
            }
        }
    }
    let tab = &tab;
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            let (r, gg, b) = (px[0] as usize, px[1] as usize, px[2] as usize);
            let a = (px[3] as i32) << 8;
            px[0] = ((tab[0][0][r] + tab[0][1][gg] + tab[0][2][b]).clamp(0, a) >> 8) as u8;
            px[1] = ((tab[1][0][r] + tab[1][1][gg] + tab[1][2][b]).clamp(0, a) >> 8) as u8;
            px[2] = ((tab[2][0][r] + tab[2][1][gg] + tab[2][2][b]).clamp(0, a) >> 8) as u8;
        }
    });
}

pub fn invert(f: &mut Frame, amt: f32) {
    let t = (amt.clamp(0.0, 1.0) * 256.0) as i32;
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            let a = px[3] as i32;
            for c in 0..3 {
                let v = px[c] as i32;
                let inv = a - v;
                px[c] = (v + (((inv - v) * t) >> 8)).clamp(0, 255) as u8;
            }
        }
    });
}

pub fn rgb_split(f: &mut Frame, amount: f32, angle_deg: f32) {
    if amount.abs() < 0.5 {
        return;
    }
    let (s, c) = angle_deg.to_radians().sin_cos();
    let dx = (c * amount).round() as i64;
    let dy = (s * amount).round() as i64;
    let src = f.clone();
    let (w, h) = (f.w as i64, f.h as i64);
    f.data.par_chunks_mut(f.w * 4).enumerate().for_each(|(y, row)| {
        let y = y as i64;
        let yr = (y - dy).clamp(0, h - 1);
        let yb = (y + dy).clamp(0, h - 1);
        for x in 0..w {
            let xr = (x - dx).clamp(0, w - 1);
            let xb = (x + dx).clamp(0, w - 1);
            let ir = ((yr * w + xr) * 4) as usize;
            let ib = ((yb * w + xb) * 4) as usize;
            let o = (x * 4) as usize;
            row[o] = src.data[ir];
            row[o + 2] = src.data[ib + 2];
            row[o + 3] = row[o + 3].max(src.data[ir + 3]).max(src.data[ib + 3]);
        }
    });
}

pub fn glitch(f: &mut Frame, intensity: f32, seed: u64, k: f32) {
    if intensity <= 0.01 {
        return;
    }
    let (w, h) = (f.w, f.h);
    let src = f.clone();
    // franjas desplazadas
    let n = (3.0 + 14.0 * intensity) as u64;
    let mut bands: Vec<(usize, usize, i64, u8)> = Vec::new();
    for i in 0..n {
        if rand01(seed, i * 5) > 0.35 + 0.6 * intensity {
            continue;
        }
        let y0 = (rand01(seed, i * 5 + 1) * h as f32) as usize;
        let bh = ((0.01 + rand01(seed, i * 5 + 2) * 0.09) * h as f32) as usize + 1;
        let off = (rands(seed, i * 5 + 3) * intensity * 0.12 * w as f32) as i64;
        let mode = (rand01(seed, i * 5 + 4) * 4.0) as u8;
        bands.push((y0, (y0 + bh).min(h), off, mode));
    }
    let split = (intensity * 14.0 * k) as i64;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let mut off = 0i64;
        let mut mode = 255u8;
        for b in &bands {
            if y >= b.0 && y < b.1 {
                off = b.2;
                mode = b.3;
            }
        }
        if off == 0 && mode == 255 {
            return;
        }
        let srow = &src.data[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let sx = (x as i64 - off).rem_euclid(w as i64) as usize;
            let o = x * 4;
            let i = sx * 4;
            match mode {
                0 => {
                    // canal rojo desplazado extra
                    let ir = ((sx as i64 - split).rem_euclid(w as i64) as usize) * 4;
                    row[o] = srow[ir];
                    row[o + 1] = srow[i + 1];
                    row[o + 2] = srow[i + 2];
                }
                1 => {
                    // canales intercambiados
                    row[o] = srow[i + 2];
                    row[o + 1] = srow[i];
                    row[o + 2] = srow[i + 1];
                }
                2 => {
                    // brillo quemado
                    row[o] = srow[i].saturating_add(60);
                    row[o + 1] = srow[i + 1].saturating_add(60);
                    row[o + 2] = srow[i + 2].saturating_add(60);
                }
                _ => {
                    row[o..o + 3].copy_from_slice(&srow[i..i + 3]);
                }
            }
            row[o + 3] = srow[i + 3];
        }
    });
    rgb_split(f, intensity * 10.0 * k * (0.5 + rand01(seed, 999)), 0.0);
    // bloques corruptos
    let nb = (intensity * 6.0) as u64;
    for i in 0..nb {
        let bw = ((0.04 + rand01(seed, 2000 + i) * 0.18) * w as f32) as usize;
        let bh = ((0.02 + rand01(seed, 3000 + i) * 0.08) * h as f32) as usize;
        let x0 = (rand01(seed, 4000 + i) * (w.saturating_sub(bw)) as f32) as usize;
        let y0 = (rand01(seed, 5000 + i) * (h.saturating_sub(bh)) as f32) as usize;
        let sx0 = (rand01(seed, 6000 + i) * (w.saturating_sub(bw)) as f32) as usize;
        let sy0 = (rand01(seed, 7000 + i) * (h.saturating_sub(bh)) as f32) as usize;
        for yy in 0..bh {
            let d0 = ((y0 + yy) * w + x0) * 4;
            let s0 = ((sy0 + yy) * w + sx0) * 4;
            f.data[d0..d0 + bw * 4].copy_from_slice(&src.data[s0..s0 + bw * 4]);
        }
    }
}

pub fn zoom_blur(f: &mut Frame, amount: f32, center: [f32; 2]) {
    if amount <= 0.005 {
        return;
    }
    if amount > 0.06 && f.w > 64 {
        return at_half(f, |s| zoom_blur_full(s, amount, center));
    }
    zoom_blur_full(f, amount, center)
}

fn zoom_blur_full(f: &mut Frame, amount: f32, center: [f32; 2]) {
    let src = f.clone();
    let n = ((amount * 40.0) as usize).clamp(4, 18);
    let (cx, cy) = (center[0] * f.w as f32, center[1] * f.h as f32);
    let spread = amount * 0.35;
    let w = f.w;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let yf = y as f32 + 0.5;
        for x in 0..w {
            let xf = x as f32 + 0.5;
            let mut acc = [0f32; 4];
            for i in 0..n {
                let s = 1.0 - spread * (i as f32 / (n - 1) as f32);
                let p = sample(&src, cx + (xf - cx) * s, cy + (yf - cy) * s, Edge::Clamp);
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            for c in 0..4 {
                row[x * 4 + c] = (acc[c] / n as f32) as u8;
            }
        }
    });
}

pub fn directional_blur(f: &mut Frame, amount: f32, angle_deg: f32) {
    if amount < 1.0 {
        return;
    }
    if amount > 24.0 && f.w > 64 {
        return at_half(f, |s| directional_blur(s, amount / 2.0, angle_deg));
    }
    let (s, c) = angle_deg.to_radians().sin_cos();
    let n = ((amount / 3.0) as usize).clamp(4, 24);
    let src = f.clone();
    let w = f.w;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let yf = y as f32 + 0.5;
        for x in 0..w {
            let xf = x as f32 + 0.5;
            let mut acc = [0f32; 4];
            for i in 0..n {
                let o = (i as f32 / (n - 1) as f32 - 0.5) * amount;
                let p = sample(&src, xf + c * o, yf + s * o, Edge::Clamp);
                for k in 0..4 {
                    acc[k] += p[k];
                }
            }
            for k in 0..4 {
                row[x * 4 + k] = (acc[k] / n as f32) as u8;
            }
        }
    });
}

type VigKey = (usize, usize, u32);
static VIG_CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<(VigKey, std::sync::Arc<Vec<u8>>)>>> = std::sync::OnceLock::new();

/// Máscara de forma de la viñeta (0 = sin oscurecer, 255 = máximo), cacheada por tamaño/suavidad.
fn vignette_mask(w: usize, h: usize, softness: f32) -> std::sync::Arc<Vec<u8>> {
    let key = (w, h, (softness * 1000.0) as u32);
    let cache = VIG_CACHE.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    if let Some((_, m)) = cache.lock().unwrap().iter().find(|(k, _)| *k == key) {
        return m.clone();
    }
    let inner = (1.0 - softness).clamp(0.0, 0.95) * 0.75;
    let mut m = vec![0u8; w * h];
    m.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let ny = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
        for (x, v) in row.iter_mut().enumerate() {
            let nx = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
            let d = (nx * nx * 0.75 + ny * ny * 0.95).sqrt();
            *v = (smoothstep(inner, 1.25, d) * 255.0) as u8;
        }
    });
    let m = std::sync::Arc::new(m);
    let mut c = cache.lock().unwrap();
    if c.len() > 8 {
        c.clear();
    }
    c.push((key, m.clone()));
    m
}

pub fn vignette(f: &mut Frame, amount: f32, softness: f32) {
    if amount <= 0.001 {
        return;
    }
    let mask = vignette_mask(f.w, f.h, softness);
    let a = (amount.clamp(0.0, 1.0) * 256.0) as u32;
    let w = f.w;
    f.data.par_chunks_mut(w * 4 * 8).zip(mask.par_chunks(w * 8)).for_each(|(rows, m)| {
        for (px, &mv) in rows.chunks_exact_mut(4).zip(m.iter()) {
            if mv == 0 {
                continue;
            }
            // factor = 1 - amount * m
            let k = 65536 - ((a * mv as u32) << 8) / 255;
            px[0] = ((px[0] as u32 * k) >> 16) as u8;
            px[1] = ((px[1] as u32 * k) >> 16) as u8;
            px[2] = ((px[2] as u32 * k) >> 16) as u8;
        }
    });
}

pub fn pixelate(f: &mut Frame, size: usize) {
    if size <= 1 {
        return;
    }
    let (w, h) = (f.w, f.h);
    let bx = (w + size - 1) / size;
    let by = (h + size - 1) / size;
    let mut avg = vec![[0u8; 4]; bx * by];
    avg.par_chunks_mut(bx).enumerate().for_each(|(j, arow)| {
        for i in 0..bx {
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for y in j * size..((j + 1) * size).min(h) {
                for x in i * size..((i + 1) * size).min(w) {
                    let p = f.px(x, y);
                    for c in 0..4 {
                        acc[c] += p[c] as u32;
                    }
                    n += 1;
                }
            }
            for c in 0..4 {
                arow[i][c] = (acc[c] / n.max(1)) as u8;
            }
        }
    });
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let j = y / size;
        for x in 0..w {
            row[x * 4..x * 4 + 4].copy_from_slice(&avg[j * bx + x / size]);
        }
    });
}

pub fn grain(f: &mut Frame, amount: f32, seed: u64) {
    if amount <= 0.001 {
        return;
    }
    // textura de ruido de 512x512 desplazada al azar en cada frame
    static TILE: std::sync::OnceLock<Vec<i16>> = std::sync::OnceLock::new();
    let tile = TILE.get_or_init(|| (0..512 * 512u64).map(|i| ((rand01(0x5EED, i) - 0.5) * 2048.0) as i16).collect());
    let amp = (amount * 255.0 * 2.0) as i32;
    let ox = (hash64(seed) % 512) as usize;
    let oy = (hash64(seed ^ 0xABCDEF) % 512) as usize;
    let w = f.w;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let trow = &tile[((y + oy) & 511) * 512..((y + oy) & 511) * 512 + 512];
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let n = (trow[(x + ox) & 511] as i32 * amp) >> 11;
            let a = px[3] as i32;
            px[0] = (px[0] as i32 + n).clamp(0, a) as u8;
            px[1] = (px[1] as i32 + n).clamp(0, a) as u8;
            px[2] = (px[2] as i32 + n).clamp(0, a) as u8;
        }
    });
}

pub fn scanlines(f: &mut Frame, amount: f32, k: f32) {
    let period = (3.0 * k).max(2.0) as usize;
    let m = 1.0 - amount.clamp(0.0, 1.0);
    let w = f.w;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        if y % period == 0 {
            for px in row.chunks_exact_mut(4) {
                for c in 0..3 {
                    px[c] = (px[c] as f32 * m) as u8;
                }
            }
        }
    });
}

pub fn vhs(f: &mut Frame, a: f32, t: f64, seed: u64, k: f32, frame: u64) {
    // ondulación horizontal por filas
    let src = f.clone();
    let w = f.w;
    let tt = t as f32;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let yf = y as f32;
        let off = ((yf * 0.05 + tt * 3.0).sin() * 1.5 + noise1(seed, yf * 0.02 + tt * 8.0) * 2.0) * k * a;
        for x in 0..w {
            let p = sample(&src, x as f32 + 0.5 + off, yf + 0.5, Edge::Clamp);
            for c in 0..4 {
                row[x * 4 + c] = p[c] as u8;
            }
        }
    });
    rgb_split(f, 4.0 * k * a, 0.0);
    let g = Grade { saturation: 1.0 - 0.25 * a, contrast: 1.0 - 0.08 * a, brightness: 0.02 * a, ..Grade::default() };
    grade(f, &g);
    scanlines(f, 0.18 * a, k);
    grain(f, 0.05 * a, frame);
}

pub fn glow(f: &mut Frame, amount: f32, radius: f32, threshold: f32, color: Option<[u8; 4]>) {
    if amount <= 0.001 {
        return;
    }
    let factor = 4usize;
    let mut small = downscale(f, factor);
    let th = threshold.clamp(0.0, 0.99);
    for px in small.data.chunks_exact_mut(4) {
        let l = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
        let m = ((l - th) / (1.0 - th)).clamp(0.0, 1.0);
        if let Some(c) = color {
            px[0] = (c[0] as f32 * m) as u8;
            px[1] = (c[1] as f32 * m) as u8;
            px[2] = (c[2] as f32 * m) as u8;
        } else {
            for c in 0..3 {
                px[c] = (px[c] as f32 * m) as u8;
            }
        }
    }
    blur(&mut small, (radius / factor as f32).max(1.0));
    let sx = small.w as f32 / f.w as f32;
    let sy = small.h as f32 / f.h as f32;
    let w = f.w;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 + 0.5) * sy;
        for x in 0..w {
            let p = sample(&small, (x as f32 + 0.5) * sx, fy, Edge::Clamp);
            let o = x * 4;
            for c in 0..3 {
                let d = row[o + c] as f32;
                let s = p[c] * amount * 1.5;
                row[o + c] = (d + s - d * s / 255.0).clamp(0.0, 255.0) as u8;
            }
        }
    });
}

pub fn light_leak(f: &mut Frame, amount: f32, t: f32, seed: u64, cols: &[[u8; 4]]) {
    if amount <= 0.001 || cols.is_empty() {
        return;
    }
    let (w, h) = (f.w as f32, f.h as f32);
    let blobs: Vec<(f32, f32, f32, [f32; 3])> = (0..cols.len().max(2))
        .map(|i| {
            let i64_ = i as u64;
            let bx = 0.5 + 0.6 * noise1(seed + i64_ * 31, t + i as f32 * 3.7);
            let by = 0.5 + 0.6 * noise1(seed + i64_ * 57, t * 0.8 + i as f32 * 1.3);
            let r = 0.35 + 0.25 * (noise1(seed + i64_ * 91, t * 0.6) * 0.5 + 0.5);
            let c = cols[i % cols.len()];
            (bx * w, by * h, r * w.max(h), [c[0] as f32, c[1] as f32, c[2] as f32])
        })
        .collect();
    f.data.par_chunks_mut(f.w * 4).enumerate().for_each(|(y, row)| {
        let yf = y as f32;
        for x in 0..row.len() / 4 {
            let xf = x as f32;
            let mut add = [0f32; 3];
            for b in &blobs {
                let dx = xf - b.0;
                let dy = yf - b.1;
                let d2 = (dx * dx + dy * dy) / (b.2 * b.2);
                if d2 < 1.0 {
                    let v = (1.0 - d2).powi(2) * amount;
                    for c in 0..3 {
                        add[c] += b.3[c] * v;
                    }
                }
            }
            let o = x * 4;
            for c in 0..3 {
                let d = row[o + c] as f32;
                let s = add[c].min(255.0);
                row[o + c] = (d + s - d * s / 255.0) as u8;
            }
        }
    });
}

pub fn mirror(f: &mut Frame, axis: &str) {
    let w = f.w;
    let h = f.h;
    if axis == "y" {
        let src = f.clone();
        f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            let sy = h - 1 - y;
            row.copy_from_slice(&src.data[sy * w * 4..(sy + 1) * w * 4]);
        });
    } else {
        f.data.par_chunks_mut(w * 4).for_each(|row| {
            for x in 0..w / 2 {
                for c in 0..4 {
                    row.swap(x * 4 + c, (w - 1 - x) * 4 + c);
                }
            }
        });
    }
}

pub fn duotone(f: &mut Frame, a: [u8; 4], b: [u8; 4], amt: f32) {
    let amt = amt.clamp(0.0, 1.0);
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            let l = (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32) / 255.0;
            for c in 0..3 {
                let v = lerp(a[c] as f32, b[c] as f32, l);
                px[c] = lerp(px[c] as f32, v, amt) as u8;
            }
        }
    });
}

pub fn posterize(f: &mut Frame, levels: u32) {
    let n = (levels - 1) as f32;
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = ((i as f32 / 255.0 * n).round() / n * 255.0) as u8;
    }
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            for c in 0..3 {
                px[c] = lut[px[c] as usize];
            }
        }
    });
}

pub fn sharpen(f: &mut Frame, amount: f32, k: f32) {
    let mut b = f.clone();
    blur(&mut b, 1.6 * k.max(0.5));
    f.data.par_chunks_mut(CHUNK).zip(b.data.par_chunks(CHUNK)).for_each(|(d, s)| {
        for i in 0..d.len() {
            if i % 4 == 3 {
                continue;
            }
            let v = d[i] as f32 + (d[i] as f32 - s[i] as f32) * amount;
            d[i] = v.clamp(0.0, 255.0) as u8;
        }
    });
}

fn hue_of(r: f32, g: f32, b: f32) -> (f32, f32) {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    if d < 1e-5 {
        return (0.0, 0.0);
    }
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, d / mx.max(1e-5))
}

pub fn color_pop(f: &mut Frame, c: [u8; 4], threshold: f32, env: f32) {
    let (kh, _) = hue_of(c[0] as f32, c[1] as f32, c[2] as f32);
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
            let (h, s) = hue_of(r, g, b);
            let mut dh = (h - kh).abs();
            dh = dh.min(1.0 - dh);
            let keep = (1.0 - smoothstep(threshold, threshold * 1.8, dh)) * smoothstep(0.15, 0.35, s);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let m = (1.0 - keep) * env;
            px[0] = lerp(r, l, m) as u8;
            px[1] = lerp(g, l, m) as u8;
            px[2] = lerp(b, l, m) as u8;
        }
    });
}

/// Elimina un color (croma) y devuelve alfa premultiplicado.
pub fn chroma_key(f: &mut Frame, key: [u8; 4], similarity: f32, smoothness: f32) {
    let ycc = |r: f32, g: f32, b: f32| -> (f32, f32) {
        (-0.1146 * r - 0.3854 * g + 0.5 * b, 0.5 * r - 0.4542 * g - 0.0458 * b)
    };
    let (kcb, kcr) = ycc(key[0] as f32, key[1] as f32, key[2] as f32);
    let green = key[1] > key[0] && key[1] > key[2];
    let blue = key[2] > key[0] && key[2] > key[1];
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            let a0 = px[3] as f32 / 255.0;
            if a0 <= 0.0 {
                continue;
            }
            let (r, g, b) = (px[0] as f32 / a0, px[1] as f32 / a0, px[2] as f32 / a0);
            let (cb, cr) = ycc(r, g, b);
            let d = ((cb - kcb).powi(2) + (cr - kcr).powi(2)).sqrt() / 255.0;
            let a = smoothstep(similarity, similarity + smoothness.max(0.001), d);
            let (r2, mut g2, mut b2) = (r, g, b);
            if green {
                g2 = g2.min((r2 + b2) * 0.5 + 10.0);
            } else if blue {
                b2 = b2.min((r2 + g2) * 0.5 + 10.0);
            }
            let fa = a * a0;
            px[0] = (r2 * fa).clamp(0.0, 255.0) as u8;
            px[1] = (g2 * fa).clamp(0.0, 255.0) as u8;
            px[2] = (b2 * fa).clamp(0.0, 255.0) as u8;
            px[3] = (fa * 255.0) as u8;
        }
    });
}

pub fn opaque_over(f: &mut Frame, bg: [u8; 4]) {
    let b = premul([bg[0], bg[1], bg[2], 255]);
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a < 255 {
                let ia = 255 - a;
                for c in 0..3 {
                    px[c] = (px[c] as u32 + (b[c] as u32 * ia + 127) / 255).min(255) as u8;
                }
                px[3] = 255;
            }
        }
    });
}
