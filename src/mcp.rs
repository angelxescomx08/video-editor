//! Servidor MCP (JSON-RPC 2.0 sobre stdio). Permite que Claude Code, Codex, Kiro y otros agentes
//! usen el editor con la suscripción del usuario, sin claves de API.

use crate::api;
use base64::Engine;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

fn tools() -> Value {
    let project_props = json!({
        "project": {"type": "string", "description": "Ruta al archivo .json del proyecto (recomendado). Las rutas relativas dentro del proyecto se resuelven desde su carpeta."},
        "project_json": {"type": "object", "description": "Alternativa: el proyecto completo en línea."}
    });
    let mut preview_props = project_props.clone();
    preview_props["times"] = json!({"type": "array", "items": {"type": "number"}, "description": "Tiempos de salida (s) a renderizar. Vacío = 6 repartidos."});
    preview_props["scale"] = json!({"type": "number", "description": "Escala de render (0.5 por defecto)."});
    preview_props["columns"] = json!({"type": "integer", "description": "Columnas de la cuadrícula (3)."});
    let mut render_props = project_props.clone();
    render_props["output"] = json!({"type": "string", "description": "Archivo .mp4 de salida (por defecto junto al proyecto)."});
    render_props["draft"] = json!({"type": "boolean", "description": "Borrador rápido a media resolución."});
    render_props["scale"] = json!({"type": "number"});
    render_props["from"] = json!({"type": "number", "description": "Renderizar solo desde este segundo."});
    render_props["to"] = json!({"type": "number", "description": "Renderizar solo hasta este segundo."});
    json!([
        {
            "name": "guide",
            "description": "LÉELO PRIMERO. Guía completa del formato de proyecto de montaje: clips, capas, efectos, transiciones, audio, macros y recetas de edición estilo Ryker (freestyle) y Shorts.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "catalog",
            "description": "Lista en JSON de todos los efectos, macros, transiciones, animaciones, presets de texto, partículas, sonidos incorporados, efectos de audio y fuentes.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "probe",
            "description": "Información de un archivo multimedia: duración, resolución, fps, audio.",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
        },
        {
            "name": "analyze",
            "description": "Analiza el audio (y opcionalmente cortes de escena) de un archivo: tempo, beats, golpes, momentos más fuertes (reacciones del público), silencios y energía. Úsalo para sincronizar efectos y elegir los mejores momentos.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "from": {"type": "number"}, "to": {"type": "number"},
                "scenes": {"type": "boolean", "description": "Detectar cambios de plano (más lento)."}
            }, "required": ["path"]}
        },
        {
            "name": "contact_sheet",
            "description": "Devuelve una imagen con miniaturas de un video a intervalos regulares, cada una con su tiempo. Así puedes VER el contenido del video y elegir planos.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "from": {"type": "number"}, "to": {"type": "number"},
                "count": {"type": "integer", "description": "Número de miniaturas (24)."},
                "columns": {"type": "integer", "description": "Columnas (6)."},
                "width": {"type": "integer", "description": "Ancho de cada miniatura en px (280)."}
            }, "required": ["path"]}
        },
        {
            "name": "transcribe",
            "description": "Transcribe la voz de un video/audio con Whisper local (sin claves ni internet tras descargar el modelo la primera vez). Devuelve frases con tiempos de la fuente y guarda un .transcript.json con tiempos por palabra para usar en captions. Transcribe solo el tramo que vas a editar (from/to): es más rápido.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "from": {"type": "number"}, "to": {"type": "number"},
                "model": {"type": "string", "description": "tiny | base | small (por defecto) | medium | turbo. Más grande = más preciso y más lento."},
                "language": {"type": "string", "description": "Código de idioma (es, en...). Vacío = detectar."}
            }, "required": ["path"]}
        },
        {
            "name": "check",
            "description": "Valida un proyecto y devuelve la línea de tiempo calculada (inicio/fin de cada clip y capa en tiempo de salida) y avisos.",
            "inputSchema": {"type": "object", "properties": project_props}
        },
        {
            "name": "preview",
            "description": "Renderiza frames del proyecto en los tiempos indicados y devuelve una imagen en cuadrícula para revisar visualmente el resultado antes del render final.",
            "inputSchema": {"type": "object", "properties": preview_props}
        },
        {
            "name": "render",
            "description": "Renderiza el proyecto a MP4 (H.264 + AAC). Usa draft=true para un borrador rápido.",
            "inputSchema": {"type": "object", "properties": render_props}
        }
    ])
}

fn load(args: &Value) -> anyhow::Result<api::Loaded> {
    if let Some(p) = args.get("project").and_then(|v| v.as_str()) {
        api::load_project_file(p)
    } else if let Some(v) = args.get("project_json") {
        api::load_project_value(v)
    } else if let Some(v) = args.get("project").filter(|v| v.is_object()) {
        api::load_project_value(v)
    } else {
        anyhow::bail!("falta 'project' (ruta al .json) o 'project_json'")
    }
}

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(|v| v.as_f64())
}

fn text_result(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

fn image_result(info: &Value, img: &crate::frame::Frame) -> anyhow::Result<Value> {
    let jpg = img.encode_jpeg(85)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpg);
    Ok(json!({"content": [
        {"type": "image", "data": b64, "mimeType": "image/jpeg"},
        {"type": "text", "text": serde_json::to_string_pretty(info).unwrap_or_default()}
    ]}))
}

fn call(name: &str, a: &Value, progress: Option<Box<dyn Fn(u64, u64) + Send>>) -> anyhow::Result<Value> {
    match name {
        "guide" => Ok(json!({"content": [{"type": "text", "text": api::GUIDE}]})),
        "catalog" => Ok(text_result(&api::catalog())),
        "probe" => {
            let p = a.get("path").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("falta path"))?;
            Ok(text_result(&api::probe(p)?))
        }
        "analyze" => {
            let p = a.get("path").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("falta path"))?;
            let scenes = a.get("scenes").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(text_result(&api::analyze(p, num(a, "from"), num(a, "to"), scenes)?))
        }
        "contact_sheet" => {
            let p = a.get("path").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("falta path"))?;
            let count = num(a, "count").unwrap_or(24.0) as usize;
            let cols = num(a, "columns").unwrap_or(6.0) as usize;
            let width = num(a, "width").unwrap_or(280.0) as usize;
            let (info, png, _) = api::contact_sheet(p, num(a, "from"), num(a, "to"), count.clamp(1, 120), cols, width, None)?;
            image_result(&info, &png)
        }
        "transcribe" => {
            let p = a.get("path").and_then(|v| v.as_str()).ok_or_else(|| anyhow::anyhow!("falta path"))?;
            let model = a.get("model").and_then(|v| v.as_str()).unwrap_or("small");
            let lang = a.get("language").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
            let log = |m: &str| eprintln!("{m}");
            Ok(text_result(&api::transcribe(p, model, lang, num(a, "from"), num(a, "to"), None, &log)?))
        }
        "check" => Ok(text_result(&api::check(&load(a)?)?)),
        "preview" => {
            let l = load(a)?;
            let times: Vec<f64> = a
                .get("times")
                .and_then(|v| v.as_array())
                .map(|v| v.iter().filter_map(|x| x.as_f64()).collect())
                .unwrap_or_default();
            let scale = num(a, "scale").unwrap_or(0.5) as f32;
            let cols = num(a, "columns").unwrap_or(3.0) as usize;
            let (info, png, _) = api::preview(&l, &times, scale, cols, 560, None)?;
            image_result(&info, &png)
        }
        "render" => {
            let l = load(a)?;
            let v = api::render(
                &l,
                api::RenderArgs {
                    output: a.get("output").and_then(|v| v.as_str()).map(String::from),
                    draft: a.get("draft").and_then(|v| v.as_bool()).unwrap_or(false),
                    scale: num(a, "scale").map(|x| x as f32),
                    from: num(a, "from"),
                    to: num(a, "to"),
                    codec: a.get("codec").and_then(|v| v.as_str()).map(String::from),
                    quiet: true,
                    progress,
                },
            )?;
            Ok(text_result(&v))
        }
        other => anyhow::bail!("herramienta desconocida '{other}'"),
    }
}

pub fn serve() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let send = |out: &Arc<Mutex<std::io::Stdout>>, v: &Value| {
        let mut o = out.lock().unwrap();
        let _ = writeln!(o, "{}", serde_json::to_string(v).unwrap());
        let _ = o.flush();
    };
    eprintln!("montaje MCP listo (stdio)");
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(&out, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("JSON inválido: {e}")}}));
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let Some(id) = id else { continue }; // notificación
        let result: Result<Value, (i64, String)> = match method {
            "initialize" => {
                let req = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOLS[0]);
                let ver = if PROTOCOLS.contains(&req) { req } else { PROTOCOLS[0] };
                Ok(json!({
                    "protocolVersion": ver,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "montaje", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Editor de video controlado por IA. Llama primero a la herramienta 'guide'. Flujo: probe/analyze/contact_sheet de las fuentes -> escribe un proyecto .json -> check -> preview -> render."
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let token = params.get("_meta").and_then(|m| m.get("progressToken")).cloned();
                let progress: Option<Box<dyn Fn(u64, u64) + Send>> = token.map(|tok| {
                    let out2 = out.clone();
                    Box::new(move |i: u64, n: u64| {
                        let v = json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": tok, "progress": i, "total": n}});
                        let mut o = out2.lock().unwrap();
                        let _ = writeln!(o, "{}", serde_json::to_string(&v).unwrap());
                        let _ = o.flush();
                    }) as Box<dyn Fn(u64, u64) + Send>
                });
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(&name, &args, progress))) {
                    Ok(Ok(v)) => Ok(v),
                    Ok(Err(e)) => Ok(json!({"content": [{"type": "text", "text": format!("Error: {e:#}")}], "isError": true})),
                    Err(_) => Ok(json!({"content": [{"type": "text", "text": "Error interno (pánico) en la herramienta"}], "isError": true})),
                }
            }
            "resources/list" => Ok(json!({"resources": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            _ => Err((-32601, format!("método no soportado: {method}"))),
        };
        match result {
            Ok(r) => send(&out, &json!({"jsonrpc": "2.0", "id": id, "result": r})),
            Err((code, m)) => send(&out, &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": m}})),
        }
    }
    Ok(())
}
