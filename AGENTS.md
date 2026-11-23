# Instrucciones para agentes

Este repositorio es **montaje**, un editor de video en Rust pensado para que una IA lo maneje.

## Si te piden hacer o editar un video

1. Lee la guía completa: `docs/GUIA_IA.md` (o ejecuta `montaje guide`, o la herramienta MCP `guide`).
2. Inspecciona las fuentes: `montaje probe <archivo>`, `montaje sheet <video> --count 30` (abre la imagen PNG que genera), `montaje analyze <archivo> --scenes`, `montaje transcribe <video> --from A --to B --lang es`.
3. Escribe el proyecto JSON en `proyectos/<nombre>.json` (crea la carpeta si no existe).
4. `montaje check proyectos/<nombre>.json` hasta que no haya avisos.
5. `montaje preview proyectos/<nombre>.json --at 1,4.5,9` y mira la imagen.
6. `montaje render proyectos/<nombre>.json --draft` (rápido) y después sin `--draft`.

El binario está en `target/release/montaje.exe` (compílalo con `cargo build --release` si no existe) o en el PATH si se instaló con `cargo install --path .`.

## Si te piden cambiar el editor

- Todo el código es Rust (`src/`). ffmpeg/ffprobe se usan como binarios externos para decodificar y codificar.
- Módulos: `project.rs` (formato JSON), `timeline.rs` (resolución de tiempos, presets, macros, estilos), `render.rs` (composición por frame), `fx.rs` (efectos), `transitions.rs`, `text.rs`, `audio.rs` + `mix.rs` + `sfx.rs` (audio), `analyze.rs`, `transcribe.rs` (Whisper con candle), `sheet.rs`, `mcp.rs` (servidor MCP), `api.rs` (operaciones compartidas CLI/MCP).
- Si añades un efecto, transición, preset o parámetro: actualiza las listas de `project.rs`/`timeline.rs` y `docs/GUIA_IA.md`.
- Verifica con `cargo build --release` y renderizando `examples/demo_ryker.json` (`montaje preview` en tiempos clave).
- `MONTAJE_PROFILE=1` imprime tiempos por etapa durante el render.
