//! Transcripción y alineación forzada palabra por palabra con Whisper (candle, Rust puro).
//!
//! La alineación sigue el método de OpenAI (`word_timestamps`): se alimenta al decodificador el texto
//! conocido, se toman los pesos de atención cruzada de las "cabezas de alineación", se normalizan y
//! se resuelve un DTW entre tokens y frames de audio (20 ms). Así el texto puede corregirse a mano y
//! volver a alinearse con la voz.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Word {
    pub word: String,
    pub start: f64,
    pub end: f64,
    /// Fijada a mano: la realineación no la mueve y ajusta las vecinas a ella.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// Confianza de la alineación (0..1). Valores bajos conviene revisarlos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transcript {
    pub source: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub from: f64,
    #[serde(default)]
    pub to: f64,
    /// "dtw" si los tiempos por palabra vienen de la alineación forzada.
    #[serde(default)]
    pub alignment: String,
    pub segments: Vec<TSegment>,
}

static OVERRIDES: std::sync::Mutex<Vec<(std::path::PathBuf, Transcript)>> = std::sync::Mutex::new(Vec::new());

fn canon(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Sustituye en memoria una transcripción (la usa el editor visual mientras editas, sin guardar).
#[cfg_attr(not(feature = "studio"), allow(dead_code))]
pub fn set_override(path: &std::path::Path, t: Transcript) {
    let c = canon(path);
    let mut g = OVERRIDES.lock().unwrap();
    g.retain(|(p, _)| *p != c);
    g.push((c, t));
}

/// Carga una transcripción (de memoria si hay una versión editada, si no del disco).
pub fn load_transcript(path: &std::path::Path) -> anyhow::Result<Transcript> {
    let c = canon(path);
    if let Some((_, t)) = OVERRIDES.lock().unwrap().iter().find(|(p, _)| *p == c) {
        return Ok(t.clone());
    }
    let txt = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("no se pudo leer la transcripción {}: {e}", path.display()))?;
    serde_json::from_str(txt.trim_start_matches('\u{feff}')).map_err(|e| anyhow::anyhow!("transcripción inválida {}: {e}", path.display()))
}

pub fn to_srt(t: &Transcript) -> String {
    let ts = |s: f64| {
        let ms = (s * 1000.0).round() as i64;
        format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
    };
    let mut out = String::new();
    for (i, s) in t.segments.iter().enumerate() {
        out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, ts(s.start), ts(s.end), s.text.trim()));
    }
    out
}

/// Palabras del texto de un segmento (separadas por espacios).
pub fn split_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(String::from).collect()
}

/// Reparto proporcional (respaldo cuando no hay alineación).
pub fn proportional_words(seg: &TSegment) -> Vec<Word> {
    let words = split_words(&seg.text);
    if words.is_empty() {
        return vec![];
    }
    let weights: Vec<f64> = words.iter().map(|w| w.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f64 + 1.5).collect();
    let total: f64 = weights.iter().sum();
    let span = (seg.end - seg.start).max(0.05);
    let mut t = seg.start;
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let d = span * weights[i] / total;
        out.push(Word { word: w.clone(), start: r3(t), end: r3(t + d), locked: false, score: None });
        t += d;
    }
    out
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Ajusta las palabras alineadas para respetar las fijadas a mano (`locked`), interpolando entre anclas.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
pub fn apply_locks(old: &[Word], new: &mut [Word]) {
    if old.len() != new.len() {
        return;
    }
    // anclas: (tiempo alineado, tiempo fijado)
    let mut anchors: Vec<(f64, f64)> = Vec::new();
    for (o, n) in old.iter().zip(new.iter()) {
        if o.locked {
            anchors.push((n.start, o.start));
            anchors.push((n.end, o.end));
        }
    }
    if anchors.is_empty() {
        return;
    }
    anchors.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let map = |t: f64| -> f64 {
        let first = anchors[0];
        let last = anchors[anchors.len() - 1];
        if t <= first.0 {
            return t + (first.1 - first.0);
        }
        if t >= last.0 {
            return t + (last.1 - last.0);
        }
        for w in anchors.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t >= a.0 && t <= b.0 {
                let span = (b.0 - a.0).max(1e-6);
                let q = (t - a.0) / span;
                return a.1 + (b.1 - a.1) * q;
            }
        }
        t
    };
    for (o, n) in old.iter().zip(new.iter_mut()) {
        if o.locked {
            n.start = o.start;
            n.end = o.end;
            n.locked = true;
        } else {
            n.start = r3(map(n.start));
            n.end = r3(map(n.end).max(n.start + 0.02));
        }
    }
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
pub struct Opts<'a> {
    pub model: &'a str,
    pub lang: Option<&'a str>,
    pub log: &'a dyn Fn(&str),
}

#[cfg(feature = "whisper")]
pub use imp::{align, align_text, transcribe};

#[cfg(not(feature = "whisper"))]
pub fn transcribe(_path: &str, _from: f64, _to: Option<f64>, _o: &Opts) -> anyhow::Result<Transcript> {
    anyhow::bail!("esta compilación no incluye transcripción (compila con --features whisper)")
}
#[cfg(not(feature = "whisper"))]
pub fn align(_path: &str, _t: &mut Transcript, _only: Option<&[usize]>, _o: &Opts) -> anyhow::Result<()> {
    anyhow::bail!("esta compilación no incluye alineación (compila con --features whisper)")
}
#[cfg(not(feature = "whisper"))]
pub fn align_text(_path: &str, _from: f64, _to: f64, _text: &str, _o: &Opts) -> anyhow::Result<Transcript> {
    anyhow::bail!("esta compilación no incluye alineación (compila con --features whisper)")
}

#[cfg(feature = "whisper")]
mod imp {
    use super::*;
    use crate::media;
    use anyhow::{bail, Context, Result};
    use candle_core::{DType, Device, IndexOp, Tensor};
    use candle_nn::{Embedding, LayerNorm, Linear, Module, VarBuilder};
    use candle_transformers::models::whisper::{self as m, audio, Config};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    fn repo_for(model: &str) -> Result<&'static str> {
        Ok(match model {
            "tiny" => "openai/whisper-tiny",
            "base" => "openai/whisper-base",
            "small" => "openai/whisper-small",
            "medium" => "openai/whisper-medium",
            "turbo" | "large-v3-turbo" => "openai/whisper-large-v3-turbo",
            "large" | "large-v3" => "openai/whisper-large-v3",
            other => bail!("modelo '{other}' desconocido (tiny, base, small, medium, turbo, large-v3)"),
        })
    }

    pub fn models_dir() -> PathBuf {
        let base = std::env::var("MONTAJE_MODELS")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("LOCALAPPDATA").map(|l| PathBuf::from(l).join("montaje").join("models")))
            .unwrap_or_else(|_| PathBuf::from("models"));
        let _ = std::fs::create_dir_all(&base);
        base
    }

    fn fetch(repo: &str, file: &str, dir: &Path, log: &dyn Fn(&str)) -> Result<PathBuf> {
        let dst = dir.join(file);
        if dst.exists() && std::fs::metadata(&dst).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(dst);
        }
        std::fs::create_dir_all(dir)?;
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        log(&format!("descargando {url} ..."));
        let tmp = dir.join(format!("{file}.part"));
        let st = std::process::Command::new("curl")
            .args(["-L", "--fail", "--silent", "--show-error", "-o"])
            .arg(&tmp)
            .arg(&url)
            .status()
            .context("no se pudo ejecutar curl para descargar el modelo")?;
        if !st.success() {
            let _ = std::fs::remove_file(&tmp);
            bail!("falló la descarga de {url}");
        }
        std::fs::rename(&tmp, &dst)?;
        Ok(dst)
    }

    /// Banco de filtros mel tipo Slaney (como librosa / OpenAI).
    fn mel_filters(n_mels: usize) -> Vec<f32> {
        let sr = m::SAMPLE_RATE as f64;
        let n_fft = m::N_FFT;
        let n_bins = n_fft / 2 + 1;
        let f_sp = 200.0 / 3.0;
        let min_log_hz = 1000.0;
        let min_log_mel = min_log_hz / f_sp;
        let logstep = (6.4f64).ln() / 27.0;
        let hz_to_mel = |f: f64| if f < min_log_hz { f / f_sp } else { min_log_mel + (f / min_log_hz).ln() / logstep };
        let mel_to_hz = |mm: f64| if mm < min_log_mel { mm * f_sp } else { min_log_hz * (logstep * (mm - min_log_mel)).exp() };
        let mmax = hz_to_mel(sr / 2.0);
        let pts: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(mmax * i as f64 / (n_mels + 1) as f64)).collect();
        let mut w = vec![0f32; n_mels * n_bins];
        for i in 0..n_mels {
            let enorm = 2.0 / (pts[i + 2] - pts[i]);
            for k in 0..n_bins {
                let f = k as f64 * sr / n_fft as f64;
                let lower = (f - pts[i]) / (pts[i + 1] - pts[i]);
                let upper = (pts[i + 2] - f) / (pts[i + 2] - pts[i + 1]);
                w[i * n_bins + k] = (lower.min(upper).max(0.0) * enorm) as f32;
            }
        }
        w
    }

    // ------------------------------------------------------------ tokenizador BPE (byte-level, GPT-2)

    struct Tok {
        id_to_bytes: Vec<Vec<u8>>,
        vocab: HashMap<String, u32>,
        ranks: HashMap<String, usize>,
        byte_enc: [char; 256],
        special: HashMap<String, u32>,
    }

    fn byte_tables() -> ([char; 256], HashMap<char, u8>) {
        let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
        let mut cs: Vec<u32> = bs.clone();
        let mut n = 0;
        for b in 0..256u32 {
            if !bs.contains(&b) {
                bs.push(b);
                cs.push(256 + n);
                n += 1;
            }
        }
        let mut enc = [' '; 256];
        let mut dec = HashMap::new();
        for (b, c) in bs.into_iter().zip(cs) {
            let ch = char::from_u32(c).unwrap();
            enc[b as usize] = ch;
            dec.insert(ch, b as u8);
        }
        (enc, dec)
    }

    impl Tok {
        fn load(path: &Path, vocab_size: usize) -> Result<Tok> {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let (byte_enc, dec) = byte_tables();
            let mut id_to_bytes = vec![Vec::new(); vocab_size.max(52000)];
            let mut vocab = HashMap::new();
            if let Some(vo) = v["model"]["vocab"].as_object() {
                for (s, id) in vo {
                    let id = id.as_u64().unwrap_or(0) as usize;
                    if id < id_to_bytes.len() {
                        id_to_bytes[id] = s.chars().filter_map(|c| dec.get(&c).copied()).collect();
                    }
                    vocab.insert(s.clone(), id as u32);
                }
            }
            let mut ranks = HashMap::new();
            if let Some(ms) = v["model"]["merges"].as_array() {
                for (i, mg) in ms.iter().enumerate() {
                    let key = match mg {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Array(a) => format!("{} {}", a[0].as_str().unwrap_or(""), a[1].as_str().unwrap_or("")),
                        _ => continue,
                    };
                    ranks.insert(key, i);
                }
            }
            let mut special = HashMap::new();
            if let Some(added) = v["added_tokens"].as_array() {
                for a in added {
                    if let (Some(c), Some(id)) = (a["content"].as_str(), a["id"].as_u64()) {
                        special.insert(c.to_string(), id as u32);
                    }
                }
            }
            Ok(Tok { id_to_bytes, vocab, ranks, byte_enc, special })
        }

        fn id(&self, s: &str) -> Result<u32> {
            self.special.get(s).copied().ok_or_else(|| anyhow::anyhow!("token especial {s} no encontrado"))
        }

        fn decode(&self, ids: &[u32]) -> String {
            let mut b = Vec::new();
            for &i in ids {
                if let Some(x) = self.id_to_bytes.get(i as usize) {
                    b.extend_from_slice(x);
                }
            }
            String::from_utf8_lossy(&b).to_string()
        }

        /// Pre-tokenización GPT-2: 's|'t|'re|'ve|'m|'ll|'d| ?\p{L}+| ?\p{N}+| ?[^\s\p{L}\p{N}]+|\s+(?!\S)|\s+
        fn pretokenize(s: &str) -> Vec<String> {
            let ch: Vec<char> = s.chars().collect();
            let n = ch.len();
            let mut out = Vec::new();
            let mut i = 0;
            while i < n {
                if ch[i] == '\'' {
                    let rest: String = ch[i + 1..(i + 3).min(n)].iter().collect();
                    let suf = ["re", "ve", "ll"].iter().find(|x| rest.starts_with(**x)).map(|x| x.len()).or_else(|| {
                        ch.get(i + 1).filter(|c| matches!(c, 's' | 't' | 'm' | 'd')).map(|_| 1)
                    });
                    if let Some(l) = suf {
                        out.push(ch[i..i + 1 + l].iter().collect());
                        i += 1 + l;
                        continue;
                    }
                }
                let start = i;
                let mut j = i;
                if ch[j] == ' ' && j + 1 < n && !ch[j + 1].is_whitespace() {
                    j += 1;
                }
                let c = ch[j];
                if c.is_alphabetic() {
                    while j < n && ch[j].is_alphabetic() {
                        j += 1;
                    }
                } else if c.is_numeric() {
                    while j < n && ch[j].is_numeric() {
                        j += 1;
                    }
                } else if !c.is_whitespace() {
                    while j < n && !ch[j].is_whitespace() && !ch[j].is_alphabetic() && !ch[j].is_numeric() {
                        j += 1;
                    }
                } else {
                    while j < n && ch[j].is_whitespace() {
                        j += 1;
                    }
                    if j < n && j - start > 1 {
                        j -= 1;
                    }
                }
                out.push(ch[start..j].iter().collect());
                i = j.max(start + 1);
            }
            out
        }

        fn bpe(&self, piece: &str) -> Vec<u32> {
            let s: String = piece.bytes().map(|b| self.byte_enc[b as usize]).collect();
            if let Some(&id) = self.vocab.get(&s) {
                return vec![id];
            }
            let mut parts: Vec<String> = s.chars().map(|c| c.to_string()).collect();
            while parts.len() > 1 {
                let mut best: Option<(usize, usize)> = None;
                for i in 0..parts.len() - 1 {
                    if let Some(&r) = self.ranks.get(&format!("{} {}", parts[i], parts[i + 1])) {
                        if best.map_or(true, |b| r < b.0) {
                            best = Some((r, i));
                        }
                    }
                }
                let Some((_, bi)) = best else { break };
                let (a, b) = (parts[bi].clone(), parts[bi + 1].clone());
                let mut merged = Vec::with_capacity(parts.len());
                let mut k = 0;
                while k < parts.len() {
                    if k + 1 < parts.len() && parts[k] == a && parts[k + 1] == b {
                        merged.push(format!("{a}{b}"));
                        k += 2;
                    } else {
                        merged.push(parts[k].clone());
                        k += 1;
                    }
                }
                parts = merged;
            }
            parts.iter().filter_map(|p| self.vocab.get(p).copied()).collect()
        }

        fn encode(&self, text: &str) -> Vec<u32> {
            Self::pretokenize(text).iter().flat_map(|p| self.bpe(p)).collect()
        }
    }

    // ------------------------------------------------------------ red (codificador y decodificador con caché KV)

    fn lin(vb: &VarBuilder, i: usize, o: usize, bias: bool) -> candle_core::Result<Linear> {
        let w = vb.get((o, i), "weight")?;
        let b = if bias { Some(vb.get(o, "bias")?) } else { None };
        Ok(Linear::new(w, b))
    }
    fn ln(vb: &VarBuilder, n: usize) -> candle_core::Result<LayerNorm> {
        Ok(LayerNorm::new(vb.get(n, "weight")?, vb.get(n, "bias")?, 1e-5))
    }

    struct Attn {
        q: Linear,
        k: Linear,
        v: Linear,
        o: Linear,
    }
    impl Attn {
        fn load(vb: VarBuilder, d: usize) -> candle_core::Result<Attn> {
            Ok(Attn {
                q: lin(&vb.pp("q_proj"), d, d, true)?,
                k: lin(&vb.pp("k_proj"), d, d, false)?,
                v: lin(&vb.pp("v_proj"), d, d, true)?,
                o: lin(&vb.pp("out_proj"), d, d, true)?,
            })
        }
    }

    fn heads(x: &Tensor, n_head: usize, d: usize) -> candle_core::Result<Tensor> {
        let (b, n, _) = x.dims3()?;
        x.reshape((b, n, n_head, d / n_head))?.transpose(1, 2)?.contiguous()
    }

    struct ELayer {
        sa: Attn,
        sa_ln: LayerNorm,
        fc1: Linear,
        fc2: Linear,
        mlp_ln: LayerNorm,
    }
    struct Encoder {
        conv1: candle_nn::Conv1d,
        conv2: candle_nn::Conv1d,
        pos: Tensor,
        layers: Vec<ELayer>,
        ln: LayerNorm,
        n_head: usize,
        d: usize,
    }

    impl Encoder {
        fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Encoder> {
            let d = cfg.d_model;
            let conv = |name: &str, cin: usize, stride: usize| -> candle_core::Result<candle_nn::Conv1d> {
                let w = vb.pp(name).get((d, cin, 3), "weight")?;
                let b = vb.pp(name).get(d, "bias")?;
                Ok(candle_nn::Conv1d::new(w, Some(b), candle_nn::Conv1dConfig { padding: 1, stride, ..Default::default() }))
            };
            let mut layers = Vec::new();
            for i in 0..cfg.encoder_layers {
                let l = vb.pp(format!("layers.{i}"));
                layers.push(ELayer {
                    sa: Attn::load(l.pp("self_attn"), d)?,
                    sa_ln: ln(&l.pp("self_attn_layer_norm"), d)?,
                    fc1: lin(&l.pp("fc1"), d, d * 4, true)?,
                    fc2: lin(&l.pp("fc2"), d * 4, d, true)?,
                    mlp_ln: ln(&l.pp("final_layer_norm"), d)?,
                });
            }
            Ok(Encoder {
                conv1: conv("conv1", cfg.num_mel_bins, 1)?,
                conv2: conv("conv2", d, 2)?,
                pos: vb.get((cfg.max_source_positions, d), "embed_positions.weight")?,
                layers,
                ln: ln(&vb.pp("layer_norm"), d)?,
                n_head: cfg.encoder_attention_heads,
                d,
            })
        }

        fn forward(&self, mel: &Tensor) -> candle_core::Result<Tensor> {
            let x = self.conv1.forward(mel)?.gelu()?;
            let x = self.conv2.forward(&x)?.gelu()?;
            let x = x.transpose(1, 2)?;
            let n = x.dim(1)?;
            let mut x = x.broadcast_add(&self.pos.narrow(0, 0, n)?)?;
            let dh = (self.d / self.n_head) as f64;
            for l in &self.layers {
                let h = l.sa_ln.forward(&x)?;
                let q = heads(&l.sa.q.forward(&h)?, self.n_head, self.d)?;
                let k = heads(&l.sa.k.forward(&h)?, self.n_head, self.d)?;
                let v = heads(&l.sa.v.forward(&h)?, self.n_head, self.d)?;
                let qk = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * (1.0 / dh.sqrt()))?;
                let w = candle_nn::ops::softmax_last_dim(&qk)?;
                let (b, _, nn, _) = q.dims4()?;
                let a = w.matmul(&v)?.transpose(1, 2)?.reshape((b, nn, self.d))?;
                x = (&x + l.sa.o.forward(&a)?)?;
                let h = l.mlp_ln.forward(&x)?;
                x = (&x + l.fc2.forward(&l.fc1.forward(&h)?.gelu()?)?)?;
            }
            self.ln.forward(&x)
        }
    }

    struct DLayer {
        sa: Attn,
        sa_ln: LayerNorm,
        ca: Attn,
        ca_ln: LayerNorm,
        fc1: Linear,
        fc2: Linear,
        mlp_ln: LayerNorm,
    }
    struct Decoder {
        emb: Embedding,
        pos: Tensor,
        layers: Vec<DLayer>,
        ln: LayerNorm,
        n_head: usize,
        d: usize,
        sk: Vec<Option<Tensor>>,
        sv: Vec<Option<Tensor>>,
        ck: Vec<Tensor>,
        cv: Vec<Tensor>,
        len: usize,
        /// (capa, cabeza) cuya atención cruzada se captura (pre-softmax)
        capture: Vec<(usize, usize)>,
        /// por cabeza capturada: [tokens][frames]
        captured: Vec<Vec<Vec<f32>>>,
    }

    impl Decoder {
        fn load(vb: VarBuilder, cfg: &Config) -> candle_core::Result<Decoder> {
            let d = cfg.d_model;
            let emb = Embedding::new(vb.get((cfg.vocab_size, d), "embed_tokens.weight")?, d);
            let pos = vb.get((cfg.max_target_positions, d), "embed_positions.weight")?;
            let mut layers = Vec::new();
            for i in 0..cfg.decoder_layers {
                let l = vb.pp(format!("layers.{i}"));
                layers.push(DLayer {
                    sa: Attn::load(l.pp("self_attn"), d)?,
                    sa_ln: ln(&l.pp("self_attn_layer_norm"), d)?,
                    ca: Attn::load(l.pp("encoder_attn"), d)?,
                    ca_ln: ln(&l.pp("encoder_attn_layer_norm"), d)?,
                    fc1: lin(&l.pp("fc1"), d, d * 4, true)?,
                    fc2: lin(&l.pp("fc2"), d * 4, d, true)?,
                    mlp_ln: ln(&l.pp("final_layer_norm"), d)?,
                });
            }
            let n = layers.len();
            Ok(Decoder {
                emb,
                pos,
                layers,
                ln: ln(&vb.pp("layer_norm"), d)?,
                n_head: cfg.decoder_attention_heads,
                d,
                sk: vec![None; n],
                sv: vec![None; n],
                ck: vec![],
                cv: vec![],
                len: 0,
                capture: vec![],
                captured: vec![],
            })
        }

        fn reset(&mut self, xa: &Tensor) -> candle_core::Result<()> {
            self.ck.clear();
            self.cv.clear();
            for l in &self.layers {
                self.ck.push(heads(&l.ca.k.forward(xa)?, self.n_head, self.d)?);
                self.cv.push(heads(&l.ca.v.forward(xa)?, self.n_head, self.d)?);
            }
            for s in self.sk.iter_mut() {
                *s = None;
            }
            for s in self.sv.iter_mut() {
                *s = None;
            }
            self.len = 0;
            self.captured.clear();
            Ok(())
        }

        fn attend(&self, q: &Tensor, k: &Tensor, v: &Tensor, mask: Option<&Tensor>) -> candle_core::Result<(Tensor, Tensor)> {
            let dh = (self.d / self.n_head) as f64;
            let qk = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * (1.0 / dh.sqrt()))?;
            let qk = match mask {
                Some(m) => qk.broadcast_add(m)?,
                None => qk,
            };
            let w = candle_nn::ops::softmax_last_dim(&qk)?;
            let (b, _, n, _) = q.dims4()?;
            Ok((w.matmul(v)?.transpose(1, 2)?.reshape((b, n, self.d))?, qk))
        }

        /// Procesa tokens nuevos y devuelve los logits del último.
        fn step(&mut self, toks: &[u32], dev: &Device) -> candle_core::Result<Vec<f32>> {
            let n = toks.len();
            let t = Tensor::new(toks, dev)?.unsqueeze(0)?;
            let mut x = self.emb.forward(&t)?.broadcast_add(&self.pos.narrow(0, self.len, n)?)?;
            let total = self.len + n;
            let past = self.len;
            let mask = if n > 1 {
                let m: Vec<f32> = (0..n)
                    .flat_map(|i| (0..total).map(move |j| if j > past + i { f32::NEG_INFINITY } else { 0.0 }))
                    .collect();
                Some(Tensor::from_vec(m, (n, total), dev)?)
            } else {
                None
            };
            for li in 0..self.layers.len() {
                let h = self.layers[li].sa_ln.forward(&x)?;
                let q = heads(&self.layers[li].sa.q.forward(&h)?, self.n_head, self.d)?;
                let k_new = heads(&self.layers[li].sa.k.forward(&h)?, self.n_head, self.d)?;
                let v_new = heads(&self.layers[li].sa.v.forward(&h)?, self.n_head, self.d)?;
                let k = match &self.sk[li] {
                    Some(p) => Tensor::cat(&[p, &k_new], 2)?,
                    None => k_new,
                };
                let v = match &self.sv[li] {
                    Some(p) => Tensor::cat(&[p, &v_new], 2)?,
                    None => v_new,
                };
                let (a, _) = self.attend(&q, &k, &v, mask.as_ref())?;
                self.sk[li] = Some(k);
                self.sv[li] = Some(v);
                x = (&x + self.layers[li].sa.o.forward(&a)?)?;
                let h = self.layers[li].ca_ln.forward(&x)?;
                let q = heads(&self.layers[li].ca.q.forward(&h)?, self.n_head, self.d)?;
                let (a, qk) = self.attend(&q, &self.ck[li], &self.cv[li], None)?;
                for &(cl, ch) in self.capture.clone().iter() {
                    if cl == li {
                        let m: Vec<Vec<f32>> = qk.i((0, ch))?.to_vec2()?;
                        self.captured.push(m);
                    }
                }
                x = (&x + self.layers[li].ca.o.forward(&a)?)?;
                let h = self.layers[li].mlp_ln.forward(&x)?;
                let h = self.layers[li].fc2.forward(&self.layers[li].fc1.forward(&h)?.gelu()?)?;
                x = (&x + h)?;
            }
            self.len = total;
            let last = self.ln.forward(&x.narrow(1, n - 1, 1)?)?.squeeze(0)?;
            let logits = last.matmul(&self.emb.embeddings().t()?)?;
            logits.squeeze(0)?.to_vec1()
        }
    }

    // ------------------------------------------------------------ modelo completo

    struct Model {
        name: String,
        cfg: Config,
        tok: Tok,
        enc: Encoder,
        dec: Decoder,
        filters: Vec<f32>,
        heads: Vec<(usize, usize)>,
        dev: Device,
    }

    static CACHE: Mutex<Option<Model>> = Mutex::new(None);

    fn with_model<R>(name: &str, log: &dyn Fn(&str), f: impl FnOnce(&mut Model) -> Result<R>) -> Result<R> {
        let mut g = CACHE.lock().unwrap();
        if g.as_ref().map(|m| m.name != name).unwrap_or(true) {
            *g = None;
            *g = Some(Model::load(name, log)?);
        }
        f(g.as_mut().unwrap())
    }

    impl Model {
        fn load(name: &str, log: &dyn Fn(&str)) -> Result<Model> {
            let repo = repo_for(name)?;
            let dir = models_dir().join(repo.replace('/', "--"));
            let cfg_p = fetch(repo, "config.json", &dir, log)?;
            let tok_p = fetch(repo, "tokenizer.json", &dir, log)?;
            let w_p = fetch(repo, "model.safetensors", &dir, log)?;
            let gen_p = fetch(repo, "generation_config.json", &dir, log).ok();
            let cfg: Config = serde_json::from_str(&std::fs::read_to_string(&cfg_p)?)?;
            let tok = Tok::load(&tok_p, cfg.vocab_size)?;
            let dev = Device::Cpu;
            let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[w_p], m::DTYPE, &dev)? };
            let enc = Encoder::load(vb.pp("model.encoder"), &cfg)?;
            let dec = Decoder::load(vb.pp("model.decoder"), &cfg)?;
            let mut heads: Vec<(usize, usize)> = gen_p
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["alignment_heads"].as_array().cloned())
                .map(|a| a.iter().filter_map(|x| Some((x[0].as_u64()? as usize, x[1].as_u64()? as usize))).collect())
                .unwrap_or_default();
            if heads.is_empty() {
                // respaldo: todas las cabezas de la mitad superior
                for l in cfg.decoder_layers / 2..cfg.decoder_layers {
                    for h in 0..cfg.decoder_attention_heads {
                        heads.push((l, h));
                    }
                }
            }
            let filters = mel_filters(cfg.num_mel_bins);
            Ok(Model { name: name.to_string(), cfg, tok, enc, dec, filters, heads, dev })
        }

        /// Espectrograma de una ventana (rellena a 30 s).
        fn mel_window(&self, pcm: &[f32]) -> Result<Tensor> {
            let n_mels = self.cfg.num_mel_bins;
            let mel = audio::pcm_to_mel(&self.cfg, pcm, &self.filters);
            let n_frames = mel.len() / n_mels;
            let mel = Tensor::from_vec(mel, (1, n_mels, n_frames), &self.dev)?;
            let take = m::N_FRAMES.min(n_frames);
            let mel = mel.narrow(2, 0, take)?;
            Ok(if take < m::N_FRAMES {
                let pad = Tensor::zeros((1, n_mels, m::N_FRAMES - take), DType::F32, &self.dev)?;
                Tensor::cat(&[&mel, &pad], 2)?
            } else {
                mel
            })
        }

        fn lang_id(&self, lang: &str) -> Result<u32> {
            self.tok.id(&format!("<|{lang}|>")).map_err(|_| anyhow::anyhow!("idioma '{lang}' no soportado"))
        }

        /// Alinea `words` (texto) dentro de una ventana de audio (≤ 30 s). Devuelve (inicio, fin, confianza) relativos.
        fn align_window(&mut self, pcm: &[f32], words: &[String], lang: &str) -> Result<Option<Vec<(f64, f64, f32)>>> {
            if words.is_empty() {
                return Ok(Some(vec![]));
            }
            // tokens por palabra (cada palabra empieza con espacio)
            let mut text_tokens: Vec<u32> = Vec::new();
            let mut word_tok_counts: Vec<usize> = Vec::new();
            for w in words {
                let t = self.tok.encode(&format!(" {w}"));
                if t.is_empty() {
                    return Ok(None);
                }
                word_tok_counts.push(t.len());
                text_tokens.extend(t);
            }
            let sot = self.tok.id(m::SOT_TOKEN)?;
            let tr = self.tok.id(m::TRANSCRIBE_TOKEN)?;
            let no_ts = self.tok.id(m::NO_TIMESTAMPS_TOKEN)?;
            let eot = self.tok.id(m::EOT_TOKEN)?;
            let lid = self.lang_id(lang)?;
            let mut toks = vec![sot, lid, tr, no_ts];
            let sot_len = 3;
            toks.extend(&text_tokens);
            toks.push(eot);
            if toks.len() >= self.cfg.max_target_positions {
                bail!("ventana con demasiado texto ({} tokens)", toks.len());
            }
            let mel = self.mel_window(pcm)?;
            let feats = self.enc.forward(&mel)?;
            self.dec.reset(&feats)?;
            self.dec.capture = self.heads.clone();
            let _ = self.dec.step(&toks, &self.dev)?;
            self.dec.capture.clear();
            let caps = std::mem::take(&mut self.dec.captured);
            // frames de codificador con audio real (20 ms cada uno)
            let n_frames = ((pcm.len() as f64 / m::SAMPLE_RATE as f64) / 0.02).ceil() as usize;
            let n_frames = n_frames.clamp(1, 1500);
            let t_rows = toks.len();
            // matriz media de las cabezas, normalizada
            let mut matrix = vec![vec![0f32; n_frames]; t_rows];
            for cap in &caps {
                // softmax por fila sobre los frames válidos
                let mut wts: Vec<Vec<f32>> = cap
                    .iter()
                    .map(|row| {
                        let r = &row[..n_frames];
                        let mx = r.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                        let ex: Vec<f32> = r.iter().map(|&x| (x - mx).exp()).collect();
                        let s: f32 = ex.iter().sum();
                        ex.into_iter().map(|e| e / s).collect()
                    })
                    .collect();
                // normalizar cada frame sobre los tokens (media/desviación)
                for f in 0..n_frames {
                    let mean = wts.iter().map(|r| r[f]).sum::<f32>() / t_rows as f32;
                    let var = wts.iter().map(|r| (r[f] - mean).powi(2)).sum::<f32>() / t_rows as f32;
                    let sd = var.sqrt().max(1e-8);
                    for r in wts.iter_mut() {
                        r[f] = (r[f] - mean) / sd;
                    }
                }
                // filtro de mediana de ancho 7 sobre los frames
                for (ti, r) in wts.iter().enumerate() {
                    let med = median_filter(r, 7);
                    for f in 0..n_frames {
                        matrix[ti][f] += med[f] / caps.len() as f32;
                    }
                }
            }
            // filas que predicen cada token de texto: entradas [no_ts, texto...] (sin sot y sin la última)
            let rows: Vec<Vec<f32>> = matrix[sot_len..t_rows - 1].to_vec();
            let (ti, fi) = dtw(&rows);
            // tiempos de salto: primer frame de cada fila
            let mut jump = vec![f64::NAN; rows.len()];
            let mut score_acc = vec![(0f32, 0usize); rows.len()];
            for (k, (&r, &f)) in ti.iter().zip(fi.iter()).enumerate() {
                if k == 0 || ti[k - 1] != r {
                    jump[r] = f as f64 * 0.02;
                }
                score_acc[r].0 += rows[r][f];
                score_acc[r].1 += 1;
            }
            for r in 1..jump.len() {
                if jump[r].is_nan() {
                    jump[r] = jump[r - 1];
                }
            }
            // límites de palabra: fila k predice el token k; la última fila predice eot = fin
            let mut out = Vec::with_capacity(words.len());
            let mut b = 0usize;
            for &cnt in &word_tok_counts {
                let s = jump[b];
                let e = jump[(b + cnt).min(jump.len() - 1)];
                let (sc, n) = (b..b + cnt).fold((0f32, 0usize), |acc, r| (acc.0 + score_acc[r].0, acc.1 + score_acc[r].1));
                let z = if n > 0 { sc / n as f32 } else { 0.0 };
                if std::env::var("MONTAJE_DEBUG_ALIGN").is_ok() {
                    eprintln!("[align] {:>14} {:.2}-{:.2} z={:.2}", words[out.len()], s, e, z);
                }
                // z típico: 7–9 en palabras claras, < 4 en dudosas
                out.push((s, e.max(s + 0.02), ((z - 2.0) / 4.0).clamp(0.0, 1.0)));
                b += cnt;
            }
            // palabras demasiado largas (silencio absorbido): recortar el inicio
            let mut durs: Vec<f64> = out.iter().map(|w| w.1 - w.0).collect();
            durs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = durs[durs.len() / 2].max(0.08);
            let max_d = (med * 3.0).max(0.7);
            for w in out.iter_mut() {
                if w.1 - w.0 > max_d {
                    w.0 = w.1 - max_d;
                }
            }
            Ok(Some(out))
        }

        /// Transcripción con marcas de tiempo por segmento (búsqueda voraz con reglas de tiempo).
        fn transcribe_pcm(&mut self, pcm: &[f32], from: f64, lang: Option<&str>, log: &dyn Fn(&str)) -> Result<(Vec<TSegment>, String)> {
            let n_mels = self.cfg.num_mel_bins;
            let mel = audio::pcm_to_mel(&self.cfg, pcm, &self.filters);
            let n_frames = mel.len() / n_mels;
            let mel = Tensor::from_vec(mel, (1, n_mels, n_frames), &self.dev)?;
            let content_frames = (pcm.len() / m::HOP_LENGTH).min(n_frames);
            let sot = self.tok.id(m::SOT_TOKEN)?;
            let transcribe = self.tok.id(m::TRANSCRIBE_TOKEN)?;
            let eot = self.tok.id(m::EOT_TOKEN)?;
            let no_ts = self.tok.id(m::NO_TIMESTAMPS_TOKEN)?;
            let ts_begin = self.tok.special.get("<|0.00|>").copied().unwrap_or(no_ts + 1);
            let vocab = self.cfg.vocab_size;
            let mut suppress = vec![false; vocab];
            for &t in &self.cfg.suppress_tokens {
                if (t as usize) < vocab {
                    suppress[t as usize] = true;
                }
            }
            suppress[no_ts as usize] = true;
            for (s, &id) in &self.tok.special {
                if (id as usize) < vocab && id < ts_begin && id != eot && s.starts_with("<|") {
                    suppress[id as usize] = true;
                }
            }
            let mut lang_code = lang.map(String::from);
            let mut segments: Vec<TSegment> = Vec::new();
            let mut seek = 0usize;
            let t0 = std::time::Instant::now();
            while seek < content_frames {
                let seg_size = (content_frames - seek).min(m::N_FRAMES);
                let offset = from + (seek * m::HOP_LENGTH) as f64 / m::SAMPLE_RATE as f64;
                let take = m::N_FRAMES.min(n_frames - seek);
                let mel_seg = mel.narrow(2, seek, take)?;
                let mel_seg = if take < m::N_FRAMES {
                    let pad = Tensor::zeros((1, n_mels, m::N_FRAMES - take), DType::F32, &self.dev)?;
                    Tensor::cat(&[&mel_seg, &pad], 2)?
                } else {
                    mel_seg
                };
                let feats = self.enc.forward(&mel_seg)?;
                self.dec.reset(&feats)?;
                let mut next_logits = if lang_code.is_none() {
                    let logits = self.dec.step(&[sot], &self.dev)?;
                    let mut best = ("es", f32::NEG_INFINITY);
                    for l in LANGS {
                        if let Some(&id) = self.tok.special.get(&format!("<|{l}|>")) {
                            if logits[id as usize] > best.1 {
                                best = (l, logits[id as usize]);
                            }
                        }
                    }
                    lang_code = Some(best.0.to_string());
                    log(&format!("idioma detectado: {}", best.0));
                    let lid = self.lang_id(best.0)?;
                    self.dec.step(&[lid, transcribe], &self.dev)?
                } else {
                    let lid = self.lang_id(lang_code.as_deref().unwrap())?;
                    self.dec.step(&[sot, lid, transcribe], &self.dev)?
                };
                let plen = 3;
                let mut sampled: Vec<u32> = Vec::new();
                let max_ts = ((seg_size as f64 * m::HOP_LENGTH as f64 / m::SAMPLE_RATE as f64) / 0.02) as u32;
                let mut sum_lp = 0.0;
                for _ in 0..self.cfg.max_target_positions / 2 - plen {
                    let mut lg = std::mem::take(&mut next_logits);
                    lg.truncate(vocab);
                    for (k, s) in suppress.iter().enumerate() {
                        if *s {
                            lg[k] = f32::NEG_INFINITY;
                        }
                    }
                    let last_ts = sampled.last().map(|&t| t >= ts_begin).unwrap_or(false);
                    let pen_ts = sampled.len() < 2 || sampled[sampled.len() - 2] >= ts_begin;
                    if last_ts {
                        if pen_ts {
                            for v in lg[ts_begin as usize..].iter_mut() {
                                *v = f32::NEG_INFINITY;
                            }
                        } else {
                            for v in lg[..eot as usize].iter_mut() {
                                *v = f32::NEG_INFINITY;
                            }
                        }
                    }
                    if let Some(&lt) = sampled.iter().rev().find(|&&t| t >= ts_begin) {
                        let upto = if last_ts && !pen_ts { lt } else { lt + 1 };
                        for v in lg[ts_begin as usize..(upto as usize).min(vocab)].iter_mut() {
                            *v = f32::NEG_INFINITY;
                        }
                    }
                    if sampled.is_empty() {
                        for v in lg[..ts_begin as usize].iter_mut() {
                            *v = f32::NEG_INFINITY;
                        }
                        let lim = (ts_begin + 50 + 1) as usize;
                        for v in lg[lim.min(vocab)..].iter_mut() {
                            *v = f32::NEG_INFINITY;
                        }
                    }
                    let lim = (ts_begin + max_ts + 1) as usize;
                    if lim < vocab {
                        for v in lg[lim..].iter_mut() {
                            *v = f32::NEG_INFINITY;
                        }
                    }
                    {
                        let mx = lg.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
                        let ts_sum: f64 = lg[ts_begin as usize..].iter().map(|&x| ((x as f64) - mx).exp()).sum();
                        let txt_max = lg[..ts_begin as usize].iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
                        if ts_sum > 0.0 && ts_sum.ln() + mx > txt_max {
                            for v in lg[..ts_begin as usize].iter_mut() {
                                *v = f32::NEG_INFINITY;
                            }
                        }
                    }
                    let next = argmax(&lg);
                    sum_lp += log_softmax_at(&lg, next as usize);
                    if next == eot || self.dec.len + 1 >= self.cfg.max_target_positions {
                        break;
                    }
                    sampled.push(next);
                    next_logits = self.dec.step(&[next], &self.dev)?;
                }
                let avg_lp = sum_lp / sampled.len().max(1) as f64;
                let mut cur: Vec<u32> = Vec::new();
                let mut seg_start: Option<f64> = None;
                let mut last_end = 0.0f64;
                let mut found = Vec::new();
                for &t in &sampled {
                    if t >= ts_begin {
                        let ts = (t - ts_begin) as f64 * 0.02;
                        match seg_start {
                            None => seg_start = Some(ts),
                            Some(s) => {
                                if !cur.is_empty() {
                                    found.push((s, ts, self.tok.decode(&cur)));
                                    last_end = ts;
                                }
                                cur.clear();
                                seg_start = Some(ts);
                            }
                        }
                    } else {
                        cur.push(t);
                    }
                }
                if !cur.is_empty() {
                    let s = seg_start.unwrap_or(last_end);
                    let e = seg_size as f64 / 100.0;
                    found.push((s, e, self.tok.decode(&cur)));
                    last_end = e;
                }
                if avg_lp > -1.6 {
                    for (s, e, txt) in found {
                        let text = txt.split_whitespace().collect::<Vec<_>>().join(" ");
                        if text.is_empty() || segments.last().map(|p| p.text == text).unwrap_or(false) {
                            continue;
                        }
                        segments.push(TSegment { start: r3(offset + s), end: r3(offset + e.max(s + 0.1)), text, words: vec![] });
                    }
                }
                let adv = if last_end > 1.0 && last_end < seg_size as f64 / 100.0 - 0.5 { (last_end * 100.0) as usize } else { seg_size };
                seek += adv.max(100);
                log(&format!(
                    "  {:.0}/{:.0} s transcritos ({:.1} s)",
                    (seek as f64 / 100.0).min(pcm.len() as f64 / 16000.0),
                    pcm.len() as f64 / 16000.0,
                    t0.elapsed().as_secs_f64()
                ));
            }
            Ok((segments, lang_code.unwrap_or_else(|| "es".into())))
        }
    }

    fn median_filter(x: &[f32], width: usize) -> Vec<f32> {
        let n = x.len();
        let h = width / 2;
        if n <= h {
            return x.to_vec();
        }
        // relleno por reflexión
        let get = |i: i64| -> f32 {
            let mut i = i;
            if i < 0 {
                i = -i;
            }
            if i >= n as i64 {
                i = 2 * (n as i64 - 1) - i;
            }
            x[i.clamp(0, n as i64 - 1) as usize]
        };
        let mut out = vec![0f32; n];
        let mut buf = vec![0f32; width];
        for i in 0..n {
            for k in 0..width {
                buf[k] = get(i as i64 + k as i64 - h as i64);
            }
            buf.sort_by(|a, b| a.partial_cmp(b).unwrap());
            out[i] = buf[h];
        }
        out
    }

    /// DTW sobre coste = -matriz (filas = tokens, columnas = frames). Devuelve (índices de fila, índices de frame).
    fn dtw(m: &[Vec<f32>]) -> (Vec<usize>, Vec<usize>) {
        let n = m.len();
        let mm = m[0].len();
        let w = mm + 1;
        let mut cost = vec![f32::INFINITY; (n + 1) * w];
        let mut trace = vec![-1i8; (n + 1) * w];
        cost[0] = 0.0;
        for j in 1..=mm {
            for i in 1..=n {
                let c0 = cost[(i - 1) * w + j - 1];
                let c1 = cost[(i - 1) * w + j];
                let c2 = cost[i * w + j - 1];
                let (c, t) = if c0 < c1 && c0 < c2 {
                    (c0, 0)
                } else if c1 < c0 && c1 < c2 {
                    (c1, 1)
                } else {
                    (c2, 2)
                };
                cost[i * w + j] = -m[i - 1][j - 1] + c;
                trace[i * w + j] = t;
            }
        }
        for j in 0..=mm {
            trace[j] = 2;
        }
        for i in 0..=n {
            trace[i * w] = 1;
        }
        let (mut i, mut j) = (n, mm);
        let mut path = Vec::new();
        while i > 0 || j > 0 {
            path.push((i - 1, j.saturating_sub(1)));
            match trace[i * w + j] {
                0 => {
                    i -= 1;
                    j -= 1;
                }
                1 => i -= 1,
                _ => j -= 1,
            }
            if i == 0 {
                break;
            }
        }
        path.reverse();
        (path.iter().map(|p| p.0).collect(), path.iter().map(|p| p.1).collect())
    }

    const LANGS: &[&str] = &[
        "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
    ];

    fn argmax(v: &[f32]) -> u32 {
        let mut best = 0usize;
        for i in 1..v.len() {
            if v[i] > v[best] {
                best = i;
            }
        }
        best as u32
    }

    fn log_softmax_at(v: &[f32], idx: usize) -> f64 {
        let mx = v.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as f64;
        let s: f64 = v.iter().map(|&x| ((x as f64) - mx).exp()).sum();
        v[idx] as f64 - mx - s.ln()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn tok_roundtrip_es() {
            let p = models_dir().join("openai--whisper-small").join("tokenizer.json");
            if !p.exists() {
                eprintln!("sin tokenizer descargado; se omite");
                return;
            }
            let tok = Tok::load(&p, 51865).unwrap();
            for s in [
                " Cállate por la verga, puto",
                " ¡Hijo de perra! ¿Nacionalidad?",
                " yo soy de otro planeta… 1990 MCs",
                " Azuky vs. Chuty: el niño, la montaña y el pingüino",
            ] {
                let ids = tok.encode(s);
                assert!(!ids.is_empty());
                assert_eq!(tok.decode(&ids), s, "ida y vuelta de {s:?} -> {ids:?}");
            }
            // "Ġthe" es un único token en el vocabulario GPT-2
            assert_eq!(tok.encode(" the").len(), 1);
        }

        #[test]
        fn dtw_diagonal() {
            // matriz identidad estirada: 3 tokens x 6 frames
            let m: Vec<Vec<f32>> = (0..3).map(|i| (0..6).map(|j| if j / 2 == i { 1.0 } else { -1.0 }).collect()).collect();
            let (ti, fi) = dtw(&m);
            let starts: Vec<usize> = (0..3).map(|r| fi[ti.iter().position(|&x| x == r).unwrap()]).collect();
            assert_eq!(starts, vec![0, 2, 4]);
        }
    }

    // ------------------------------------------------------------ API

    /// Agrupa segmentos en ventanas de ≤ 28 s para alinearlos con su contexto.
    fn windows(segs: &[TSegment], only: Option<&[usize]>) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        let mut cur: Vec<usize> = Vec::new();
        let mut cur_start = 0.0;
        let mut cur_tokens = 0usize;
        for (i, s) in segs.iter().enumerate() {
            if let Some(o) = only {
                if !o.contains(&i) {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        cur_tokens = 0;
                    }
                    continue;
                }
            }
            let words = s.text.split_whitespace().count();
            if cur.is_empty() {
                cur_start = s.start;
            }
            if !cur.is_empty() && (s.end - cur_start > 27.0 || cur_tokens + words * 3 > 380) {
                out.push(std::mem::take(&mut cur));
                cur_start = s.start;
                cur_tokens = 0;
            }
            cur.push(i);
            cur_tokens += words * 3;
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// Alinea (o realinea) las palabras de una transcripción con la voz. `only` limita a ciertos segmentos.
    pub fn align(path: &str, t: &mut Transcript, only: Option<&[usize]>, o: &Opts) -> Result<()> {
        let info = media::probe(path)?;
        let lang = o.lang.map(String::from).unwrap_or_else(|| if t.language.is_empty() { "es".into() } else { t.language.clone() });
        let groups = windows(&t.segments, only);
        let total = groups.len();
        with_model(o.model, o.log, |mdl| {
            for (gi, g) in groups.iter().enumerate() {
                let first = &t.segments[g[0]];
                let last = &t.segments[*g.last().unwrap()];
                // no empezar antes de la última palabra del segmento anterior (evita solapes entre ventanas)
                let prev_end = if g[0] > 0 { t.segments[g[0] - 1].words.last().map(|w| w.end).unwrap_or(0.0) } else { 0.0 };
                let ws = (first.start - 0.6).max(0.0).max((prev_end - 0.02).min(first.start));
                let we = (last.end + 0.6).min(ws + 29.5).min(info.duration);
                let pcm = media::decode_audio_mono(path, ws, Some((we - ws).max(0.2)), m::SAMPLE_RATE as u32)?;
                let words: Vec<String> = g.iter().flat_map(|&i| split_words(&t.segments[i].text)).collect();
                let res = mdl.align_window(&pcm, &words, &lang)?;
                let mut k = 0usize;
                for &si in g {
                    let seg = &mut t.segments[si];
                    let ws_words = split_words(&seg.text);
                    let old = seg.words.clone();
                    let mut new: Vec<Word> = match &res {
                        Some(r) => ws_words
                            .iter()
                            .enumerate()
                            .map(|(j, w)| {
                                let (s, e, sc) = r[k + j];
                                Word { word: w.clone(), start: r3(ws + s), end: r3(ws + e), locked: false, score: Some((sc * 100.0).round() / 100.0) }
                            })
                            .collect(),
                        None => proportional_words(seg),
                    };
                    k += ws_words.len();
                    // conservar fijaciones si el texto no cambió
                    if old.len() == new.len() && old.iter().zip(new.iter()).all(|(a, b)| a.word == b.word) {
                        apply_locks(&old, &mut new);
                    }
                    if let (Some(f), Some(l)) = (new.first(), new.last()) {
                        seg.start = f.start;
                        seg.end = l.end.max(f.start + 0.05);
                    }
                    seg.words = new;
                }
                (o.log)(&format!("  alineado {}/{} ventanas", gi + 1, total));
            }
            Ok(())
        })?;
        t.alignment = "dtw".into();
        if t.model.is_empty() {
            t.model = o.model.to_string();
        }
        Ok(())
    }

    pub fn transcribe(path: &str, from: f64, to: Option<f64>, o: &Opts) -> Result<Transcript> {
        let info = media::probe(path)?;
        let to = to.unwrap_or(info.duration).min(info.duration);
        let pcm = media::decode_audio_mono(path, from, Some((to - from).max(0.1)), m::SAMPLE_RATE as u32)?;
        let (segments, lang) = with_model(o.model, o.log, |mdl| mdl.transcribe_pcm(&pcm, from, o.lang, o.log))?;
        let mut t = Transcript {
            source: path.to_string(),
            model: o.model.to_string(),
            language: lang.clone(),
            from,
            to,
            alignment: String::new(),
            segments,
        };
        let ol = Opts { model: o.model, lang: Some(&lang), log: o.log };
        align(path, &mut t, None, &ol)?;
        Ok(t)
    }

    /// Crea una transcripción a partir de un texto conocido (letra) para el tramo [from, to] (≤ 29 s por ventana).
    /// Cada línea del texto se convierte en un segmento.
    pub fn align_text(path: &str, from: f64, to: f64, text: &str, o: &Opts) -> Result<Transcript> {
        if to - from > 29.5 {
            bail!("para textos sin tiempos el tramo debe durar ≤ 29 s; divide la letra en tramos o transcribe primero y corrige el texto");
        }
        let lang = o.lang.unwrap_or("es").to_string();
        let lines: Vec<String> = text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect();
        let all: Vec<String> = lines.iter().flat_map(|l| split_words(l)).collect();
        let pcm = media::decode_audio_mono(path, from, Some(to - from), m::SAMPLE_RATE as u32)?;
        let res = with_model(o.model, o.log, |mdl| mdl.align_window(&pcm, &all, &lang))?
            .ok_or_else(|| anyhow::anyhow!("no se pudo tokenizar el texto"))?;
        let mut segments = Vec::new();
        let mut k = 0;
        for l in &lines {
            let ws = split_words(l);
            let words: Vec<Word> = ws
                .iter()
                .enumerate()
                .map(|(j, w)| {
                    let (s, e, sc) = res[k + j];
                    Word { word: w.clone(), start: r3(from + s), end: r3(from + e), locked: false, score: Some((sc * 100.0).round() / 100.0) }
                })
                .collect();
            k += ws.len();
            if let (Some(f), Some(la)) = (words.first(), words.last()) {
                segments.push(TSegment { start: f.start, end: la.end, text: l.clone(), words });
            }
        }
        Ok(Transcript {
            source: path.to_string(),
            model: o.model.to_string(),
            language: lang,
            from,
            to,
            alignment: "dtw".into(),
            segments,
        })
    }
}
