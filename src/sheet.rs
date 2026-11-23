//! Hojas de contacto: cuadrículas de miniaturas con su tiempo, para que la IA "vea" un video.

use crate::frame::*;
use crate::media;
use crate::text;
use crate::util::fmt_time;
use anyhow::Result;

/// Une frames en una cuadrícula con etiquetas.
pub fn tile(items: &[(String, Frame)], cols: usize, thumb_w: usize) -> Frame {
    let cols = cols.max(1).min(items.len().max(1));
    let (fw, fh) = items.first().map(|(_, f)| (f.w, f.h)).unwrap_or((16, 9));
    let tw = thumb_w.max(32);
    let th = ((tw as f32 * fh as f32 / fw as f32).round() as usize).max(8);
    let gap = 6usize;
    let rows = (items.len() + cols - 1) / cols;
    let w = cols * tw + (cols + 1) * gap;
    let h = rows * th + (rows + 1) * gap;
    let mut out = Frame::filled(w, h, [24, 24, 28, 255]);
    let lsize = (tw as f32 / 13.0).clamp(11.0, 26.0);
    for (i, (label, f)) in items.iter().enumerate() {
        let cx = gap + (i % cols) * (tw + gap);
        let cy = gap + (i / cols) * (th + gap);
        let thumb = media::resize(f, tw, th);
        draw_sprite(&mut out, &thumb, &Affine::translate(cx as f32, cy as f32), 1.0, Blend::Normal);
        let lab = text::label(label, lsize, [255, 255, 255, 255], Some([0, 0, 0, 190]));
        draw_sprite(&mut out, &lab, &Affine::translate(cx as f32 + 3.0, cy as f32 + 3.0), 1.0, Blend::Normal);
    }
    out
}

pub fn contact_sheet(path: &str, start: f64, end: Option<f64>, count: usize, cols: usize, thumb_w: usize) -> Result<(Frame, Vec<f64>)> {
    let info = media::probe(path)?;
    if info.kind == "image" {
        let img = media::load_image(path)?;
        return Ok((tile(&[("imagen".into(), img)], 1, thumb_w.max(480)), vec![0.0]));
    }
    if !info.has_video {
        anyhow::bail!("{path} no tiene video");
    }
    let end = end.unwrap_or(info.duration).min(info.duration);
    let tw = thumb_w.max(32);
    let th = (((tw as f32 * info.height.max(1) as f32 / info.width.max(1) as f32).round() as usize).max(8)) & !1;
    let frames = media::grab_frames(path, start, end, count, tw & !1, th)?;
    let times: Vec<f64> = frames.iter().map(|(t, _)| *t).collect();
    let items: Vec<(String, Frame)> = frames.into_iter().map(|(t, f)| (format!("{} ({:.1}s)", fmt_time(t), t), f)).collect();
    Ok((tile(&items, cols, tw), times))
}
