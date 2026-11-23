//! Transcripción local con Whisper (candle, Rust puro). Devuelve segmentos con tiempos y palabras.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Word {
    pub word: String,
    pub start: f64,
    pub end: f64,
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
    pub segments: Vec<TSegment>,
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

/// Reparte los tiempos de las palabras dentro de cada segmento y los ajusta a los ataques de la voz.
pub fn assign_words(segments: &mut [TSegment], onsets: &[f64]) {
    for seg in segments.iter_mut() {
        let words: Vec<String> = seg.text.split_whitespace().map(String::from).collect();
        if words.is_empty() {
            continue;
        }
        let weights: Vec<f64> = words.iter().map(|w| w.chars().filter(|c| c.is_alphanumeric()).count().max(1) as f64 + 1.5).collect();
        let total: f64 = weights.iter().sum();
        let span = (seg.end - seg.start).max(0.05);
        let mut t = seg.start;
        let mut out = Vec::with_capacity(words.len());
        let mut prev_start = seg.start - 1.0;
        for (i, w) in words.iter().enumerate() {
            let d = span * weights[i] / total;
            let mut s = t;
            // ajustar al ataque más cercano (±0.12 s) sin romper el orden
            if i > 0 {
                if let Some(&o) = onsets
                    .iter()
                    .filter(|&&o| (o - s).abs() < 0.12 && o > prev_start + 0.06 && o < seg.end - 0.05)
                    .min_by(|a, b| (*a - s).abs().partial_cmp(&(*b - s).abs()).unwrap())
                {
                    s = o;
                }
            }
            prev_start = s;
            out.push(Word { word: w.clone(), start: (s * 1000.0).round() / 1000.0, end: 0.0 });
            t += d;
        }
        for i in 0..out.len() {
            let e = if i + 1 < out.len() { out[i + 1].start } else { seg.end };
            out[i].end = (e * 1000.0).round() / 1000.0;
        }
        seg.words = out;
    }
}

#[cfg(feature = "whisper")]
pub use imp::transcribe;

#[cfg(not(feature = "whisper"))]
pub fn transcribe(_path: &str, _model: &str, _lang: Option<&str>, _from: f64, _to: Option<f64>, _log: &dyn Fn(&str)) -> anyhow::Result<Transcript> {
    anyhow::bail!("esta compilación no incluye transcripción (compila con --features whisper)")
}

#[cfg(feature = "whisper")]
mod imp {
    use super::*;
    use crate::media;
    use anyhow::{bail, Context, Result};
    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::whisper::{self as m, audio, Config};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

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

    struct Tok {
        id_to_bytes: Vec<Vec<u8>>,
        special: HashMap<String, u32>,
    }

    fn byte_decoder() -> HashMap<char, u8> {
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
        bs.into_iter().zip(cs).map(|(b, c)| (char::from_u32(c).unwrap(), b as u8)).collect()
    }

    fn load_tokenizer(path: &Path, vocab_size: usize) -> Result<Tok> {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        let dec = byte_decoder();
        let mut id_to_bytes = vec![Vec::new(); vocab_size.max(52000)];
        if let Some(vocab) = v["model"]["vocab"].as_object() {
            for (s, id) in vocab {
                let id = id.as_u64().unwrap_or(0) as usize;
                if id < id_to_bytes.len() {
                    id_to_bytes[id] = s.chars().filter_map(|c| dec.get(&c).copied()).collect();
                }
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
        Ok(Tok { id_to_bytes, special })
    }

    impl Tok {
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
    }

    const LANGS: &[&str] = &[
        "en", "zh", "de", "es", "ru", "ko", "fr", "ja", "pt", "tr", "pl", "ca", "nl", "ar", "sv", "it", "id", "hi", "fi", "vi", "he", "uk", "el", "ms", "cs", "ro", "da", "hu", "ta", "no", "th", "ur", "hr", "bg", "lt", "la", "mi", "ml", "cy", "sk", "te", "fa", "lv", "bn", "sr", "az", "sl", "kn", "et", "mk", "br", "eu", "is", "hy", "ne", "mn", "bs", "kk", "sq", "sw", "gl", "mr", "pa", "si", "km", "sn", "yo", "so", "af", "oc", "ka", "be", "tg", "sd", "gu", "am", "yi", "lo", "uz", "fo", "ht", "ps", "tk", "nn", "mt", "sa", "lb", "my", "bo", "tl", "mg", "as", "tt", "haw", "ln", "ha", "ba", "jw", "su",
    ];

    // ---------------- decodificador propio con caché KV incremental
    use candle_nn::{Embedding, LayerNorm, Linear, Module};

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
    struct DLayer {
        sa: Attn,
        sa_ln: LayerNorm,
        ca: Attn,
        ca_ln: LayerNorm,
        fc1: Linear,
        fc2: Linear,
        mlp_ln: LayerNorm,
    }
    pub(super) struct Decoder {
        emb: Embedding,
        pos: Tensor,
        layers: Vec<DLayer>,
        ln: LayerNorm,
        n_head: usize,
        d: usize,
        // caché
        sk: Vec<Option<Tensor>>,
        sv: Vec<Option<Tensor>>,
        ck: Vec<Tensor>,
        cv: Vec<Tensor>,
        len: usize,
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
            })
        }

        /// Nueva ventana de audio: calcula K/V cruzados y vacía la caché.
        fn reset(&mut self, xa: &Tensor) -> candle_core::Result<()> {
            self.ck.clear();
            self.cv.clear();
            for l in &self.layers {
                self.ck.push(self.heads(&l.ca.k.forward(xa)?)?);
                self.cv.push(self.heads(&l.ca.v.forward(xa)?)?);
            }
            for s in self.sk.iter_mut() {
                *s = None;
            }
            for s in self.sv.iter_mut() {
                *s = None;
            }
            self.len = 0;
            Ok(())
        }

        fn heads(&self, x: &Tensor) -> candle_core::Result<Tensor> {
            let (b, n, _) = x.dims3()?;
            x.reshape((b, n, self.n_head, self.d / self.n_head))?.transpose(1, 2)?.contiguous()
        }

        fn attend(&self, q: &Tensor, k: &Tensor, v: &Tensor, mask: Option<&Tensor>) -> candle_core::Result<Tensor> {
            let dh = (self.d / self.n_head) as f64;
            let qk = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * (1.0 / dh.sqrt()))?;
            let qk = match mask {
                Some(m) => qk.broadcast_add(m)?,
                None => qk,
            };
            let w = candle_nn::ops::softmax_last_dim(&qk)?;
            let (b, _, n, _) = q.dims4()?;
            w.matmul(v)?.transpose(1, 2)?.reshape((b, n, self.d))
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
                let q = self.heads(&self.layers[li].sa.q.forward(&h)?)?;
                let k_new = self.heads(&self.layers[li].sa.k.forward(&h)?)?;
                let v_new = self.heads(&self.layers[li].sa.v.forward(&h)?)?;
                let k = match &self.sk[li] {
                    Some(p) => Tensor::cat(&[p, &k_new], 2)?,
                    None => k_new,
                };
                let v = match &self.sv[li] {
                    Some(p) => Tensor::cat(&[p, &v_new], 2)?,
                    None => v_new,
                };
                let a = self.attend(&q, &k, &v, mask.as_ref())?;
                self.sk[li] = Some(k);
                self.sv[li] = Some(v);
                x = (&x + self.layers[li].sa.o.forward(&a)?)?;
                let h = self.layers[li].ca_ln.forward(&x)?;
                let q = self.heads(&self.layers[li].ca.q.forward(&h)?)?;
                let a = self.attend(&q, &self.ck[li], &self.cv[li], None)?;
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

    struct ELayer {
        sa: Attn,
        sa_ln: LayerNorm,
        fc1: Linear,
        fc2: Linear,
        mlp_ln: LayerNorm,
    }
    pub(super) struct Encoder {
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

        fn heads(&self, x: &Tensor) -> candle_core::Result<Tensor> {
            let (b, n, _) = x.dims3()?;
            x.reshape((b, n, self.n_head, self.d / self.n_head))?.transpose(1, 2)?.contiguous()
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
                let q = self.heads(&l.sa.q.forward(&h)?)?;
                let k = self.heads(&l.sa.k.forward(&h)?)?;
                let v = self.heads(&l.sa.v.forward(&h)?)?;
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

    pub fn transcribe(path: &str, model: &str, lang: Option<&str>, from: f64, to: Option<f64>, log: &dyn Fn(&str)) -> Result<Transcript> {
        let repo = repo_for(model)?;
        let dir = models_dir().join(repo.replace('/', "--"));
        let cfg_p = fetch(repo, "config.json", &dir, log)?;
        let tok_p = fetch(repo, "tokenizer.json", &dir, log)?;
        let w_p = fetch(repo, "model.safetensors", &dir, log)?;
        let config: Config = serde_json::from_str(&std::fs::read_to_string(&cfg_p)?)?;
        let tok = load_tokenizer(&tok_p, config.vocab_size)?;
        let device = Device::Cpu;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[w_p], m::DTYPE, &device)? };
        let encoder = Encoder::load(vb.pp("model.encoder"), &config)?;
        let mut decoder = Decoder::load(vb.pp("model.decoder"), &config)?;

        let info = media::probe(path)?;
        let to = to.unwrap_or(info.duration).min(info.duration);
        let pcm = media::decode_audio_mono(path, from, Some((to - from).max(0.1)), m::SAMPLE_RATE as u32)?;
        let filters = mel_filters(config.num_mel_bins);
        let mel = audio::pcm_to_mel(&config, &pcm, &filters);
        let n_mels = config.num_mel_bins;
        let n_frames = mel.len() / n_mels;
        let mel = Tensor::from_vec(mel, (1, n_mels, n_frames), &device)?;
        let content_frames = (pcm.len() / m::HOP_LENGTH).min(n_frames);

        let sot = tok.id(m::SOT_TOKEN)?;
        let transcribe = tok.id(m::TRANSCRIBE_TOKEN)?;
        let eot = tok.id(m::EOT_TOKEN)?;
        let no_ts = tok.id(m::NO_TIMESTAMPS_TOKEN)?;
        let ts_begin = tok.special.get("<|0.00|>").copied().unwrap_or(no_ts + 1);
        let vocab = config.vocab_size;
        let mut suppress = vec![false; vocab];
        for &t in &config.suppress_tokens {
            if (t as usize) < vocab {
                suppress[t as usize] = true;
            }
        }
        suppress[no_ts as usize] = true;
        for (s, &id) in &tok.special {
            // suprimir tokens especiales que no son marcas de tiempo ni eot
            if (id as usize) < vocab && id < ts_begin && id != eot && s.starts_with("<|") {
                suppress[id as usize] = true;
            }
        }

        let mut lang_code = lang.map(String::from);
        let mut segments: Vec<TSegment> = Vec::new();
        let mut seek = 0usize;
        let t_start = std::time::Instant::now();
        while seek < content_frames {
            let seg_size = (content_frames - seek).min(m::N_FRAMES);
            let offset = from + (seek * m::HOP_LENGTH) as f64 / m::SAMPLE_RATE as f64;
            // el codificador espera 3000 frames: el espectrograma ya trae silencio de relleno al final
            let take = m::N_FRAMES.min(n_frames - seek);
            let mel_seg = mel.narrow(2, seek, take)?;
            let mel_seg = if take < m::N_FRAMES {
                let pad = Tensor::zeros((1, n_mels, m::N_FRAMES - take), DType::F32, &device)?;
                Tensor::cat(&[&mel_seg, &pad], 2)?
            } else {
                mel_seg
            };
            let feats = encoder.forward(&mel_seg)?;
            decoder.reset(&feats)?;
            // detección de idioma (con el mismo caché)
            let mut next_logits = if lang_code.is_none() {
                let logits = decoder.step(&[sot], &device)?;
                let mut best = ("es", f32::NEG_INFINITY);
                for l in LANGS {
                    if let Some(&id) = tok.special.get(&format!("<|{l}|>")) {
                        if logits[id as usize] > best.1 {
                            best = (l, logits[id as usize]);
                        }
                    }
                }
                lang_code = Some(best.0.to_string());
                log(&format!("idioma detectado: {}", best.0));
                let lang_id = tok.id(&format!("<|{}|>", best.0))?;
                decoder.step(&[lang_id, transcribe], &device)?
            } else {
                let lang_id = tok.id(&format!("<|{}|>", lang_code.as_deref().unwrap_or("es")))?;
                decoder.step(&[sot, lang_id, transcribe], &device)?
            };
            let lang_id = tok.id(&format!("<|{}|>", lang_code.as_deref().unwrap_or("es")))?;
            let prompt = vec![sot, lang_id, transcribe];
            let mut tokens = prompt.clone();
            let max_ts = ((seg_size as f64 * m::HOP_LENGTH as f64 / m::SAMPLE_RATE as f64) / 0.02) as u32;
            let mut sum_lp = 0.0;
            for _ in 0..config.max_target_positions / 2 - prompt.len() {
                let mut lg = std::mem::take(&mut next_logits);
                lg.truncate(vocab);
                for (k, s) in suppress.iter().enumerate() {
                    if *s {
                        lg[k] = f32::NEG_INFINITY;
                    }
                }
                // reglas de marcas de tiempo (como OpenAI)
                let sampled = &tokens[prompt.len()..];
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
                // si la masa de probabilidad de los tiempos supera al mejor texto, forzar tiempo
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
                tokens.push(next);
                if next == eot || decoder.len + 1 >= config.max_target_positions {
                    break;
                }
                next_logits = decoder.step(&[next], &device)?;
            }
            let sampled: Vec<u32> = tokens[prompt.len()..].iter().cloned().filter(|&t| t != eot).collect();
            let avg_lp = sum_lp / sampled.len().max(1) as f64;
            // separar en segmentos por pares de marcas de tiempo
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
                                found.push((s, ts, tok.decode(&cur)));
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
                found.push((s, e, tok.decode(&cur)));
                last_end = e;
            }
            if avg_lp > -1.6 {
                for (s, e, txt) in found {
                    let text = txt.trim().to_string();
                    if text.is_empty() || segments.last().map(|p| p.text == text).unwrap_or(false) {
                        continue;
                    }
                    segments.push(TSegment { start: round3(offset + s), end: round3(offset + e.max(s + 0.1)), text, words: vec![] });
                }
            }
            // avanzar: hasta el último tiempo completo si cae dentro de la ventana
            let adv = if last_end > 1.0 && last_end < seg_size as f64 / 100.0 - 0.5 { (last_end * 100.0) as usize } else { seg_size };
            seek += adv.max(100);
            log(&format!(
                "  {:.0}/{:.0} s transcritos ({:.1} s)",
                (seek as f64 / 100.0).min(to - from),
                to - from,
                t_start.elapsed().as_secs_f64()
            ));
        }
        // tiempos por palabra
        let onsets = speech_onsets(&pcm, from);
        assign_words(&mut segments, &onsets);
        Ok(Transcript {
            source: path.to_string(),
            model: model.to_string(),
            language: lang_code.unwrap_or_default(),
            from,
            to,
            segments,
        })
    }

    fn round3(x: f64) -> f64 {
        (x * 1000.0).round() / 1000.0
    }

    /// Ataques de energía (10 ms) de la voz para afinar los tiempos de palabra.
    fn speech_onsets(pcm: &[f32], offset: f64) -> Vec<f64> {
        let hop = 160usize;
        let n = pcm.len() / hop;
        let mut e = vec![0f32; n];
        let mut lp = crate::audio::Biquad::bandpass(1200.0, 0.6);
        let mut acc = 0f32;
        let mut k = 0;
        for (i, &s) in pcm.iter().enumerate() {
            let y = lp.run(s);
            acc += y * y;
            if (i + 1) % hop == 0 {
                e[k] = (acc / hop as f32).sqrt();
                acc = 0.0;
                k += 1;
                if k >= n {
                    break;
                }
            }
        }
        let mut out = Vec::new();
        for i in 2..n.saturating_sub(1) {
            let d = e[i] - e[i - 2];
            let dp = e[i - 1] - e[i.saturating_sub(3)];
            let dn = e[i + 1] - e[i - 1];
            if d > 0.0 && d >= dp && d >= dn && e[i] > 0.01 {
                out.push(offset + i as f64 * 0.01);
            }
        }
        out
    }
}
