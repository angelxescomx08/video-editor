//! Búfer de imagen RGBA8 premultiplicado y primitivas de composición.

use rayon::prelude::*;

/// Tama�o de bloque para el trabajo en paralelo (m�ltiplo de 4 bytes).
pub const CHUNK: usize = 64 * 1024;

#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Edge {
    Clamp,
    Transparent,
    Mirror,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Blend {
    Normal,
    Add,
    Screen,
    Multiply,
    Overlay,
}

impl Blend {
    pub fn parse(s: Option<&str>) -> Blend {
        match s.unwrap_or("normal") {
            "add" | "additive" => Blend::Add,
            "screen" => Blend::Screen,
            "multiply" => Blend::Multiply,
            "overlay" => Blend::Overlay,
            _ => Blend::Normal,
        }
    }
}

#[inline]
pub fn premul(c: [u8; 4]) -> [u8; 4] {
    let a = c[3] as u32;
    [
        ((c[0] as u32 * a + 127) / 255) as u8,
        ((c[1] as u32 * a + 127) / 255) as u8,
        ((c[2] as u32 * a + 127) / 255) as u8,
        c[3],
    ]
}

impl Frame {
    pub fn new(w: usize, h: usize) -> Frame {
        Frame { w, h, data: vec![0; w * h * 4] }
    }

    pub fn filled(w: usize, h: usize, c: [u8; 4]) -> Frame {
        let mut f = Frame::new(w, h);
        f.fill(c);
        f
    }

    pub fn fill(&mut self, c: [u8; 4]) {
        let p = premul(c);
        self.data.par_chunks_mut(CHUNK).for_each(|ch| {
            for px in ch.chunks_exact_mut(4) {
                px.copy_from_slice(&p);
            }
        });
    }

    pub fn from_rgba_straight(w: usize, h: usize, mut data: Vec<u8>) -> Frame {
        data.par_chunks_mut(CHUNK).for_each(|ch| {
            for px in ch.chunks_exact_mut(4) {
                let a = px[3] as u32;
                if a < 255 {
                    px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
                    px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
                    px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
                }
            }
        });
        Frame { w, h, data }
    }

    /// Convierte a RGBA recto (para guardar PNG con transparencia).
    pub fn to_straight(&self) -> Vec<u8> {
        let mut d = self.data.clone();
        for px in d.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a > 0 && a < 255 {
                px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
                px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
                px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
        d
    }

    /// Convierte a YUV 4:2:0 planar (BT.709, rango limitado). w y h deben ser pares.
    pub fn to_yuv420(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.w, self.h);
        let ysz = w * h;
        let csz = (w / 2) * (h / 2);
        out.resize(ysz + 2 * csz, 0);
        let (yp, cp) = out.split_at_mut(ysz);
        let (up, vp) = cp.split_at_mut(csz);
        let cw = w / 2;
        yp.par_chunks_mut(w * 2)
            .zip(up.par_chunks_mut(cw))
            .zip(vp.par_chunks_mut(cw))
            .enumerate()
            .for_each(|(j, ((yrows, urow), vrow))| {
                let y0 = j * 2;
                for x2 in 0..cw {
                    let mut sr = 0i32;
                    let mut sg = 0i32;
                    let mut sb = 0i32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let x = x2 * 2 + dx;
                            let i = ((y0 + dy) * w + x) * 4;
                            let r = self.data[i] as i32;
                            let g = self.data[i + 1] as i32;
                            let b = self.data[i + 2] as i32;
                            yrows[dy * w + x] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
                            sr += r;
                            sg += g;
                            sb += b;
                        }
                    }
                    let (r, g, b) = (sr >> 2, sg >> 2, sb >> 2);
                    urow[x2] = (((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
                    vrow[x2] = (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
                }
            });
    }

    /// Desde YUV 4:2:0 planar (BT.709, rango limitado) a RGBA opaco.
    pub fn from_yuv420(w: usize, h: usize, src: &[u8]) -> Frame {
        let mut f = Frame::new(w, h);
        let ysz = w * h;
        let cw = w / 2;
        let csz = cw * (h / 2);
        let (yp, cp) = src.split_at(ysz);
        let (up, vp) = cp.split_at(csz);
        f.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            let yr = &yp[y * w..(y + 1) * w];
            let ur = &up[(y / 2) * cw..(y / 2 + 1) * cw];
            let vr = &vp[(y / 2) * cw..(y / 2 + 1) * cw];
            for x in 0..w {
                let c = 298 * (yr[x] as i32 - 16);
                let d = ur[x / 2] as i32 - 128;
                let e = vr[x / 2] as i32 - 128;
                let o = x * 4;
                row[o] = ((c + 459 * e + 128) >> 8).clamp(0, 255) as u8;
                row[o + 1] = ((c - 55 * d - 136 * e + 128) >> 8).clamp(0, 255) as u8;
                row[o + 2] = ((c + 541 * d + 128) >> 8).clamp(0, 255) as u8;
                row[o + 3] = 255;
            }
        });
        f
    }

    #[inline]
    pub fn px(&self, x: usize, y: usize) -> &[u8] {
        let i = (y * self.w + x) * 4;
        &self.data[i..i + 4]
    }

    pub fn encode_jpeg(&self, quality: u8) -> anyhow::Result<Vec<u8>> {
        let mut rgb = Vec::with_capacity(self.w * self.h * 3);
        for p in self.data.chunks_exact(4) {
            rgb.extend_from_slice(&p[..3]);
        }
        let mut out = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
        enc.encode(&rgb, self.w as u32, self.h as u32, image::ExtendedColorType::Rgb8)?;
        Ok(out)
    }

    pub fn encode_png(&self) -> anyhow::Result<Vec<u8>> {
        let img = image::RgbaImage::from_raw(self.w as u32, self.h as u32, self.to_straight())
            .ok_or_else(|| anyhow::anyhow!("búfer inválido"))?;
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)?;
        Ok(out.into_inner())
    }
}

/// Transformación afín 2D: (x,y) -> (a*x + b*y + c, d*x + e*y + f).
#[derive(Clone, Copy, Debug)]
pub struct Affine(pub [f32; 6]);

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    pub fn translate(x: f32, y: f32) -> Affine {
        Affine([1.0, 0.0, x, 0.0, 1.0, y])
    }
    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine([sx, 0.0, 0.0, 0.0, sy, 0.0])
    }
    pub fn rotate(rad: f32) -> Affine {
        let (s, c) = rad.sin_cos();
        Affine([c, -s, 0.0, s, c, 0.0])
    }
    pub fn shear_x(k: f32) -> Affine {
        Affine([1.0, k, 0.0, 0.0, 1.0, 0.0])
    }
    /// self ∘ o (aplica `o` primero y luego `self`).
    pub fn then_after(&self, o: &Affine) -> Affine {
        let a = self.0;
        let b = o.0;
        Affine([
            a[0] * b[0] + a[1] * b[3],
            a[0] * b[1] + a[1] * b[4],
            a[0] * b[2] + a[1] * b[5] + a[2],
            a[3] * b[0] + a[4] * b[3],
            a[3] * b[1] + a[4] * b[4],
            a[3] * b[2] + a[4] * b[5] + a[5],
        ])
    }
    /// Aplica `self` y después `next`.
    pub fn then(&self, next: &Affine) -> Affine {
        next.then_after(self)
    }
    pub fn about(center: (f32, f32), m: &Affine) -> Affine {
        Affine::translate(-center.0, -center.1)
            .then(m)
            .then(&Affine::translate(center.0, center.1))
    }
    pub fn invert(&self) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let det = a * e - b * d;
        let det = if det.abs() < 1e-12 { 1e-12 } else { det };
        let ia = e / det;
        let ib = -b / det;
        let id = -d / det;
        let ie = a / det;
        Affine([ia, ib, -(ia * c + ib * f), id, ie, -(id * c + ie * f)])
    }
    #[inline]
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let m = self.0;
        (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
    }
    pub fn is_identity(&self) -> bool {
        let m = self.0;
        (m[0] - 1.0).abs() < 1e-5
            && m[1].abs() < 1e-5
            && m[2].abs() < 1e-3
            && m[3].abs() < 1e-5
            && (m[4] - 1.0).abs() < 1e-5
            && m[5].abs() < 1e-3
    }
}

#[inline]
fn mirror_coord(v: i64, n: i64) -> i64 {
    if n <= 1 {
        return 0;
    }
    let period = 2 * n;
    let mut m = v.rem_euclid(period);
    if m >= n {
        m = period - 1 - m;
    }
    m
}

#[inline]
fn fetch(f: &Frame, x: i64, y: i64, edge: Edge) -> [f32; 4] {
    let (w, h) = (f.w as i64, f.h as i64);
    let (xx, yy) = match edge {
        Edge::Clamp => (x.clamp(0, w - 1), y.clamp(0, h - 1)),
        Edge::Mirror => (mirror_coord(x, w), mirror_coord(y, h)),
        Edge::Transparent => {
            if x < 0 || y < 0 || x >= w || y >= h {
                return [0.0; 4];
            }
            (x, y)
        }
    };
    let i = ((yy * w + xx) * 4) as usize;
    let d = &f.data;
    [d[i] as f32, d[i + 1] as f32, d[i + 2] as f32, d[i + 3] as f32]
}

/// Muestreo bilineal. Coordenadas en píxeles con centros en i+0.5.
#[inline]
pub fn sample(f: &Frame, x: f32, y: f32, edge: Edge) -> [f32; 4] {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let (xi, yi) = (x0 as i64, y0 as i64);
    // camino rápido: interior
    if xi >= 0 && yi >= 0 && xi + 1 < f.w as i64 && yi + 1 < f.h as i64 {
        let w = f.w;
        let i00 = (yi as usize * w + xi as usize) * 4;
        let i10 = i00 + 4;
        let i01 = i00 + w * 4;
        let i11 = i01 + 4;
        let d = &f.data;
        let mut out = [0f32; 4];
        for c in 0..4 {
            let a = d[i00 + c] as f32 + (d[i10 + c] as f32 - d[i00 + c] as f32) * tx;
            let b = d[i01 + c] as f32 + (d[i11 + c] as f32 - d[i01 + c] as f32) * tx;
            out[c] = a + (b - a) * ty;
        }
        return out;
    }
    let p00 = fetch(f, xi, yi, edge);
    let p10 = fetch(f, xi + 1, yi, edge);
    let p01 = fetch(f, xi, yi + 1, edge);
    let p11 = fetch(f, xi + 1, yi + 1, edge);
    let mut out = [0f32; 4];
    for c in 0..4 {
        let a = p00[c] + (p10[c] - p00[c]) * tx;
        let b = p01[c] + (p11[c] - p01[c]) * tx;
        out[c] = a + (b - a) * ty;
    }
    out
}

#[inline]
pub fn sample_nearest(f: &Frame, x: f32, y: f32, edge: Edge) -> [f32; 4] {
    fetch(f, x.floor() as i64, y.floor() as i64, edge)
}

/// dst = src transformado por `m` (src->dst).
pub fn warp(src: &Frame, dst: &mut Frame, m: &Affine, edge: Edge) {
    let inv = m.invert();
    let w = dst.w;
    dst.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let yc = y as f32 + 0.5;
        for x in 0..w {
            let (sx, sy) = inv.apply(x as f32 + 0.5, yc);
            let p = sample(src, sx, sy, edge);
            let o = &mut row[x * 4..x * 4 + 4];
            o[0] = p[0] as u8;
            o[1] = p[1] as u8;
            o[2] = p[2] as u8;
            o[3] = p[3] as u8;
        }
    });
}

pub fn warp_in_place(f: &mut Frame, m: &Affine, edge: Edge) {
    if m.is_identity() {
        return;
    }
    let src = f.clone();
    warp(&src, f, m, edge);
}

#[inline]
fn blend_px(d: &mut [u8], s: [f32; 4], opacity: f32, mode: Blend) {
    let sa = s[3] * opacity / 255.0;
    if sa <= 0.0 {
        return;
    }
    let sr = s[0] * opacity;
    let sg = s[1] * opacity;
    let sb = s[2] * opacity;
    let dr = d[0] as f32;
    let dg = d[1] as f32;
    let db = d[2] as f32;
    let da = d[3] as f32;
    let inv = 1.0 - sa;
    let (r, g, b) = match mode {
        Blend::Normal => (sr + dr * inv, sg + dg * inv, sb + db * inv),
        Blend::Add => (dr + sr, dg + sg, db + sb),
        Blend::Screen => (
            dr + sr - dr * sr / 255.0,
            dg + sg - dg * sg / 255.0,
            db + sb - db * sb / 255.0,
        ),
        Blend::Multiply => (
            dr * inv + dr * sr / 255.0,
            dg * inv + dg * sg / 255.0,
            db * inv + db * sb / 255.0,
        ),
        Blend::Overlay => {
            let ov = |dc: f32, sc: f32| {
                // sc premultiplicado; aproximación usando el color recto
                let s_str = if sa > 0.0 { sc / (sa) } else { 0.0 };
                let res = if dc < 128.0 {
                    2.0 * dc * s_str / 255.0
                } else {
                    255.0 - 2.0 * (255.0 - dc) * (255.0 - s_str) / 255.0
                };
                dc * inv + res * sa
            };
            (ov(dr, sr), ov(dg, sg), ov(db, sb))
        }
    };
    let a = match mode {
        Blend::Normal | Blend::Overlay | Blend::Multiply => sa * 255.0 + da * inv,
        _ => (da + sa * 255.0).min(255.0),
    };
    d[0] = r.clamp(0.0, 255.0) as u8;
    d[1] = g.clamp(0.0, 255.0) as u8;
    d[2] = b.clamp(0.0, 255.0) as u8;
    d[3] = a.clamp(0.0, 255.0) as u8;
}

/// Compone `src` (mismo tamaño) sobre `dst`.
pub fn composite(dst: &mut Frame, src: &Frame, opacity: f32, mode: Blend) {
    let w = dst.w;
    dst.data
        .par_chunks_mut(w * 4)
        .zip(src.data.par_chunks(w * 4))
        .for_each(|(drow, srow)| {
            for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(4)) {
                if s[3] == 0 && mode != Blend::Multiply {
                    continue;
                }
                if s[3] == 255 && opacity >= 1.0 && mode == Blend::Normal {
                    d.copy_from_slice(s);
                    continue;
                }
                blend_px(d, [s[0] as f32, s[1] as f32, s[2] as f32, s[3] as f32], opacity, mode);
            }
        });
}

/// Copia rápida con desplazamiento entero (mezcla normal, aritmética entera).
pub fn blit(dst: &mut Frame, src: &Frame, ox: i64, oy: i64, opacity: f32) {
    let x0 = ox.max(0);
    let y0 = oy.max(0);
    let x1 = (ox + src.w as i64).min(dst.w as i64);
    let y1 = (oy + src.h as i64).min(dst.h as i64);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let op = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
    let w = dst.w;
    let (x0, x1, y0, y1) = (x0 as usize, x1 as usize, y0 as usize, y1 as usize);
    dst.data[y0 * w * 4..y1 * w * 4].par_chunks_mut(w * 4).enumerate().for_each(|(ry, row)| {
        let sy = (y0 + ry) as i64 - oy;
        let srow = &src.data[sy as usize * src.w * 4..(sy as usize + 1) * src.w * 4];
        for x in x0..x1 {
            let sx = (x as i64 - ox) as usize;
            let s = &srow[sx * 4..sx * 4 + 4];
            if s[3] == 0 {
                continue;
            }
            let d = &mut row[x * 4..x * 4 + 4];
            if op >= 256 {
                if s[3] == 255 {
                    d.copy_from_slice(s);
                } else {
                    let inv = 255 - s[3] as u32;
                    for c in 0..4 {
                        d[c] = (s[c] as u32 + (d[c] as u32 * inv + 127) / 255).min(255) as u8;
                    }
                }
            } else {
                let sa = (s[3] as u32 * op) >> 8;
                let inv = 255 - sa;
                for c in 0..4 {
                    let sc = (s[c] as u32 * op) >> 8;
                    d[c] = (sc + (d[c] as u32 * inv + 127) / 255).min(255) as u8;
                }
            }
        }
    });
}

/// Aplica `op` a media resolución y vuelve a escalar (para desenfoques grandes).
pub fn at_half(f: &mut Frame, op: impl FnOnce(&mut Frame)) {
    let mut small = downscale(f, 2);
    op(&mut small);
    upscale_into(&small, f);
}

/// Inclina un sprite horizontalmente (cursiva falsa). Devuelve el sprite nuevo y el desplazamiento x aplicado.
pub fn shear_sprite(src: &Frame, skew_deg: f32) -> Frame {
    let k = skew_deg.to_radians().tan();
    if k.abs() < 1e-4 {
        return src.clone();
    }
    let extra = (k.abs() * src.h as f32).ceil() as usize + 2;
    let mut out = Frame::new(src.w + extra, src.h);
    // centrado vertical: x' = x + k*(y - h/2) + extra/2
    let m = Affine::translate(0.0, -(src.h as f32) / 2.0)
        .then(&Affine::shear_x(k))
        .then(&Affine::translate(extra as f32 / 2.0, src.h as f32 / 2.0));
    draw_sprite(&mut out, src, &m, 1.0, Blend::Normal);
    out
}

/// Dibuja un sprite con transformación afín (sprite->dst).
pub fn draw_sprite(dst: &mut Frame, sprite: &Frame, m: &Affine, opacity: f32, mode: Blend) {
    if opacity <= 0.001 || sprite.w == 0 || sprite.h == 0 {
        return;
    }
    // traslación pura: copia entera rápida
    if mode == Blend::Normal
        && (m.0[0] - 1.0).abs() < 1e-4
        && m.0[1].abs() < 1e-4
        && m.0[3].abs() < 1e-4
        && (m.0[4] - 1.0).abs() < 1e-4
    {
        blit(dst, sprite, m.0[2].round() as i64, m.0[5].round() as i64, opacity);
        return;
    }
    let corners = [
        m.apply(0.0, 0.0),
        m.apply(sprite.w as f32, 0.0),
        m.apply(0.0, sprite.h as f32),
        m.apply(sprite.w as f32, sprite.h as f32),
    ];
    let minx = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).ceil().min(dst.w as f32) as usize;
    let miny = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxy = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).ceil().min(dst.h as f32) as usize;
    if minx >= maxx || miny >= maxy {
        return;
    }
    let inv = m.invert();
    let w = dst.w;
    // detectar traslación entera pura para ir rápido
    let pure = (m.0[0] - 1.0).abs() < 1e-6
        && m.0[1].abs() < 1e-6
        && m.0[3].abs() < 1e-6
        && (m.0[4] - 1.0).abs() < 1e-6
        && (m.0[2] - m.0[2].round()).abs() < 1e-4
        && (m.0[5] - m.0[5].round()).abs() < 1e-4;
    dst.data[miny * w * 4..maxy * w * 4]
        .par_chunks_mut(w * 4)
        .enumerate()
        .for_each(|(ry, row)| {
            let y = miny + ry;
            let yc = y as f32 + 0.5;
            for x in minx..maxx {
                let (sx, sy) = inv.apply(x as f32 + 0.5, yc);
                if sx < -1.0 || sy < -1.0 || sx > sprite.w as f32 + 1.0 || sy > sprite.h as f32 + 1.0 {
                    continue;
                }
                let p = if pure {
                    sample_nearest(sprite, sx, sy, Edge::Transparent)
                } else {
                    sample(sprite, sx, sy, Edge::Transparent)
                };
                if p[3] <= 0.5 && mode != Blend::Multiply {
                    continue;
                }
                blend_px(&mut row[x * 4..x * 4 + 4], p, opacity, mode);
            }
        });
}

/// Mezcla lineal: out = a*(1-t) + b*t.
pub fn mix(a: &Frame, b: &Frame, t: f32, out: &mut Frame) {
    let ti = (t.clamp(0.0, 1.0) * 256.0) as u32;
    let ui = 256 - ti;
    out.data
        .par_chunks_mut(CHUNK)
        .zip(a.data.par_chunks(CHUNK))
        .zip(b.data.par_chunks(CHUNK))
        .for_each(|((o, a), b)| {
            for i in 0..o.len() {
                o[i] = ((a[i] as u32 * ui + b[i] as u32 * ti) >> 8) as u8;
            }
        });
}

/// Mezcla hacia un color sólido.
pub fn mix_color(f: &mut Frame, c: [u8; 4], t: f32) {
    let p = premul(c);
    let ti = (t.clamp(0.0, 1.0) * 256.0) as u32;
    let ui = 256 - ti;
    f.data.par_chunks_mut(CHUNK).for_each(|ch| {
        for px in ch.chunks_exact_mut(4) {
            for c in 0..4 {
                px[c] = ((px[c] as u32 * ui + p[c] as u32 * ti) >> 8) as u8;
            }
        }
    });
}

/// Rellena un rectángulo (coordenadas en píxeles), con mezcla normal.
pub fn fill_rect(f: &mut Frame, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 4], opacity: f32) {
    let xa = x0.max(0.0).floor() as usize;
    let xb = (x1.min(f.w as f32).ceil() as usize).min(f.w);
    let ya = y0.max(0.0).floor() as usize;
    let yb = (y1.min(f.h as f32).ceil() as usize).min(f.h);
    if xa >= xb || ya >= yb {
        return;
    }
    let p = premul(c);
    // camino rápido: color opaco sobre filas enteras
    if c[3] == 255 && opacity >= 1.0 {
        let iy0 = y0.max(0.0).ceil() as usize;
        let iy1 = (y1.min(f.h as f32).floor() as usize).max(iy0);
        let ix0 = x0.max(0.0).ceil() as usize;
        let ix1 = (x1.min(f.w as f32).floor() as usize).max(ix0);
        if iy0 == ya && iy1 == yb && ix0 == xa && ix1 == xb {
            let w = f.w;
            f.data[ya * w * 4..yb * w * 4].par_chunks_mut(w * 4).for_each(|row| {
                for px in row[xa * 4..xb * 4].chunks_exact_mut(4) {
                    px.copy_from_slice(&p);
                }
            });
            return;
        }
    }
    let s = [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32];
    let w = f.w;
    f.data[ya * w * 4..yb * w * 4]
        .par_chunks_mut(w * 4)
        .enumerate()
        .for_each(|(ry, row)| {
            let y = (ya + ry) as f32;
            // cobertura vertical parcial
            let cy = ((y + 1.0).min(y1) - y.max(y0)).clamp(0.0, 1.0);
            for x in xa..xb {
                let xf = x as f32;
                let cx = ((xf + 1.0).min(x1) - xf.max(x0)).clamp(0.0, 1.0);
                let cov = cx * cy * opacity;
                if cov > 0.0 {
                    blend_px(&mut row[x * 4..x * 4 + 4], s, cov, Blend::Normal);
                }
            }
        });
}

struct SyncPtr(*mut u8);
unsafe impl Sync for SyncPtr {}
unsafe impl Send for SyncPtr {}

fn box_h(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    dst.par_chunks_mut(w * 4).enumerate().for_each(|(y, out)| {
        let row = &src[y * w * 4..(y + 1) * w * 4];
        let mut acc = [0u32; 4];
        let get = |x: i64| -> usize { (x.clamp(0, w as i64 - 1) as usize) * 4 };
        for k in -(r as i64)..=(r as i64) {
            let i = get(k);
            for c in 0..4 {
                acc[c] += row[i + c] as u32;
            }
        }
        for x in 0..w {
            for c in 0..4 {
                out[x * 4 + c] = (acc[c] as f32 * norm + 0.5) as u8;
            }
            let add = get(x as i64 + r as i64 + 1);
            let sub = get(x as i64 - r as i64);
            for c in 0..4 {
                acc[c] = acc[c] + row[add + c] as u32 - row[sub + c] as u32;
            }
        }
    });
    let _ = h;
}

fn box_v(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let norm = 1.0 / (2 * r + 1) as f32;
    let strip = 16usize;
    let nstrips = (w + strip - 1) / strip;
    let ptr = SyncPtr(dst.as_mut_ptr());
    let ptr = &ptr;
    (0..nstrips).into_par_iter().for_each(|s| {
        let x0 = s * strip;
        let x1 = (x0 + strip).min(w);
        let n = x1 - x0;
        let mut acc = vec![0u32; n * 4];
        let get = |y: i64| -> usize { y.clamp(0, h as i64 - 1) as usize };
        for k in -(r as i64)..=(r as i64) {
            let yy = get(k);
            let base = (yy * w + x0) * 4;
            for i in 0..n * 4 {
                acc[i] += src[base + i] as u32;
            }
        }
        for y in 0..h {
            let base = (y * w + x0) * 4;
            for i in 0..n * 4 {
                // SAFETY: cada franja escribe columnas disjuntas.
                unsafe {
                    *ptr.0.add(base + i) = (acc[i] as f32 * norm + 0.5) as u8;
                }
            }
            let add = (get(y as i64 + r as i64 + 1) * w + x0) * 4;
            let sub = (get(y as i64 - r as i64) * w + x0) * 4;
            for i in 0..n * 4 {
                acc[i] = acc[i] + src[add + i] as u32 - src[sub + i] as u32;
            }
        }
    });
}

fn box_blur3(f: &mut Frame, radius: f32) {
    let r = (radius / 1.7).round().max(1.0) as usize;
    let mut tmp = vec![0u8; f.data.len()];
    for _ in 0..3 {
        box_h(&f.data, &mut tmp, f.w, f.h, r);
        box_v(&tmp, &mut f.data, f.w, f.h, r);
    }
}

pub fn downscale(f: &Frame, factor: usize) -> Frame {
    let factor = factor.max(1);
    let w = (f.w / factor).max(1);
    let h = (f.h / factor).max(1);
    let mut out = Frame::new(w, h);
    let n = (factor * factor) as u32;
    out.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let mut acc = [0u32; 4];
            for dy in 0..factor {
                let sy = (y * factor + dy).min(f.h - 1);
                for dx in 0..factor {
                    let sx = (x * factor + dx).min(f.w - 1);
                    let i = (sy * f.w + sx) * 4;
                    for c in 0..4 {
                        acc[c] += f.data[i + c] as u32;
                    }
                }
            }
            for c in 0..4 {
                row[x * 4 + c] = (acc[c] / n) as u8;
            }
        }
    });
    out
}

pub fn upscale_into(small: &Frame, dst: &mut Frame) {
    let sx = small.w as f32 / dst.w as f32;
    let sy = small.h as f32 / dst.h as f32;
    let w = dst.w;
    dst.data.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 + 0.5) * sy;
        for x in 0..w {
            let p = sample(small, (x as f32 + 0.5) * sx, fy, Edge::Clamp);
            for c in 0..4 {
                row[x * 4 + c] = p[c] as u8;
            }
        }
    });
}

/// Desenfoque aproximadamente gaussiano (radio en píxeles).
pub fn blur(f: &mut Frame, radius: f32) {
    if radius < 0.6 {
        return;
    }
    let factor = if radius > 48.0 {
        8
    } else if radius > 24.0 {
        4
    } else if radius > 10.0 {
        2
    } else {
        1
    };
    if factor == 1 {
        box_blur3(f, radius);
    } else {
        let mut small = downscale(f, factor);
        box_blur3(&mut small, radius / factor as f32);
        upscale_into(&small, f);
    }
}
