mod analyze;
mod api;
mod audio;
mod ease;
mod frame;
mod fx;
mod mcp;
mod media;
mod mix;
mod project;
mod render;
mod sfx;
mod sheet;
mod text;
mod timeline;
mod transcribe;
mod transitions;
mod util;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "montaje", version, about = "Editor de video rápido controlado por IA (CLI + servidor MCP)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

fn parse_times(s: &str) -> Result<Vec<f64>, String> {
    s.split(',').filter(|x| !x.trim().is_empty()).map(|x| util::parse_time(x)).collect()
}

#[derive(Subcommand)]
enum Cmd {
    /// Renderiza un proyecto a MP4
    Render {
        project: String,
        #[arg(short, long)]
        output: Option<String>,
        /// Borrador rápido (media resolución, codificación rápida)
        #[arg(long)]
        draft: bool,
        #[arg(long)]
        scale: Option<f32>,
        /// Segundo inicial
        #[arg(long, value_parser = util::parse_time)]
        from: Option<f64>,
        /// Segundo final
        #[arg(long, value_parser = util::parse_time)]
        to: Option<f64>,
        /// Codificador: nvenc | qsv | amf | x264
        #[arg(long)]
        codec: Option<String>,
    },
    /// Renderiza frames del proyecto en tiempos concretos a una imagen PNG
    Preview {
        project: String,
        /// Tiempos separados por comas: 1.5,3,0:12.4
        #[arg(long)]
        at: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long, default_value_t = 0.5)]
        scale: f32,
        #[arg(long, default_value_t = 3)]
        cols: usize,
    },
    /// Valida el proyecto y muestra la línea de tiempo calculada
    Check { project: String },
    /// Información de un archivo multimedia
    Probe { path: String },
    /// Análisis de audio: tempo, beats, golpes, picos, silencios (y escenas)
    Analyze {
        path: String,
        #[arg(long, value_parser = util::parse_time)]
        from: Option<f64>,
        #[arg(long, value_parser = util::parse_time)]
        to: Option<f64>,
        #[arg(long)]
        scenes: bool,
    },
    /// Transcribe la voz con Whisper local (guarda .transcript.json y .srt)
    Transcribe {
        path: String,
        /// tiny | base | small | medium | turbo
        #[arg(long, default_value = "small")]
        model: String,
        /// Código de idioma (es, en...). Sin valor = detectar
        #[arg(long)]
        lang: Option<String>,
        #[arg(long, value_parser = util::parse_time)]
        from: Option<f64>,
        #[arg(long, value_parser = util::parse_time)]
        to: Option<f64>,
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Hoja de contacto (miniaturas con tiempos) de un video
    Sheet {
        path: String,
        #[arg(long, value_parser = util::parse_time)]
        from: Option<f64>,
        #[arg(long, value_parser = util::parse_time)]
        to: Option<f64>,
        #[arg(long, default_value_t = 24)]
        count: usize,
        #[arg(long, default_value_t = 6)]
        cols: usize,
        #[arg(long, default_value_t = 280)]
        width: usize,
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Exporta un efecto de sonido incorporado a WAV (o --list)
    Sfx {
        name: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        duration: Option<f32>,
        #[arg(long)]
        list: bool,
    },
    /// Muestra la guía para la IA (formato, efectos y recetas)
    Guide,
    /// Catálogo JSON de efectos, transiciones, sonidos, fuentes...
    Catalog,
    /// Crea un proyecto de ejemplo
    Init {
        #[arg(default_value = "proyecto.json")]
        path: String,
        #[arg(long, default_value = "ryker")]
        style: String,
        #[arg(long)]
        src: Option<String>,
    },
    /// Inicia el servidor MCP por stdio (para Claude Code, Codex, Kiro...)
    Mcp,
}

fn print(v: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.cmd {
        Cmd::Render { project, output, draft, scale, from, to, codec } => {
            let l = api::load_project_file(&project)?;
            eprintln!("renderizando {project}...");
            let v = api::render(&l, api::RenderArgs { output, draft, scale, from, to, codec, quiet: false, progress: None })?;
            print(&v);
        }
        Cmd::Preview { project, at, output, scale, cols } => {
            let l = api::load_project_file(&project)?;
            let times = match at { Some(s) => parse_times(&s).map_err(|e| anyhow::anyhow!(e))?, None => vec![] };
            let thumb = if times.len() == 1 { 0 } else { 640 };
            let (v, _, _) = api::preview(&l, &times, scale, cols, thumb, output)?;
            print(&v);
        }
        Cmd::Check { project } => {
            let l = api::load_project_file(&project)?;
            print(&api::check(&l)?);
        }
        Cmd::Probe { path } => print(&api::probe(&path)?),
        Cmd::Analyze { path, from, to, scenes } => print(&api::analyze(&path, from, to, scenes)?),
        Cmd::Transcribe { path, model, lang, from, to, output } => {
            let log = |m: &str| eprintln!("{m}");
            print(&api::transcribe(&path, &model, lang.as_deref(), from, to, output, &log)?);
        }
        Cmd::Sheet { path, from, to, count, cols, width, output } => {
            let (v, _, _) = api::contact_sheet(&path, from, to, count, cols, width, output)?;
            print(&v);
        }
        Cmd::Sfx { name, output, duration, list } => {
            if list || name.is_none() {
                for (n, d) in sfx::SFX {
                    println!("{n:16} {d}");
                }
                return Ok(());
            }
            let name = name.unwrap();
            let out = PathBuf::from(output.unwrap_or_else(|| format!("{name}.wav")));
            api::sfx_to_wav(&name, &out, duration)?;
            println!("{}", out.display());
        }
        Cmd::Guide => println!("{}", api::GUIDE),
        Cmd::Catalog => print(&api::catalog()),
        Cmd::Init { path, style, src } => {
            let p = PathBuf::from(&path);
            if p.exists() {
                anyhow::bail!("{path} ya existe");
            }
            std::fs::write(&p, serde_json::to_string_pretty(&api::template(&style, src.as_deref()))?)?;
            println!("creado {path}");
        }
        Cmd::Mcp => mcp::serve()?,
    }
    Ok(())
}
