# montaje

Editor de video en Rust pensado para que una IA lo maneje. Le pides a Claude Code, Codex o Kiro (con tu suscripción, sin claves de API) "hazme un minuto de esta batalla estilo Ryker" o "un short de 30 s sobre X", y el agente usa `montaje` para ver el material, escribir el montaje en JSON, revisarlo con imágenes de vista previa y renderizarlo.

- **Un solo lenguaje**: todo el editor es Rust. ffmpeg/ffprobe se usan como binarios externos para decodificar y codificar.
- **Rápido**: composición en paralelo (rayon), tuberías YUV entre ffmpeg y Rust, codificación por GPU (NVENC/QSV/AMF) si existe. En esta máquina (i5-11400F + RTX 3060 Ti), 1080p30: ~100 fps sin efectos, ~60 fps con el estilo `ryker`, ~40 fps en una edición densa (transiciones cada 2 s, textos, partículas). El borrador (`--draft`, 540p) va a ~85 fps.
- **Pensado para la IA**: formato JSON con errores claros (campos desconocidos y tiempos fuera de rango se avisan), macros de alto nivel (`punch`, `drop`, `bw_hit`...), presets de texto, servidor MCP con imágenes de vista previa, y una guía completa en `docs/GUIA_IA.md`.

## Qué puede hacer

- **Clips**: cortes, velocidad (cámara lenta con o sin cambio de tono), reversa, congelado, repetición (stutter), recorte/reencuadre, ajustes `cover`/`contain`/`blur` (horizontal → vertical), imágenes, colores y degradados.
- **Transiciones** centradas en el corte (no cambian la duración): zoom-through, whip con motion blur, glitch, flash, spin, slide, wipe, iris, pixelado, blur, shake, RGB, stretch, luma, fundidos. Con sonido automático.
- **Efectos**: zoom, zoom punch, shake, flash, B/N, gradación, aberración RGB, glitch, desenfoques (normal, radial, direccional), viñeta, franjas de cine, grano, VHS, glow, fugas de luz, duotono, color pop, pixelado, espejo, strobe...
- **Texto**: resaltados en caja con `[palabra]` o `[palabra|#color]`, aparición palabra a palabra o letra a letra, contorno, sombra, brillo, ecos en contorno, cursiva, franjas a todo el ancho, 17 animaciones de entrada/salida y 7 en bucle. Presets: `subtitle` (barras estilo Ryker), `banner`, `title`, `kinetic`, `impact`, `caption` (Shorts), `lower_third`, `watermark`. Fuentes incluidas: Anton, Bebas Neue, Archivo Black, Bangers (OFL) + fuentes del sistema.
- **Capas**: imágenes, video con croma (green screen), rectángulos, partículas (brasas, chispas, estrellas, nieve, confeti, bokeh, lluvia, polvo).
- **Audio**: audio de los clips con fundidos cruzados, música con ducking automático, 27 efectos de sonido sintetizados (boom, whoosh, riser, impact, vine boom, airhorn, scratch, rewind...), efectos por rango (paso bajo/alto con barrido, eco con cola, reverb, tape stop, stutter, reversa, bitcrush, distorsión, censura), normalización y limitador.
- **Análisis**: tempo y beats, golpes, momentos más fuertes (reacciones del público), silencios, cambios de plano, hojas de contacto con tiempos.
- **Transcripción local** con Whisper (Rust puro, candle): frases y tiempos por palabra. Los subtítulos con `"transcript"` siguen tu edición solos, aunque cortes, reordenes o cambies la velocidad.
- **Estilos**: `ryker`, `cinematic`, `meme`, `shorts`. **Formatos**: `youtube`, `720p`, `4k`, `shorts`/`tiktok`/`reels`, `square`.

## Requisitos

- Rust 1.80+ (`cargo`).
- ffmpeg y ffprobe en el PATH (probado con ffmpeg 7.1 de gyan.dev). Otra ruta: variables `MONTAJE_FFMPEG` y `MONTAJE_FFPROBE`.
- `curl` (viene con Windows 10+) para descargar el modelo de Whisper la primera vez (~920 MB el modelo `small`, en `%LOCALAPPDATA%\montaje\models`; cámbialo con `MONTAJE_MODELS`).

## Instalación

```powershell
cargo install --path .
# o solo compilar: cargo build --release  (queda en target\release\montaje.exe)
```

Sin transcripción (compila más rápido): `cargo install --path . --no-default-features`.

## Conectar tu IA (MCP)

El servidor MCP se arranca con `montaje mcp` (stdio). Herramientas: `guide`, `catalog`, `probe`, `analyze`, `contact_sheet`, `transcribe`, `check`, `preview`, `render`.

Los renders y transcripciones pueden durar más de un minuto, así que conviene subir el tiempo máximo por herramienta.

**Claude Code**
```powershell
claude mcp add montaje --scope user -- "$env:USERPROFILE\.cargo\bin\montaje.exe" mcp
# tiempo máximo por herramienta (ms), antes de abrir claude:
$env:MCP_TOOL_TIMEOUT = "900000"
```

**Codex** (`~/.codex/config.toml`)
```toml
[mcp_servers.montaje]
command = 'C:\Users\USER\.cargo\bin\montaje.exe'
args = ["mcp"]
tool_timeout_sec = 900
```

**Kiro** (`~/.kiro/settings/mcp.json`, o `.kiro/settings/mcp.json` en un proyecto)
```json
{
  "mcpServers": {
    "montaje": {
      "command": "C:\\Users\\USER\\.cargo\\bin\\montaje.exe",
      "args": ["mcp"],
      "autoApprove": ["guide", "catalog", "probe", "analyze", "contact_sheet", "check", "preview"]
    }
  }
}
```

Sin MCP también funciona: cualquier agente puede usar la CLI (`montaje guide`, `montaje sheet`, `montaje render`...). En esta carpeta, `AGENTS.md` y `CLAUDE.md` le explican el flujo.

## Ejemplos de lo que puedes pedir

- "Con `C:\videos\batalla.mp4`, hazme el mejor minuto de MC1 estilo Ryker: subtítulos de las barras con la palabra clave resaltada, punch en cada remate, intro con título y la temática."
- "Convierte este video horizontal en un Short vertical de 30 s con subtítulos grandes y un gancho al principio."
- "Haz un video de 45 s sobre la historia del freestyle con estas imágenes y esta música, con textos animados."

## Uso directo (CLI)

```powershell
montaje probe video.mp4
montaje sheet video.mp4 --from 60 --to 120 --count 24      # imagen con miniaturas y tiempos
montaje analyze video.mp4 --scenes                          # beats, picos, silencios, cortes
montaje transcribe video.mp4 --from 60 --to 120 --lang es   # .transcript.json + .srt
montaje init proyecto.json --style ryker --src video.mp4    # proyecto de ejemplo
montaje check proyecto.json
montaje preview proyecto.json --at 1,4.5,9
montaje render proyecto.json --draft                        # borrador rápido
montaje render proyecto.json -o final.mp4
montaje sfx boom -o boom.wav                                 # escuchar un sonido incorporado
montaje catalog                                              # todo lo disponible en JSON
```

Los archivos auxiliares (hojas de contacto, vistas previas, transcripciones) se guardan en `montaje-out/`.

`examples/demo_ryker.json` muestra casi todas las funciones (usa el video de Ryker de Descargas como fuente).

## Límites actuales

- La transcripción va en CPU: con el modelo `small`, ~1.6× tiempo real (30 s de audio en ~18 s). Transcribe solo los tramos que vas a usar. Los tiempos por palabra se estiman dentro de cada frase y se ajustan a los ataques de la voz; no son exactos al fotograma.
- Sin emojis a color en los textos.
- Las imágenes de conceptos (planetas, personajes) y la música deben aportarse como archivos; el editor no genera imágenes.
- Efectos pesados (zoom_blur, motion blur, spin, glow grande) bajan la velocidad en los frames donde actúan.
