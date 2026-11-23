# montaje: guía para la IA

`montaje` es un editor de video que se controla escribiendo un proyecto JSON. Tú (la IA) no ves ni oyes el video directamente: usas herramientas para inspeccionarlo, escribes el proyecto y lo verificas con imágenes de vista previa antes del render final.

## Flujo de trabajo

1. **Inspeccionar fuentes**
   - `probe` → duración, resolución, fps, audio.
   - `contact_sheet` → imagen con miniaturas y tiempos. Mira el contenido. Para videos largos, primero una hoja de todo el video (24–40 miniaturas) y después hojas de los tramos interesantes (`from`/`to`).
   - `analyze` → tempo (BPM), `beats`, `onsets` (golpes), `loudest` (momentos más fuertes: reacciones del público, punchlines, drops), `silences`, `energy` (un dígito 0–9 cada `energy_step` s) y, con `scenes: true`, cambios de plano.
   - `transcribe` → frases con tiempos de la fuente y un `.transcript.json` con tiempos por palabra para usar directamente en `captions`. Va en CPU (~1.6× tiempo real con `small`): transcribe solo los tramos que vas a usar (`from`/`to`). `model`: `tiny`/`base` (rápidos, peores), `small` (por defecto), `medium`/`turbo` (mejores, lentos). Indica `language` si lo sabes (`es`). Revisa el texto: en rap con público puede equivocarse en nombres y jerga; corrige el texto en `segments` si hace falta.
2. **Escribir el proyecto** en un archivo `.json`. Las rutas relativas (fuentes, imágenes, transcripciones y el `output` de `render`) se resuelven desde la carpeta del proyecto.
3. **`check`** → valida y devuelve la línea de tiempo de salida (inicio/fin de cada clip y capa) y avisos. Corrige todos los avisos.
4. **`preview`** con `times` en los momentos clave (punchlines, transiciones, textos). Mira las imágenes y ajusta.
5. **`render`** con `draft: true` para un borrador rápido si el usuario quiere verlo; después `render` final.

Por CLI es lo mismo: `montaje probe|sheet|analyze|transcribe|check|preview|render|guide|catalog`.

## Estructura del proyecto

```json
{
  "output": { "preset": "youtube", "fps": 30 },
  "style": "ryker",
  "sources": { "batalla": "C:/videos/batalla.mp4" },
  "clips": [ ... ],
  "layers": [ ... ],
  "effects": [ ... ],
  "audio": [ ... ],
  "audio_effects": [ ... ]
}
```

- **`output`**: `width`, `height` (1920×1080), `fps` (30), `preset` (`youtube`, `720p`, `4k`, `shorts`/`tiktok`/`reels` = 1080×1920, `square`), `background` (color), `duration` (forzar), `quality` (0–51, menor = mejor, 19), `volume` (maestro), `loudness` (normalizar, p. ej. `-14`).
- **`style`**: aplica un look global y presets por defecto:
  - `ryker`: gradación oscura y contrastada con tinte azul, viñeta, franjas de cine 2.35:1, grano; los textos usan el preset `subtitle` centrado en pantalla.
  - `cinematic`: teal & orange suave, viñeta, franjas, grano.
  - `meme`: textos `impact`. `shorts`/`tiktok`: textos `caption`. `clean`/ausente: nada.
- **`sources`**: alias → ruta. En `src` puedes usar el alias o una ruta.
- Tiempos: número en segundos (`12.5`) o texto `"1:23.5"`, `"83.5s"`, `"500ms"`.
- Colores: `#rrggbb`, `#rrggbbaa` o nombre (`white`, `black`, `red`, `blue`, `yellow`, `gold`, `purple`, `teal`, `cyan`, `pink`, `orange`, `green`, `gray`).
- Los campos desconocidos dan error (así detectas erratas).

## Clips (secuencia principal)

Los clips se reproducen uno tras otro. El tiempo de salida de cada clip es la suma de las duraciones anteriores (`check` te lo muestra).

| campo | uso |
|---|---|
| `src` | video o imagen (alias o ruta) |
| `color` / `gradient` (+`gradient_angle`) | clip de color sólido o degradado `["#1a0033","#000"]` |
| `in`, `out` | rango de la fuente (s). Sin `out`: hasta el final o `in + duration` |
| `duration` | duración (imágenes/colores, o en lugar de `out`) |
| `speed` | 0.5 = cámara lenta, 2 = rápido |
| `audio_mode` | `stretch` (mantiene el tono, por defecto) o `pitch` (el tono baja/sube con la velocidad, efecto "voz lenta") |
| `reverse` | reproducir al revés (efecto rebobinar con `speed` 2–3) |
| `freeze` | segundos extra congelando el último frame. Con `in` y sin `out` = solo frame congelado |
| `repeat` | repetir el clip N veces seguidas (stutter: clip de 0.1–0.2 s con `repeat` 3) |
| `volume`, `mute` | audio del clip |
| `fit` | `cover` (recorta para llenar), `contain`, `blur` (fondo desenfocado: ideal 16:9 → 9:16), `stretch` |
| `crop` | `[x, y, w, h]` normalizado (0–1) antes de escalar. Para vertical desde horizontal: `[0.3, 0, 0.32, 1]` |
| `transition` | transición desde el clip anterior (ver abajo) |
| `effects` | efectos con tiempos **relativos al inicio del clip** |
| `label` | nota libre |

Duración de salida de un clip = `(out - in) / speed × repeat + freeze`.
Para pasar de tiempo de fuente a tiempo de salida dentro de un clip: `salida = inicio_clip + (t_fuente - in) / speed`.

### Transiciones

`"transition": {"type": "zoom_in", "duration": 0.4}`. La transición se centra en el corte y usa material de la fuente antes de `in` y después de `out` del clip anterior, así que **no cambia la duración total**.

Tipos: `cut`, `fade`, `fade_black`, `flash` (`color`), `zoom_in`, `zoom_out`, `whip` (barrido con motion blur, `direction`: left|right|up|down), `slide`, `spin`, `glitch`, `wipe` (`direction`), `circle`, `pixelate`, `blur`, `shake`, `rgb`, `stretch`, `luma`.
Opciones: `duration`, `direction`, `color`, `intensity` (1), `ease`.
Las transiciones dinámicas añaden sonido automático (whip/slide/spin/stretch → `whoosh`; zoom → `swoosh`; glitch/rgb/pixelate → `glitch`; shake → `impact`). Cambia con `"sfx": "boom"` o quítalo con `"sfx": false`; volumen con `sfx_volume`.

## Efectos

Van en `effects` del proyecto (tiempos absolutos de salida), de un clip (tiempos relativos al clip) o de una capa (relativos a la capa).

Campos de tiempo comunes:
- `at` (número o lista) + `duration` → efecto puntual. Con lista se repite en cada tiempo (útil con beats).
- o `start` / `end` (o `start` + `duration`). Sin tiempos: todo el rango.
- `fade_in`, `fade_out` (s): rampa de intensidad. `ease`: `linear`, `in_quad`, `out_quad`, `in_out_quad`, `in_cubic`, `out_cubic`, `in_out_cubic`, `in_expo`, `out_expo`, `in_out_expo`, `in_back`, `out_back`, `out_elastic`, `out_bounce`, `smooth`.
- `apply_to`: `video` (solo la imagen de los clips) o `all` (también textos y capas). Por defecto `video`, salvo flash, fade, letterbox, vignette, grain, strobe, scanlines y vhs, que van sobre todo.

| efecto | parámetros |
|---|---|
| `zoom` | `from` (1), `to` (1.2), `center` [x,y], `ease` (out_cubic). Usa `from`<1 para alejar |
| `zoom_punch` | `scale` (1.25): zoom brusco y vuelta suave; `hold: true` se queda. Duración 0.5 |
| `shake` | `intensity` px (25), `frequency` Hz (18), `rotation` grados (1.2), `decay` (true) |
| `move` | `from`/`to` = [dx, dy] en fracción del lienzo |
| `rotate` | `from`/`to` en grados |
| `flash` | `color` (blanco), `opacity` (0.9), duración 0.15 |
| `fade` | fundido a `color` (negro), opacidad `from`→`to` |
| `bw` | blanco y negro, `amount` |
| `grade` | `brightness` (−1..1), `contrast` (1), `saturation` (1), `exposure` (pasos), `gamma`, `hue` (grados), `temperature` (−1..1), `tint` + `tint_amount` |
| `invert` | `amount` |
| `rgb_split` | aberración cromática: `amount` px (12), `angle` |
| `glitch` | `intensity` 0–1 (0.7), `rate` cambios/s (14) |
| `blur` | `radius` px o `from`/`to` |
| `zoom_blur` | desenfoque radial: `amount` 0–1, `center` |
| `motion_blur` | `amount` px, `angle` |
| `vignette` | `amount` (0.45), `softness` |
| `letterbox` | franjas: `ratio` (2.35) o `size` (fracción de alto), `color`; con `fade_in` entran deslizando |
| `pixelate` | `size` px o `from`/`to` |
| `grain` | `amount` (0.06) |
| `scanlines` | `amount` |
| `vhs` | look VHS completo, `amount` |
| `glow` | resplandor: `amount`, `radius`, `threshold` (0.6), `color` |
| `light_leak` | fugas de luz animadas: `colors`, `amount`, `rate` |
| `mirror` | `axis` x/y |
| `strobe` | parpadeo de `color` a `rate` Hz |
| `duotone` | `colors` [sombras, luces] |
| `posterize` | `amount` niveles |
| `sharpen` | `amount` |
| `color_pop` | conserva solo un `color`, el resto gris |

### Macros (combinaciones listas)

| macro | qué hace |
|---|---|
| `punch` | zoom_punch + shake + flash + rgb_split + sonido `boom`. Con `text` añade un texto de impacto (en estilo ryker: franja `banner`). `intensity`, `sfx` (nombre o false), `volume`, `color`, `preset` |
| `impact` | como punch pero más fuerte, con glow, `impact` + `sub_drop` |
| `glitch_hit` | glitch + rgb_split + sonido glitch (opcional `text`) |
| `drop` | 1.6 s antes de `at`: zoom lento, desenfoque, filtro paso bajo en el audio de los clips y `riser`; en `at`: punch. `duration` = duración de la subida |
| `beat` | pulso de zoom + flash suave. Ideal con `"at": [beats...]` |
| `whip` | barrido con motion blur y `whoosh` |
| `bw_hit` | durante `duration`: blanco y negro con contraste, zoom mantenido, sacudida y boom (opcional `text`) |

Ejemplo: `{"type": "punch", "at": [12.3, 18.9], "intensity": 1.2}`.

## Capas (`layers`)

Se dibujan encima de los clips en orden (la última queda arriba). Campos comunes:

- `start`, `end` o `duration` (sin `end`: hasta el final).
- Posición: `x`, `y` en fracción del lienzo (0.5, 0.5 = centro) o `position`: `center`, `top`, `bottom`, `left`, `right`, `top_left`, `top_right`, `bottom_left`, `bottom_right`, `lower_third`, `upper_third`. `anchor` [ax, ay] (punto del elemento que se coloca en x,y; por defecto [0.5,0.5]).
- `scale`, `rotation` (grados), `opacity`, `blend` (`normal`, `add`, `screen`, `multiply`, `overlay`).
- `width`, `height`: píxeles (`400`) o porcentaje del lienzo (`"50%"`).
- `animate_in`, `animate_out`: `"pop"` o `{"type": "pop", "duration": 0.3, "intensity": 1}`. Tipos: `pop`, `fade`, `slide_up`, `slide_down`, `slide_left`, `slide_right`, `zoom_in` (de grande a normal), `zoom_out` (de pequeño a normal), `blur`, `glitch`, `wipe`, `spin`, `bounce`, `stretch`, `flicker`, `shake`, `typewriter`.
- `animate` (en bucle): `wiggle`, `pulse`, `float`, `shake`, `flicker`, `spin`, `glitch`.
- `drift` [dx, dy]: desplazamiento lineal durante la capa. `zoom_to`: zoom lento (Ken Burns), p. ej. 1.15.
- `effects`: efectos sobre la propia capa (tiempos relativos a la capa), p. ej. `rgb_split`, `glitch`, `glow`.

### `text`

- `text`: admite `\n` y **resaltados**: `"yo soy de otro [planeta]"` o con color de caja `"[ESCAMANDO|#b71c1c]"`. Un grupo entre corchetes puede tener varias palabras: `"[uno a una]"`.
- `preset`: `subtitle` (barras estilo Ryker: Bebas Neue en cursiva, espaciado ancho, mayúsculas, palabras resaltadas en cajas de colores que rotan azul/dorado/rojo/teal/morado, aparición palabra a palabra), `banner` (palabra gigante blanca en franja de color a todo el ancho; `box_color` para cambiar el color), `title` (título grande morado con brillo), `kinetic` (texto de impacto con pop y temblor), `impact` (meme), `caption` (Shorts/TikTok), `lower_third`, `watermark`.
- Estilo: `font` (`anton`, `bebas`, `archivo`, `bangers`, `impact`, `arial-black`, `bahnschrift`... o ruta a .ttf), `size` px, `color`, `uppercase`, `italic` o `skew` (grados), `letter_spacing` (en em, 0.3 = ancho), `line_height`, `align`, `max_width` (px o %).
- Contorno/sombra/brillo: `stroke_color`, `stroke_width`, `shadow_color`, `shadow_offset` [x,y], `shadow_blur`, `glow_color`, `glow_radius`.
- Cajas: `box_color` (fondo del texto), `box_padding` [x,y], `box_full_width`; resaltados: `highlight_color` (color del texto resaltado), `highlight_box` (un color) o `highlight_colors` (lista que rota).
- Aparición: `reveal`: `words` (palabra a palabra) o `none`; `reveal_duration`; `word_times` (lista de tiempos absolutos de salida, uno por palabra) para sincronizar con la voz. `animate_in: "typewriter"` = letra a letra.
- `echo`: `{"count": 3, "opacity": 0.4, "spacing": 1, "outline": true, "direction": "vertical|horizontal|depth"}` → copias en contorno arriba/abajo (efecto de "OBSERVO QUE TE ESTÁS ESCAMANDO" repetido).

### `captions` (subtítulos)

```json
{"type": "captions", "segments": [{"start": 4.2, "end": 6.0, "text": "y cada vez que [suelto]", "word_times": [4.2, 4.5, 4.7, 4.9, 5.2]}]}
```
También `srt: "archivo.srt"` o **`transcript: "video.transcript.json"`** (salida de `transcribe`). Con `transcript` los tiempos de la fuente se **convierten solos al tiempo de salida** según los clips que usan esa fuente: los subtítulos siguen tu edición aunque cortes, reordenes o cambies la velocidad. Opciones: `max_words` (palabras por subtítulo, 5), `highlight_words` (lista de palabras que se resaltan en caja: `["planeta", "diminuto"]`). Cada subtítulo es una capa de texto con el preset `subtitle` (o `caption` en shorts); el resto de campos de estilo se heredan.

### `image`, `video`, `rect`, `particles`

- `image`: `src`, tamaño con `width`/`height` (si das los dos, `fit`: contain|cover|stretch), `radius` (esquinas), `crop`, `chroma_key` (color a quitar, p. ej. `"#00ff00"`, con `chroma_similarity`, `chroma_smoothness`).
- `video`: como imagen + `in`, `out`, `speed`, `loop`, `volume` (su audio se mezcla). Sirve para memes en croma, reacciones, picture-in-picture, overlays de partículas (`blend: "screen"`).
- `rect`: `color`, `width`, `height`, `radius`. Barras de color que entran deslizando, fondos de rótulos.
- `particles`: `particle` = `embers` (brasas), `sparks`, `snow`, `stars`, `dust`, `confetti`, `bokeh`, `rain`; `count`, `colors`/`color`, `particle_size`, `particle_speed`, `seed`.

## Audio

`audio`: lista de pistas.
- Música: `{"src": "beat.mp3", "start": 0, "in": 30, "volume": 0.5, "fade_in": 1, "fade_out": 2, "loop": true, "duck": 0.6}`. `duck` baja la música cuando hay voz en los clips (0–1).
- Sonidos: `{"sfx": "boom", "at": [3.2, 7.5], "volume": 0.9}`. `sfx` acepta un sonido incorporado o una ruta a un archivo. Opcional: `pitch` (1 = normal), `duration` (en riser, whoosh, beep, sub_drop, static, reverse_cymbal, downlifter), `pan` (−1..1).
- Campos: `start`/`at`, `end`, `in`, `out`, `volume`, `fade_in`, `fade_out`, `loop`, `speed`, `pitch`, `pan`, `duck`, `bus` (`music`, `sfx`, `voice`).

Sonidos incorporados (sintetizados, sin archivos): `boom`, `vine_boom`, `impact`, `hit`, `bass_hit`, `sub_drop`, `whoosh`, `swoosh`, `riser`, `reverse_cymbal`, `downlifter`, `glitch`, `beep`, `scratch`, `rewind`, `airhorn`, `ding`, `pop`, `click`, `shutter`, `heartbeat`, `laser`, `thunder`, `explosion`, `clap`, `tape_stop`, `static`.

`audio_effects`: `{"type": "echo", "start": 12.1, "end": 12.5, "target": "clips"}`.
- `target`: `master` (por defecto), `clips` (audio de los clips y capas de video), `music`, `sfx`.
- Tipos: `lowpass` (`cutoff` 500, `cutoff_to` para barrido), `highpass`, `bandpass`, `telephone`, `gain` (`volume` o `db`, `volume_to`), `mute`, `beep` (censura), `echo` (`delay` 0.22, `feedback` 0.45, `mix`; la cola continúa después de `end`: ideal en la última palabra de una barra), `reverb` (`size`, `mix`), `tape_stop`, `stutter` (`slice`, `repeats`), `reverse`, `bitcrush` (`bits`, `downsample`), `distortion` (`drive`).

El audio final pasa por un limitador (−1 dBFS) para que no sature.

## Receta: edición estilo Ryker (batallas de freestyle)

Lo que caracteriza estas ediciones (analizadas frame a frame):
1. **Look**: `"style": "ryker"` (oscuro, contrastado, tinte azul/morado, franjas 2.35:1, viñeta, grano). Tramos en blanco y negro (`bw`) para dramatismo, con color de nuevo en el remate.
2. **Intro**: tarjeta de título sobre fondo oscuro o estrellado: `gradient` + `particles: stars` + texto `title` ("MC1 VS MC2"), luego una tarjeta de temática ("TEMÁTICA: EXTRATERRESTRE") y la marca del editor (`watermark` o un texto con `animate_in: "wipe"`).
3. **Subtítulos de las barras** centrados en pantalla, palabra por palabra, con la palabra clave de cada barra resaltada en caja de color: `captions` con `transcript` + `highlight_words`, o `segments` con `[palabra]`. Los colores rotan solos.
4. **Palabras clave gigantes**: cuando la barra menciona algo visual (VEGETA, LUNA, REPTILIANO), un texto `banner` de 0.6–1 s exactamente en esa palabra, con `glitch_hit` o `punch`. Si tienes una imagen del concepto (planeta, luna, cerebro, personaje), ponla como clip corto (0.5–1.5 s, `zoom_to` 1.15) o capa `image` con `animate_in: "zoom_in"`.
5. **Punchlines**: `punch` en la palabra final de la barra fuerte (`intensity` 1–1.5), `echo` de audio sobre esa palabra (`target: "clips"`), y si el público explota (`analyze.loudest`), deja respirar el clip 1–2 s con `zoom` lento y quizá `shake` leve.
6. **Ritmo**: cortes y transiciones en los `beats` (`analyze`). Transiciones rápidas: `whip`, `zoom_in`, `glitch`, `flash` de 0.25–0.4 s. Evita transiciones en mitad de una barra.
7. **Efectos de textura**: `rgb_split` en golpes, `glitch` corto en cambios, `light_leak` suave en momentos épicos, `echo` de texto (`echo` en capa) en una barra que se repite.
8. **Recursos de tiempo**: cámara lenta (`speed` 0.4–0.6, `audio_mode: "pitch"` para voz grave) en la reacción; `freeze` + `shutter` + `bw` para congelar una cara; `reverse` con `speed` 2–3 + `rewind` + `vhs` para "rebobinar y verlo otra vez"; `repeat` 3 sobre 0.15 s para tartamudeo.
9. **Sonido**: el audio original de la batalla es la base (beat + voz + público). Encima: `boom`/`impact` en punchlines, `whoosh` en transiciones (automático), `riser` antes del momento fuerte, `drop` si hay subida, `vine_boom` para remates cómicos, `airhorn` con moderación.
10. **Outro**: últimas 3–5 s con marca/redes (texto `watermark` o `lower_third`), fundido (`fade` a negro) y bajada de audio (`gain` con `volume_to: 0`).

Plantilla mínima:

```json
{
  "output": {"preset": "youtube", "fps": 30},
  "style": "ryker",
  "sources": {"b": "C:/videos/batalla.mp4"},
  "clips": [
    {"gradient": ["#12002e", "#000000"], "duration": 2.5},
    {"src": "b", "in": 754.2, "out": 771.8, "transition": {"type": "zoom_in", "duration": 0.4},
     "effects": [{"type": "punch", "at": 9.6, "intensity": 1.2}]},
    {"src": "b", "in": 772.0, "out": 774.0, "speed": 0.5, "audio_mode": "pitch", "transition": {"type": "flash"},
     "effects": [{"type": "bw"}, {"type": "zoom", "from": 1.05, "to": 1.25}]}
  ],
  "layers": [
    {"type": "particles", "particle": "stars", "end": 2.5},
    {"type": "text", "preset": "title", "text": "LOBO\nVS\nCHUTY", "start": 0.2, "end": 2.4},
    {"type": "captions", "transcript": "batalla.transcript.json", "highlight_words": ["planeta", "luna"]},
    {"type": "text", "preset": "banner", "text": "VEGETA", "start": 8.1, "duration": 0.8}
  ],
  "audio": [{"sfx": "riser", "start": 0.8, "duration": 1.7, "volume": 0.5}],
  "audio_effects": [{"type": "echo", "start": 11.9, "end": 12.3, "target": "clips"}]
}
```

## Receta: Shorts / TikTok / Reels

- `"output": {"preset": "shorts"}` y `"style": "shorts"`.
- Desde video horizontal: `"fit": "blur"` (cabe entero con fondo desenfocado) o `"crop": [x, 0, 0.316, 1]` para reencuadrar a 9:16 sobre la persona (0.316 ≈ 9/16 ÷ 16/9).
- Subtítulos grandes `caption` en el tercio inferior (`y` 0.7), palabra a palabra, palabra clave en amarillo con `[ ]`.
- Gancho en los primeros 1.5 s: texto `kinetic` + `punch`. Cortes cada 1–3 s. Duración 15–60 s.

## Receta: un video de cualquier tema sin metraje

Cuando no hay video del usuario, monta el video con lo que tengas:
- Clips de `color`/`gradient` + capas `particles` (`bokeh`, `stars`, `dust`) como fondos animados.
- Imágenes (las que el usuario dé o las que descargues con permiso) como clips con `zoom_to` 1.1–1.2 y `drift` (Ken Burns), transiciones `fade`/`zoom_in`/`slide`.
- Textos `title`, `kinetic`, `lower_third` y `caption` para contar la historia; `reveal: "words"` para que el texto acompañe la narración.
- Música con `fade_in`/`fade_out` y `duck` si hay voz; sonidos `whoosh`/`pop`/`ding` para dar ritmo a cada aparición.
- Videos de stock como capas `video` o clips; memes con croma con `chroma_key`.

## Consejos

- Usa siempre `check` tras editar y `preview` en 6–12 tiempos clave. Ajusta tamaños y posiciones mirando las imágenes.
- `contact_sheet` de 1 miniatura cada 2–4 s es suficiente para orientarte; luego afina con tramos cortos.
- Los efectos de clip usan tiempo relativo al clip; los del proyecto, tiempo absoluto de salida. Si dudas, usa `check` para ver el inicio de cada clip.
- Los efectos pesados (zoom_blur, motion_blur, spin, glow grande) son más lentos: úsalos en momentos cortos.
- `render` con `draft: true` va a media resolución y es varias veces más rápido; para el final, sin draft.
- Si un archivo tiene espacios o emojis en la ruta, va bien igual: escribe la ruta tal cual (en JSON usa `/` o `\\`).
