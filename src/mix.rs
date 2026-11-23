//! Mezcla de audio de la línea de tiempo: clips, capas de video, música y efectos de sonido.

use crate::audio::{self, apply_fx, envelope, frames, mix_into, read_interp, t2f};
use crate::media::{self, SR};
use crate::timeline::*;
use crate::util::smoothstep;
use anyhow::Result;
use std::collections::HashMap;

fn smooth_transition(kind: &str) -> bool {
    matches!(kind, "fade" | "crossfade" | "dissolve" | "fade_black" | "fade_color" | "dip" | "blur" | "luma" | "luma_fade")
}

/// Devuelve None si no hay ningún audio.
pub fn build_mix(tl: &Timeline) -> Result<Option<Vec<f32>>> {
    let total = (tl.duration * SR as f64).ceil() as usize + 1;
    let mut clips_bus = vec![0f32; total * 2];
    let mut music_bus = vec![0f32; total * 2];
    let mut sfx_bus = vec![0f32; total * 2];
    let mut any = false;

    // ---------------- clips principales (decodificación en paralelo)
    let ranges: Vec<Option<(f64, f64, f64, f64, f64)>> = tl
        .clips
        .iter()
        .enumerate()
        .map(|(ci, c)| {
            if !matches!(c.src, ClipSrc::Video { .. }) || !c.has_audio || c.volume <= 0.0 {
                return None;
            }
            let next_tr = tl.clips.get(ci + 1).and_then(|n| n.trans_in.as_ref());
            let pre_a = match &c.trans_in {
                Some(tr) if smooth_transition(&tr.kind) => c.pre,
                Some(_) => c.pre.min(0.06),
                None => 0.0,
            };
            let post_a = match next_tr {
                Some(tr) if smooth_transition(&tr.kind) => c.post,
                Some(_) => c.post.min(0.06),
                None => 0.0,
            };
            let cands = [c.src_time(-pre_a).0, c.src_time(c.dur + post_a).0, c.in_, c.out];
            let lo = cands.iter().cloned().fold(f64::MAX, f64::min).max(0.0);
            let hi = cands.iter().cloned().fold(f64::MIN, f64::max);
            Some((pre_a, post_a, lo, hi, if c.stretch { c.speed } else { 1.0 }))
        })
        .collect();
    let decoded: Vec<Option<Vec<f32>>> = std::thread::scope(|s| {
        let handles: Vec<_> = tl
            .clips
            .iter()
            .zip(ranges.iter())
            .map(|(c, r)| {
                s.spawn(move || {
                    let (ClipSrc::Video { path }, Some((_, _, lo, hi, tempo))) = (&c.src, r) else { return None };
                    media::decode_audio(path, *lo, Some(hi - lo + 0.1), *tempo).ok()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    for (ci, c) in tl.clips.iter().enumerate() {
        let (Some((pre_a, post_a, lo, _hi, _tempo)), Some(buf)) = (ranges[ci], decoded[ci].as_ref()) else { continue };
        if buf.is_empty() {
            continue;
        }
        any = true;
        let t_start = c.start - pre_a;
        let t_end = c.end() + post_a;
        let f0 = t2f(t_start).max(0) as usize;
        let f1 = (t2f(t_end).max(0) as usize).min(total);
        let micro = 0.004;
        for fi in f0..f1 {
            let t = fi as f64 / SR as f64;
            let tau = t - c.start;
            let (s, frozen) = c.src_time(tau);
            if frozen {
                continue;
            }
            // ganancia de entrada/salida (potencia constante en transiciones suaves)
            let gin = if pre_a > 0.0 {
                (((tau + pre_a) / (2.0 * pre_a)) as f32).clamp(0.0, 1.0)
            } else {
                ((tau / micro) as f32).clamp(0.0, 1.0)
            };
            let rem = c.dur - tau;
            let gout = if post_a > 0.0 {
                (((rem + post_a) / (2.0 * post_a)) as f32).clamp(0.0, 1.0)
            } else {
                ((rem / micro) as f32).clamp(0.0, 1.0)
            };
            let g = (gin * std::f32::consts::FRAC_PI_2).sin() * (gout * std::f32::consts::FRAC_PI_2).sin() * c.volume;
            if g <= 0.0 {
                continue;
            }
            // en repeticiones, micro-fundido en cada vuelta
            let g = if c.repeat > 1 && c.single > 0.0 && tau >= 0.0 && tau < c.single * c.repeat as f64 {
                let ph = tau % c.single;
                let e = ph.min(c.single - ph);
                g * ((e / micro) as f32).clamp(0.0, 1.0)
            } else {
                g
            };
            let pos = if c.stretch && (c.speed - 1.0).abs() > 1e-4 { (s - lo) / c.speed } else { s - lo } * SR as f64;
            if let Some((l, r)) = read_interp(&buf, pos) {
                clips_bus[fi * 2] += l * g;
                clips_bus[fi * 2 + 1] += r * g;
            }
        }
    }

    // ---------------- capas de video con sonido
    for l in &tl.layers {
        if l.l.kind != "video" {
            continue;
        }
        let vol = l.l.volume.unwrap_or(1.0);
        let (Some(path), Some(info)) = (&l.path, &l.info) else { continue };
        if vol <= 0.0 || !info.has_audio {
            continue;
        }
        let a = l.l.in_.map(|t| t.0).unwrap_or(0.0);
        let b = l.l.out.map(|t| t.0).unwrap_or(info.duration);
        let speed = l.l.speed.unwrap_or(1.0).max(0.05);
        let buf = media::decode_audio(path, a, Some(b - a), speed)?;
        if buf.is_empty() {
            continue;
        }
        any = true;
        let n = frames(&buf);
        let f0 = t2f(l.start).max(0) as usize;
        let f1 = (t2f(l.end).max(0) as usize).min(total);
        let lp = l.l.loop_.unwrap_or(false);
        for fi in f0..f1 {
            let mut j = fi - f0;
            if lp && n > 0 {
                j %= n;
            }
            if j >= n {
                break;
            }
            let edge = (fi - f0).min(f1 - fi) as f32 / (0.01 * SR as f32);
            let g = vol * edge.min(1.0);
            clips_bus[fi * 2] += buf[j * 2] * g;
            clips_bus[fi * 2 + 1] += buf[j * 2 + 1] * g;
        }
    }

    // efectos en el bus de clips antes de calcular el ducking
    for f in tl.audio_fx.iter().filter(|f| f.target == "clips" || f.target == "voice") {
        apply_fx(&mut clips_bus, &f.fx, f.start, f.end);
    }
    let env = if tl.audio.iter().any(|a| a.duck > 0.0) { Some(envelope(&clips_bus)) } else { None };

    // ---------------- música y efectos de sonido
    let mut cache: HashMap<String, Vec<f32>> = HashMap::new();
    for a in &tl.audio {
        let key = match &a.src {
            AudioSrc::Synth(n) => format!("synth:{n}:{:?}:{}", a.duration, a.pitch),
            AudioSrc::File(p) => format!("file:{p}:{}:{:?}:{}:{}", a.in_, a.out, a.speed, a.pitch),
        };
        let buf = if let Some(b) = cache.get(&key) {
            b.clone()
        } else {
            let b = match &a.src {
                AudioSrc::Synth(n) => crate::sfx::synth(n, a.duration).unwrap_or_default(),
                AudioSrc::File(p) => {
                    let dur = a.out.map(|o| o - a.in_);
                    media::decode_audio(p, a.in_, dur, a.speed).unwrap_or_default()
                }
            };
            let b = crate::sfx::repitch(&b, a.pitch);
            cache.insert(key, b.clone());
            b
        };
        if buf.is_empty() {
            continue;
        }
        any = true;
        // longitud final (bucle / end)
        let end_t = a.end.unwrap_or(if a.loop_ { tl.duration } else { a.start + frames(&buf) as f64 / SR as f64 });
        let len = ((end_t - a.start) * SR as f64).max(0.0) as usize;
        let mut item: Vec<f32> = if a.loop_ {
            let n = frames(&buf);
            (0..len).flat_map(|i| {
                let j = i % n;
                [buf[j * 2], buf[j * 2 + 1]]
            })
            .collect()
        } else {
            buf[..(len.min(frames(&buf)) * 2)].to_vec()
        };
        // ducking
        if let (Some(env), true) = (&env, a.duck > 0.0) {
            let f0 = t2f(a.start);
            for i in 0..frames(&item) {
                let o = f0 + i as i64;
                if o < 0 || o as usize >= env.len() {
                    continue;
                }
                let g = 1.0 - a.duck.clamp(0.0, 1.0) * smoothstep(0.015, 0.12, env[o as usize]);
                item[i * 2] *= g;
                item[i * 2 + 1] *= g;
            }
        }
        let fi = (a.fade_in * SR as f64) as usize;
        let fo = (a.fade_out * SR as f64) as usize;
        let micro = (0.003 * SR as f64) as usize;
        let bus = match a.bus {
            Bus::Clips => &mut clips_bus,
            Bus::Music => &mut music_bus,
            Bus::Sfx => &mut sfx_bus,
        };
        mix_into(bus, &item, t2f(a.start), a.volume, fi.max(if a.in_ > 0.0 { micro } else { 0 }), fo.max(micro), a.pan);
    }
    if !any {
        return Ok(None);
    }

    for f in tl.audio_fx.iter() {
        match f.target.as_str() {
            "music" => apply_fx(&mut music_bus, &f.fx, f.start, f.end),
            "sfx" => apply_fx(&mut sfx_bus, &f.fx, f.start, f.end),
            _ => {}
        }
    }
    let mut master = clips_bus;
    for i in 0..master.len() {
        master[i] += music_bus[i] + sfx_bus[i];
    }
    for f in tl.audio_fx.iter().filter(|f| !matches!(f.target.as_str(), "clips" | "voice" | "music" | "sfx")) {
        apply_fx(&mut master, &f.fx, f.start, f.end);
    }
    let mut gain = tl.volume;
    if let Some(target) = tl.loudness {
        if let Some(l) = audio::loudness_lufs(&master) {
            gain *= 10f32.powf((target - l) / 20.0);
        }
    }
    if (gain - 1.0).abs() > 1e-4 {
        for s in master.iter_mut() {
            *s *= gain;
        }
    }
    audio::limit(&mut master, 0.89);
    Ok(Some(master))
}
