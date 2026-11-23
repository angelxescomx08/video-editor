//! Utilidades compartidas: tiempos, colores, ruido determinista.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Tiempo en segundos. Acepta números (`12.5`) o cadenas (`"1:23.5"`, `"83.5s"`, `"500ms"`).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Default)]
pub struct Time(pub f64);

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::Number(n) => Ok(Time(n.as_f64().unwrap_or(0.0))),
            // ancla a palabra ("@montaña"): se resuelve antes (anchors.rs); aquí solo se valida la sintaxis
            serde_json::Value::String(s) if s.trim_start().starts_with('@') => Ok(Time(f64::NAN)),
            serde_json::Value::String(s) => parse_time(&s).map(Time).map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "tiempo inválido {other}: usa segundos (12.5) o \"m:ss.s\""
            ))),
        }
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(self.0)
    }
}

pub fn parse_time(s: &str) -> Result<f64, String> {
    let t = s.trim();
    let (t, mult) = if let Some(x) = t.strip_suffix("ms") {
        (x, 0.001)
    } else if let Some(x) = t.strip_suffix('s') {
        (x, 1.0)
    } else {
        (t, 1.0)
    };
    let parts: Vec<&str> = t.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err(format!("tiempo inválido '{s}'"));
    }
    let mut total = 0.0;
    for p in &parts {
        let v: f64 = p
            .trim()
            .parse()
            .map_err(|_| format!("tiempo inválido '{s}': usa segundos (12.5) o \"m:ss.s\""))?;
        total = total * 60.0 + v;
    }
    Ok(total * mult)
}

/// Un valor o una lista de valores (`"at": 3.2` o `"at": [3.2, 4.1]`).
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for OneOrMany<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        if let serde_json::Value::Array(items) = v {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(serde_json::from_value(it).map_err(serde::de::Error::custom)?);
            }
            Ok(OneOrMany::Many(out))
        } else {
            Ok(OneOrMany::One(serde_json::from_value(v).map_err(serde::de::Error::custom)?))
        }
    }
}

impl<T: Clone> OneOrMany<T> {
    pub fn to_vec(&self) -> Vec<T> {
        match self {
            OneOrMany::One(t) => vec![t.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

pub fn one(t: f64) -> Option<OneOrMany<Time>> {
    Some(OneOrMany::One(Time(t)))
}

/// Color en RGBA "recto" (no premultiplicado).
pub fn parse_color(s: &str) -> Result<[u8; 4], String> {
    let t = s.trim().to_lowercase();
    let named = match t.as_str() {
        "white" | "blanco" => Some([255, 255, 255, 255]),
        "black" | "negro" => Some([0, 0, 0, 255]),
        "red" | "rojo" => Some([230, 30, 40, 255]),
        "green" | "verde" => Some([0, 255, 0, 255]),
        "blue" | "azul" => Some([20, 90, 200, 255]),
        "yellow" | "amarillo" => Some([255, 214, 0, 255]),
        "gold" | "dorado" => Some([199, 154, 30, 255]),
        "orange" | "naranja" => Some([255, 120, 0, 255]),
        "purple" | "morado" => Some([123, 44, 255, 255]),
        "pink" | "rosa" => Some([255, 60, 170, 255]),
        "cyan" => Some([0, 220, 255, 255]),
        "magenta" => Some([255, 0, 200, 255]),
        "teal" => Some([14, 124, 134, 255]),
        "gray" | "grey" | "gris" => Some([128, 128, 128, 255]),
        "transparent" => Some([0, 0, 0, 0]),
        _ => None,
    };
    if let Some(c) = named {
        return Ok(c);
    }
    let hex = t.strip_prefix('#').unwrap_or(&t);
    let parse = |h: &str| u8::from_str_radix(h, 16).map_err(|_| format!("color inválido '{s}'"));
    match hex.len() {
        3 => {
            let r = parse(&hex[0..1].repeat(2))?;
            let g = parse(&hex[1..2].repeat(2))?;
            let b = parse(&hex[2..3].repeat(2))?;
            Ok([r, g, b, 255])
        }
        6 => Ok([parse(&hex[0..2])?, parse(&hex[2..4])?, parse(&hex[4..6])?, 255]),
        8 => Ok([parse(&hex[0..2])?, parse(&hex[2..4])?, parse(&hex[4..6])?, parse(&hex[6..8])?]),
        _ => Err(format!("color inválido '{s}' (usa #rrggbb, #rrggbbaa o un nombre)")),
    }
}

pub fn color_or(s: &Option<String>, def: [u8; 4]) -> [u8; 4] {
    s.as_ref().and_then(|c| parse_color(c).ok()).unwrap_or(def)
}

#[inline]
pub fn hash64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Aleatorio determinista en [0,1).
#[inline]
pub fn rand01(seed: u64, i: u64) -> f32 {
    (hash64(seed ^ hash64(i)) >> 40) as f32 / (1u64 << 24) as f32
}

/// Aleatorio determinista en [-1,1).
#[inline]
pub fn rands(seed: u64, i: u64) -> f32 {
    rand01(seed, i) * 2.0 - 1.0
}

/// Ruido 1D suave en [-1,1].
pub fn noise1(seed: u64, t: f32) -> f32 {
    let i = t.floor();
    let f = t - i;
    let a = rands(seed, i as i64 as u64);
    let b = rands(seed, (i as i64 + 1) as u64);
    let u = f * f * (3.0 - 2.0 * f);
    a + (b - a) * u
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn fmt_time(t: f64) -> String {
    let m = (t / 60.0).floor();
    let s = t - m * 60.0;
    format!("{}:{:05.2}", m as i64, s)
}
