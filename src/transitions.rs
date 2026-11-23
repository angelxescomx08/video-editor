//! Transiciones entre dos clips. `p` va de 0 a 1 y el corte está en p = 0.5.

use crate::ease::Ease;
use crate::frame::*;
use crate::fx::{self, Ctx};
use crate::util::*;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct TTransition {
    pub kind: String,
    pub duration: f64,
    pub color: [u8; 4],
    pub direction: String,
    pub ease: Option<Ease>,
    pub intensity: f32,
    pub seed: u64,
}

fn dir_vec(d: &str) -> (f32, f32) {
    match d {
        "right" => (1.0, 0.0),
        "up" => (0.0, -1.0),
        "down" => (0.0, 1.0),
        _ => (-1.0, 0.0),
    }
}

/// Combina a (saliente) y b (entrante).
pub fn apply(tr: &TTransition, a: Frame, b: Frame, p: f32, ctx: &Ctx) -> Frame {
    let p = p.clamp(0.0, 1.0);
    let (w, h) = (ctx.w as f32, ctx.h as f32);
    let it = tr.intensity;
    match tr.kind.as_str() {
        "fade" | "crossfade" | "dissolve" => {
            let mut out = Frame::new(a.w, a.h);
            mix(&a, &b, tr.ease.unwrap_or(Ease::Linear).apply(p), &mut out);
            out
        }
        "fade_black" | "fade_color" | "dip" => {
            let c = if tr.kind == "fade_black" { [0, 0, 0, 255] } else { tr.color };
            if p < 0.5 {
                let mut f = a;
                mix_color(&mut f, c, Ease::InOutQuad.apply(p * 2.0));
                f
            } else {
                let mut f = b;
                mix_color(&mut f, c, Ease::InOutQuad.apply((1.0 - p) * 2.0));
                f
            }
        }
        "flash" | "flash_white" => {
            let c = tr.color;
            let amt = 1.0 - ((p - 0.5).abs() * 2.0);
            let mut f = if p < 0.5 { a } else { b };
            mix_color(&mut f, c, Ease::InQuad.apply(amt).min(1.0));
            f
        }
        "zoom_in" | "zoom" => {
            let mut f;
            let blur_amt = (1.0 - (p - 0.5).abs() * 2.0).powf(1.5) * 0.6 * it;
            if p < 0.5 {
                let q = Ease::InExpo.apply(p * 2.0);
                let s = 1.0 + 2.2 * q * it;
                f = Frame::new(a.w, a.h);
                warp(&a, &mut f, &Affine::about((w / 2.0, h / 2.0), &Affine::scale(s, s)), Edge::Mirror);
            } else {
                let q = Ease::OutExpo.apply((p - 0.5) * 2.0);
                let s = lerp(0.45, 1.0, q);
                f = Frame::new(b.w, b.h);
                warp(&b, &mut f, &Affine::about((w / 2.0, h / 2.0), &Affine::scale(s, s)), Edge::Mirror);
            }
            fx::zoom_blur(&mut f, blur_amt, [0.5, 0.5]);
            f
        }
        "zoom_out" => {
            let mut f;
            let blur_amt = (1.0 - (p - 0.5).abs() * 2.0).powf(1.5) * 0.6 * it;
            if p < 0.5 {
                let q = Ease::InExpo.apply(p * 2.0);
                let s = lerp(1.0, 0.45, q);
                f = Frame::new(a.w, a.h);
                warp(&a, &mut f, &Affine::about((w / 2.0, h / 2.0), &Affine::scale(s, s)), Edge::Mirror);
            } else {
                let q = Ease::OutExpo.apply((p - 0.5) * 2.0);
                let s = lerp(3.2, 1.0, q);
                f = Frame::new(b.w, b.h);
                warp(&b, &mut f, &Affine::about((w / 2.0, h / 2.0), &Affine::scale(s, s)), Edge::Mirror);
            }
            fx::zoom_blur(&mut f, blur_amt, [0.5, 0.5]);
            f
        }
        "whip" | "slide" | "push" => {
            let (dx, dy) = dir_vec(&tr.direction);
            let ease = tr.ease.unwrap_or(if tr.kind == "whip" { Ease::InOutExpo } else { Ease::InOutCubic });
            let o = ease.apply(p);
            let blur = if tr.kind == "whip" { (std::f32::consts::PI * p).sin().powi(2) * 0.22 * it } else { 0.0 };
            if blur * a.w as f32 > 40.0 {
                // desenfoque grande: a media resolución
                let sa = downscale(&a, 2);
                let sb = downscale(&b, 2);
                let small = strip_blur(&sa, &sb, o, dx, dy, blur);
                let mut out = Frame::new(a.w, a.h);
                upscale_into(&small, &mut out);
                out
            } else {
                strip_blur(&a, &b, o, dx, dy, blur)
            }
        }
        "spin" => {
            let mut f;
            let ang;
            let s;
            if p < 0.5 {
                let q = Ease::InCubic.apply(p * 2.0);
                ang = q * 180.0 * it;
                s = 1.0 + q * 0.8;
                f = a;
            } else {
                let q = Ease::OutCubic.apply((p - 0.5) * 2.0);
                ang = -(1.0 - q) * 180.0 * it;
                s = 1.0 + (1.0 - q) * 0.8;
                f = b;
            }
            let m = Affine::about((w / 2.0, h / 2.0), &Affine::rotate(ang.to_radians()).then(&Affine::scale(s, s)));
            let mut out = Frame::new(f.w, f.h);
            warp(&f, &mut out, &m, Edge::Mirror);
            let peak = 1.0 - (p - 0.5).abs() * 2.0;
            spin_blur(&mut out, peak * 14.0 * it);
            f = out;
            f
        }
        "glitch" => {
            let peak = 1.0 - (p - 0.5).abs() * 2.0;
            let step = (p * 14.0) as u64;
            let pick_b = if p < 0.35 {
                false
            } else if p > 0.65 {
                true
            } else {
                rand01(tr.seed, step) < (p - 0.35) / 0.3
            };
            let mut f = if pick_b { b } else { a };
            fx::glitch(&mut f, (peak * 1.2 * it).min(1.0), tr.seed.wrapping_add(step * 101), ctx.k);
            f
        }
        "wipe" => {
            let (dx, dy) = dir_vec(&tr.direction);
            let q = tr.ease.unwrap_or(Ease::InOutCubic).apply(p);
            let soft = 0.04f32;
            let mut out = b;
            let wi = out.w;
            out.data.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
                let ny = (y as f32 + 0.5) / h;
                for x in 0..wi {
                    let nx = (x as f32 + 0.5) / w;
                    // coordenada a lo largo de la dirección de avance (0 en el borde de entrada)
                    let s = if dx < 0.0 {
                        1.0 - nx
                    } else if dx > 0.0 {
                        nx
                    } else if dy < 0.0 {
                        1.0 - ny
                    } else {
                        ny
                    };
                    // m = 0 -> b (zona ya barrida, cerca del borde de entrada)
                    let m = smoothstep(q * (1.0 + soft) - soft, q * (1.0 + soft), s);
                    let i = (y * wi + x) * 4;
                    for c in 0..4 {
                        let av = a.data[i + c] as f32;
                        let bv = row[x * 4 + c] as f32;
                        row[x * 4 + c] = lerp(av, bv, 1.0 - m) as u8;
                    }
                }
            });
            out
        }
        "circle" | "iris" => {
            let q = tr.ease.unwrap_or(Ease::InOutCubic).apply(p);
            let maxr = (w * w + h * h).sqrt() / 2.0;
            let r = q * maxr;
            let soft = 0.02 * maxr;
            let mut out = b;
            let wi = out.w;
            out.data.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
                let dy = y as f32 + 0.5 - h / 2.0;
                for x in 0..wi {
                    let dx = x as f32 + 0.5 - w / 2.0;
                    let d = (dx * dx + dy * dy).sqrt();
                    let m = smoothstep(r - soft, r + soft, d); // 1 fuera -> a
                    if m > 0.0 {
                        let i = (y * wi + x) * 4;
                        for c in 0..4 {
                            let bv = row[x * 4 + c] as f32;
                            row[x * 4 + c] = lerp(bv, a.data[i + c] as f32, m) as u8;
                        }
                    }
                }
            });
            out
        }
        "pixelate" => {
            let peak = 1.0 - (p - 0.5).abs() * 2.0;
            let mut f = if p < 0.5 { a } else { b };
            let size = (1.0 + peak.powi(2) * 60.0 * it * ctx.k) as usize;
            fx::pixelate(&mut f, size);
            f
        }
        "blur" => {
            let peak = 1.0 - (p - 0.5).abs() * 2.0;
            let mut out = Frame::new(a.w, a.h);
            let m = smoothstep(0.35, 0.65, p);
            mix(&a, &b, m, &mut out);
            blur(&mut out, peak * 40.0 * it * ctx.k);
            out
        }
        "shake" => {
            let mut f = if p < 0.5 { a } else { b };
            let peak = (1.0 - (p - 0.5).abs() * 2.0).powi(2);
            let amp = peak * 60.0 * ctx.k * it;
            let tt = p * 40.0;
            let dx = noise1(tr.seed, tt) * amp;
            let dy = noise1(tr.seed + 3, tt) * amp;
            let r = noise1(tr.seed + 5, tt) * peak * 3.0;
            let s = 1.0 + peak * 0.12;
            let m = Affine::about((w / 2.0, h / 2.0), &Affine::scale(s, s).then(&Affine::rotate(r.to_radians())))
                .then(&Affine::translate(dx, dy));
            warp_in_place(&mut f, &m, Edge::Mirror);
            fx::directional_blur(&mut f, amp * 0.6, (dy.atan2(dx)).to_degrees());
            f
        }
        "rgb" | "chromatic" => {
            let peak = 1.0 - (p - 0.5).abs() * 2.0;
            let mut out = Frame::new(a.w, a.h);
            mix(&a, &b, smoothstep(0.4, 0.6, p), &mut out);
            fx::rgb_split(&mut out, peak * 45.0 * ctx.k * it, 0.0);
            let s = 1.0 + peak * 0.08;
            warp_in_place(&mut out, &Affine::about((w / 2.0, h / 2.0), &Affine::scale(s, s)), Edge::Mirror);
            out
        }
        "stretch" | "warp" => {
            let peak = 1.0 - (p - 0.5).abs() * 2.0;
            let mut f = if p < 0.5 { a } else { b };
            let sx = 1.0 + peak.powi(2) * 3.0 * it;
            let sy = 1.0 - peak.powi(2) * 0.15;
            warp_in_place(&mut f, &Affine::about((w / 2.0, h / 2.0), &Affine::scale(sx, sy)), Edge::Mirror);
            fx::directional_blur(&mut f, peak * 80.0 * ctx.k, 0.0);
            f
        }
        "luma" | "luma_fade" => {
            let q = tr.ease.unwrap_or(Ease::InOutQuad).apply(p);
            let soft = 0.12f32;
            let mut out = b;
            out.data.par_chunks_mut(CHUNK).zip(a.data.par_chunks(CHUNK)).for_each(|(o, av)| {
                for (po, pa) in o.chunks_exact_mut(4).zip(av.chunks_exact(4)) {
                    let l = (0.2126 * pa[0] as f32 + 0.7152 * pa[1] as f32 + 0.0722 * pa[2] as f32) / 255.0;
                    // los oscuros desaparecen primero
                    let m = smoothstep(q * (1.0 + soft) - soft, q * (1.0 + soft), l);
                    for c in 0..4 {
                        po[c] = lerp(po[c] as f32, pa[c] as f32, m) as u8;
                    }
                }
            });
            out
        }
        // corte seco y desconocidos
        _ => {
            if p < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// a y b colocados uno tras otro en la dirección (dx,dy); `o` = 0..1 desplazamiento; blur en fracción de lienzo.
fn strip_blur(a: &Frame, b: &Frame, o: f32, dx: f32, dy: f32, blur: f32) -> Frame {
    let (w, h) = (a.w as f32, a.h as f32);
    let mut out = Frame::new(a.w, a.h);
    // el contenido se mueve en dirección (dx,dy): a sale, b entra desde el lado opuesto.
    let horiz = dx != 0.0;
    let len = if horiz { w } else { h };
    let sign = if horiz { -dx } else { -dy }; // +1: avanzamos hacia +x en la tira
    let shift = o * len;
    let n = if blur > 0.002 { ((blur * len / 6.0) as usize).clamp(3, 16) } else { 1 };
    let blen = blur * len;
    let wi = a.w;
    out.data.par_chunks_mut(wi * 4).enumerate().for_each(|(y, row)| {
        let yf = y as f32 + 0.5;
        for x in 0..wi {
            let xf = x as f32 + 0.5;
            let mut acc = [0f32; 4];
            for i in 0..n {
                let off = if n > 1 { (i as f32 / (n - 1) as f32 - 0.5) * blen } else { 0.0 };
                // coordenada en la tira (a en [0,len), b en [len, 2len))
                let along = if horiz { xf } else { yf };
                let s = if sign > 0.0 { along + shift + off } else { len - along + shift + off };
                let (src, local) = if s < len { (a, s) } else { (b, s - len) };
                let local = if sign > 0.0 { local } else { len - local };
                let (sx, sy) = if horiz { (local, yf) } else { (xf, local) };
                let p = sample(src, sx.clamp(0.0, w - 0.01), sy.clamp(0.0, h - 0.01), Edge::Clamp);
                for c in 0..4 {
                    acc[c] += p[c];
                }
            }
            for c in 0..4 {
                row[x * 4 + c] = (acc[c] / n as f32) as u8;
            }
        }
    });
    out
}

/// Desenfoque rotacional alrededor del centro (ángulo total en grados).
pub fn spin_blur(f: &mut Frame, degrees: f32) {
    if degrees.abs() < 0.3 {
        return;
    }
    if degrees.abs() > 3.0 && f.w > 64 {
        return at_half(f, |s| spin_blur_full(s, degrees));
    }
    spin_blur_full(f, degrees)
}

fn spin_blur_full(f: &mut Frame, degrees: f32) {
    let src = f.clone();
    let (cx, cy) = (f.w as f32 / 2.0, f.h as f32 / 2.0);
    let n = ((degrees.abs() * 1.2) as usize).clamp(4, 20);
    let rots: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let a = ((i as f32 / (n - 1) as f32) - 0.5) * degrees.to_radians();
            a.sin_cos()
        })
        .collect();
    let w = f.w;
    f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let dy = y as f32 + 0.5 - cy;
        for x in 0..w {
            let dx = x as f32 + 0.5 - cx;
            let mut acc = [0f32; 4];
            for &(s, c) in &rots {
                let p = sample(&src, cx + dx * c - dy * s, cy + dx * s + dy * c, Edge::Mirror);
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
