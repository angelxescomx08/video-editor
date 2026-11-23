//! Anclas a palabras de las transcripciones: `"@montaña"`, `"@montaña#2"`, `"@montaña:end"`, `"@montaña+0.2"`.
//!
//! Se resuelven sobre el JSON antes de leer el proyecto:
//! - en `clips[].in` / `clips[].out` → tiempo de la **fuente** (para cortar justo en una palabra);
//! - en el resto (`start`, `end`, `at`, `word_times` de capas, efectos, audio...) → tiempo de **salida**.
//!   En efectos de un clip o de una capa, relativo a su inicio, como cualquier otro tiempo.
//!
//! Como la transcripción es la única fuente de verdad, si corriges el tiempo de una palabra se mueven
//! juntos el subtítulo y todo lo anclado a ella.

use crate::project::Project;
use crate::timeline::{norm_word, ClipSrc};
use crate::transcribe::{load_transcript, Transcript};
use anyhow::{bail, Result};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Anchor {
    pub word: String,
    pub occurrence: usize,
    pub end: bool,
    pub offset: f64,
}

pub fn parse_anchor(s: &str) -> Option<Anchor> {
    let s = s.trim().strip_prefix('@')?;
    // separar desplazamiento final (+0.2 / -0.1)
    let mut body = s.to_string();
    let mut offset = 0.0;
    if let Some(pos) = body.rfind(|c| c == '+' || c == '-') {
        let tail = &body[pos + 1..];
        if !tail.is_empty() && tail.trim_end_matches('s').parse::<f64>().is_ok() {
            let v: f64 = tail.trim_end_matches('s').parse().unwrap();
            offset = if &body[pos..pos + 1] == "-" { -v } else { v };
            body.truncate(pos);
        }
    }
    let mut end = false;
    if let Some(p) = body.find(':') {
        let k = body[p + 1..].to_lowercase();
        end = k == "end" || k == "fin";
        body.truncate(p);
    }
    let mut occurrence = 1;
    if let Some(p) = body.find('#') {
        occurrence = body[p + 1..].parse().unwrap_or(1).max(1);
        body.truncate(p);
    }
    let word = norm_word(&body);
    if word.is_empty() {
        return None;
    }
    Some(Anchor { word, occurrence, end, offset })
}

fn is_anchor(v: &Value) -> bool {
    v.as_str().map(|s| s.trim_start().starts_with('@')).unwrap_or(false)
}

#[derive(Clone, Debug)]
struct W {
    norm: String,
    start: f64,
    end: f64,
}

fn resolve_in(words: &[W], a: &Anchor) -> Option<f64> {
    words
        .iter()
        .filter(|w| w.norm == a.word)
        .nth(a.occurrence - 1)
        .map(|w| if a.end { w.end } else { w.start } + a.offset)
}

fn lev(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let c = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + c);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn not_found(words: &[W], a: &Anchor, raw: &str, scope: &str) -> anyhow::Error {
    let count = words.iter().filter(|w| w.norm == a.word).count();
    if count > 0 {
        return anyhow::anyhow!("ancla '{raw}': '{}' aparece {count} vez/veces en {scope}, pediste la #{}", a.word, a.occurrence);
    }
    let mut cands: Vec<(usize, &str)> = words.iter().map(|w| (lev(&w.norm, &a.word), w.norm.as_str())).collect();
    cands.sort();
    cands.dedup_by(|x, y| x.1 == y.1);
    let sug: Vec<&str> = cands.iter().take(4).map(|c| c.1).collect();
    anyhow::anyhow!("ancla '{raw}': la palabra no aparece en {scope}. ¿Quisiste decir: {}?", sug.join(", "))
}

fn resolve_path(base: &Path, sources: &serde_json::Map<String, Value>, s: &str) -> String {
    let s = sources.get(s).and_then(|v| v.as_str()).unwrap_or(s).to_string();
    let p = Path::new(&s);
    if p.is_absolute() {
        s
    } else {
        base.join(p).to_string_lossy().to_string()
    }
}

fn same_file(a: &str, b: &str) -> bool {
    match (std::fs::canonicalize(a).ok(), std::fs::canonicalize(b).ok()) {
        (Some(x), Some(y)) => x == y,
        _ => a.replace('\\', "/").eq_ignore_ascii_case(&b.replace('\\', "/")),
    }
}

/// Transcripciones del proyecto: (ruta del archivo de transcripción, transcripción).
pub fn project_transcripts(v: &Value, base: &Path) -> Result<Vec<(String, Transcript)>> {
    let mut out = Vec::new();
    if let Some(m) = v.get("transcripts").and_then(|x| x.as_object()) {
        for (_alias, p) in m {
            let Some(p) = p.as_str() else { continue };
            let path = if Path::new(p).is_absolute() { p.to_string() } else { base.join(p).to_string_lossy().to_string() };
            out.push((path.clone(), load_transcript(Path::new(&path))?));
        }
    }
    Ok(out)
}

fn source_words(t: &Transcript) -> Vec<W> {
    t.segments
        .iter()
        .flat_map(|s| {
            let ws = if s.words.is_empty() { crate::transcribe::proportional_words(s) } else { s.words.clone() };
            ws.into_iter().map(|w| W { norm: norm_word(&w.word), start: w.start, end: w.end })
        })
        .collect()
}

fn walk_times(v: &mut Value, words: &[W], rel: f64, scope: &str, keys: &[&str]) -> Result<()> {
    let Some(obj) = v.as_object_mut() else { return Ok(()) };
    for k in keys {
        let Some(x) = obj.get_mut(*k) else { continue };
        match x {
            Value::String(s) if s.trim_start().starts_with('@') => {
                let a = parse_anchor(s).ok_or_else(|| anyhow::anyhow!("ancla inválida '{s}'"))?;
                let t = resolve_in(words, &a).ok_or_else(|| not_found(words, &a, s, scope))?;
                *x = Value::from(((t - rel) * 1000.0).round() / 1000.0);
            }
            Value::Array(arr) => {
                for it in arr.iter_mut() {
                    if let Value::String(s) = it {
                        if s.trim_start().starts_with('@') {
                            let a = parse_anchor(s).ok_or_else(|| anyhow::anyhow!("ancla inválida '{s}'"))?;
                            let t = resolve_in(words, &a).ok_or_else(|| not_found(words, &a, s, scope))?;
                            *it = Value::from(((t - rel) * 1000.0).round() / 1000.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn contains_anchor(v: &Value) -> bool {
    match v {
        Value::String(_) => is_anchor(v),
        Value::Array(a) => a.iter().any(contains_anchor),
        Value::Object(m) => m.values().any(contains_anchor),
        _ => false,
    }
}

fn strip_anchors(v: &mut Value) {
    match v {
        Value::String(_) if is_anchor(v) => *v = Value::from(0.0),
        Value::Array(a) => a.iter_mut().for_each(strip_anchors),
        Value::Object(m) => m.values_mut().for_each(strip_anchors),
        _ => {}
    }
}

/// Palabra con tiempo de salida (para el editor visual y las anclas).
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "studio"), allow(dead_code))]
pub struct OutWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub clip: usize,
    /// índice de transcripción, segmento y palabra
    pub tr: usize,
    pub seg: usize,
    pub idx: usize,
}

/// Lleva las palabras de las transcripciones al tiempo de salida según los clips.
pub fn output_words(clips: &[crate::timeline::TClip], trs: &[(String, Transcript)]) -> Vec<OutWord> {
    let mut out = Vec::new();
    for (ci, c) in clips.iter().enumerate() {
        let ClipSrc::Video { path } = &c.src else { continue };
        if c.reverse {
            continue;
        }
        for (ti, (_, t)) in trs.iter().enumerate() {
            if !same_file(path, &t.source) {
                continue;
            }
            for (si, s) in t.segments.iter().enumerate() {
                let ws = if s.words.is_empty() { crate::transcribe::proportional_words(s) } else { s.words.clone() };
                for (wi, w) in ws.iter().enumerate() {
                    if w.start >= c.in_ - 1e-3 && w.start < c.out - 0.03 {
                        let st = c.start + (w.start - c.in_) / c.speed;
                        let en = (c.start + (w.end - c.in_) / c.speed).min(c.start + c.single);
                        if st < c.start + c.single {
                            out.push(OutWord { word: w.word.clone(), start: st, end: en.max(st + 0.02), clip: ci, tr: ti, seg: si, idx: wi });
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    out
}

/// Resuelve todas las anclas del proyecto (JSON) y devuelve el proyecto listo.
pub fn resolve(mut v: Value, base: &Path) -> Result<Project> {
    if !contains_anchor(&v) {
        return Ok(serde_json::from_value(v).map_err(|e| anyhow::anyhow!("proyecto inválido: {e}"))?);
    }
    let trs = project_transcripts(&v, base)?;
    if trs.is_empty() {
        bail!("el proyecto usa anclas \"@palabra\" pero no tiene \"transcripts\" (alias de fuente → .transcript.json)");
    }
    let sources = v.get("sources").and_then(|x| x.as_object()).cloned().unwrap_or_default();
    let tmap = v.get("transcripts").and_then(|x| x.as_object()).cloned().unwrap_or_default();
    // 1) anclas de clips → tiempo de fuente
    if let Some(clips) = v.get_mut("clips").and_then(|c| c.as_array_mut()) {
        for (i, c) in clips.iter_mut().enumerate() {
            if !["in", "out"].iter().any(|k| c.get(*k).map(is_anchor).unwrap_or(false)) {
                continue;
            }
            let src = c.get("src").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let path = resolve_path(base, &sources, &src);
            let tr = tmap
                .get(&src)
                .and_then(|p| p.as_str())
                .and_then(|p| {
                    let pp = if Path::new(p).is_absolute() { p.to_string() } else { base.join(p).to_string_lossy().to_string() };
                    trs.iter().find(|(tp, _)| same_file(tp, &pp))
                })
                .or_else(|| trs.iter().find(|(_, t)| same_file(&t.source, &path)))
                .ok_or_else(|| anyhow::anyhow!("clips[{i}]: no hay transcripción para la fuente '{src}' en \"transcripts\""))?;
            let words = source_words(&tr.1);
            walk_times(c, &words, 0.0, &format!("la transcripción de '{src}'"), &["in", "out"])?;
        }
    }
    // 2) tiempos de salida de los clips (proyecto mínimo sin anclas)
    let mut mini = serde_json::json!({
        "output": v.get("output").cloned().unwrap_or(serde_json::json!({})),
        "sources": v.get("sources").cloned().unwrap_or(serde_json::json!({})),
        "clips": v.get("clips").cloned().unwrap_or(serde_json::json!([])),
    });
    strip_anchors(&mut mini);
    let mini: Project = serde_json::from_value(mini).map_err(|e| anyhow::anyhow!("proyecto inválido: {e}"))?;
    let tl = if mini.clips.is_empty() { None } else { Some(crate::timeline::prepare(&mini, base)?) };
    let clips = tl.map(|t| t.clips).unwrap_or_default();
    let words: Vec<W> = output_words(&clips, &trs).into_iter().map(|w| W { norm: norm_word(&w.word), start: w.start, end: w.end }).collect();
    let scope = "las palabras que suenan en el montaje";
    const KEYS: &[&str] = &["start", "end", "at", "word_times"];
    // 3) efectos de clip (relativos al clip)
    if let Some(cl) = v.get_mut("clips").and_then(|c| c.as_array_mut()) {
        for (i, c) in cl.iter_mut().enumerate() {
            let rel = clips.get(i).map(|x| x.start).unwrap_or(0.0);
            if let Some(fx) = c.get_mut("effects").and_then(|e| e.as_array_mut()) {
                for e in fx.iter_mut() {
                    walk_times(e, &words, rel, scope, KEYS)?;
                }
            }
        }
    }
    // 4) capas (y sus efectos, relativos a la capa)
    if let Some(ls) = v.get_mut("layers").and_then(|c| c.as_array_mut()) {
        for l in ls.iter_mut() {
            walk_times(l, &words, 0.0, scope, KEYS)?;
            if let Some(segs) = l.get_mut("segments").and_then(|s| s.as_array_mut()) {
                for s in segs.iter_mut() {
                    walk_times(s, &words, 0.0, scope, KEYS)?;
                }
            }
            let rel = l.get("start").and_then(|s| s.as_f64()).unwrap_or(0.0);
            if let Some(fx) = l.get_mut("effects").and_then(|e| e.as_array_mut()) {
                for e in fx.iter_mut() {
                    walk_times(e, &words, rel, scope, KEYS)?;
                }
            }
        }
    }
    // 5) efectos globales y audio
    for key in ["effects", "audio", "audio_effects"] {
        if let Some(arr) = v.get_mut(key).and_then(|c| c.as_array_mut()) {
            for e in arr.iter_mut() {
                walk_times(e, &words, 0.0, scope, KEYS)?;
            }
        }
    }
    if contains_anchor(&v) {
        bail!("quedan anclas \"@...\" en campos que no las admiten (se admiten en start, end, at, word_times y en in/out de clips)");
    }
    serde_json::from_value(v).map_err(|e| anyhow::anyhow!("proyecto inválido: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse() {
        let a = parse_anchor("@Montaña").unwrap();
        assert_eq!((a.word.as_str(), a.occurrence, a.end, a.offset), ("montaña", 1, false, 0.0));
        let a = parse_anchor("@montaña#2:end+0.25").unwrap();
        assert_eq!((a.occurrence, a.end, a.offset), (2, true, 0.25));
        let a = parse_anchor("@¡Hijo-0.1").unwrap();
        assert_eq!((a.word.as_str(), a.offset), ("hijo", -0.1));
        assert!(parse_anchor("montaña").is_none());
    }
}
