# Proceso completo: editar un 4x4

Así se hizo Azuky vs Chuty. Los comandos se ejecutan desde la raíz del repo; `montaje` es `target\release\montaje.exe`. `S` es la carpeta de la skill, `.kiro/skills/edicion-freestyle`.

Carpetas por batalla:

| Qué | Dónde | ¿Se sube a git? |
|---|---|---|
| Proyecto, transcripción, render | `proyectos/<batalla>.*` | No (`.gitignore`) |
| Builder | `proyectos/<batalla>.build.py` | No |
| Insertos recortados | `proyectos/refs/<batalla>/` | No |
| Descargas, cookies, pruebas | `.agents/tmp/<batalla>/` | No |
| Hojas, previews, draft | `scratch/<batalla>/` | No |
| Contexto y notas de la batalla | `S/contexto/` | Sí |

## 1. Fuente y tramo

```
montaje probe <video>
montaje sheet <video> --count 30
```

Abre la hoja y localiza el 4x4: empieza con el «3, 2, 1» del presentador y termina cuando acaba la última entrada. El tramo va entero, con las paradas (las pausas entre entradas en que el presentador y el público reaccionan). Un solo clip continuo más un cierre de 2–3 s.

## 2. Transcripción

```
montaje transcribe <video> --from A --to B --lang es
```

- Revisa las palabras `doubtful`. Los nombres propios suelen salir mal (Chuty, Aczino, Skone, Nota Roja).
- Si corriges el texto de un segmento: `montaje align <transcript> --segments N`.
- Mueve la transcripción a `proyectos/<batalla>.transcript.json`.
- Las cuentas atrás se ven en `montaje analyze <video> --from A --to B` (golpes) o en las palabras «tres, dos, uno» de la transcripción. Comprueba cada una con `frames.py`.

## 3. Mapa de la batalla

Antes de diseñar nada, haz una tabla de barras con su significado y una idea visual. Busca en `S/contexto/` lo que ya se sabe de cada persona. Lo que no entiendas se lo preguntas al usuario en una lista corta. Ejemplos de cosas que solo sabía el usuario:

- «hermanos de leche» = pulla sobre Chuty, Skone y su mánager.
- «Chuty Lozano» = Chucky Lozano.
- «prueba del pañuelo» = prueba de virginidad, que conecta con «nota roja».
- «Quesada» = un juez y un youtuber con el mismo apellido.

Anota las respuestas en `S/contexto/` en cuanto lleguen.

## 4. Material

Buscar y ver:

```
python S/scripts/yt.py buscar "chuty vs skone fms" "nota roja azuky"
python S/scripts/yt.py storyboard cs=ZR-L8rhqRmA --out scratch/<batalla>
```

El storyboard es una hoja con una miniatura cada ~10 s del video entero, sin descargar el video. Sirve para elegir el minuto.

Bajar solo el tramo y verlo fino:

```
python S/scripts/yt.py tramos cs=ZR-L8rhqRmA@9:46-10:38 --dir .agents/tmp/<batalla>/dl
python S/scripts/yt.py hoja .agents/tmp/<batalla>/dl/cs.mp4 --paso 0.4 --out scratch/<batalla>
montaje analyze .agents/tmp/<batalla>/dl/cs.mp4 --scenes
```

`--scenes` da los cortes de plano. Recorta dentro de un plano, con 0.3–0.5 s de margen:

```
python S/scripts/recortar.py --dir proyectos/refs/<batalla> chuty_skone_cerca=.agents/tmp/<batalla>/dl/cs.mp4@17.2-20.8
python S/scripts/frames.py proyectos/refs/<batalla>/chuty_skone_cerca.mp4 scratch/<batalla>/cerca.png 0.4 0.7 1.1
```

Qué buscar:

- Clips de batallas reales de los personajes: cara a cara, celebraciones, jueces votando, el público, abrazos.
- Planos donde se reconozca a la persona: la cara o el nombre en la camiseta.
- El público de la propia batalla (`src="batalla"` con `"in"`) para barras sobre la grada o los fans.
- Fotos con licencia libre (Wikimedia Commons) para noticias y objetos. Anota autor y licencia para los créditos.
- Stock (leche, lluvia, fuego) solo como capa en modo `screen` encima de un clip, nunca solo.

### Cookies de YouTube: pídelas antes de descargar

Tras unas cuantas descargas, YouTube bloquea la IP con «Sign in to confirm you're not a bot». Probar otros clientes del reproductor (tv, android_vr, ios) no sirvió; esperar tampoco. Lo único que funcionó fue usar cookies del usuario.

Para no perder tiempo con errores, **pide las cookies al usuario en cuanto sepas que vas a bajar material de YouTube**, antes de la primera descarga. Si ya tienes unas y vuelve el aviso de bot, han caducado: pide otras.

#### Mensaje para el usuario

Mándale esto tal cual:

> Para bajar los clips de YouTube necesito unas cookies tuyas, porque YouTube bloquea las descargas automáticas. Son 2 minutos:
>
> 1. En Chrome, instala la extensión **Get cookies.txt LOCALLY**: https://chromewebstore.google.com/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc
> 2. Ve a `chrome://extensions`, entra en **Detalles** de la extensión y activa **Permitir en incógnito**.
> 3. Abre una ventana de incógnito (Ctrl+Shift+N) y entra en youtube.com con tu cuenta. Si tienes una cuenta secundaria, mejor usa esa.
> 4. En esa misma ventana, abre https://www.youtube.com/robots.txt y cierra la pestaña de YouTube. Solo debe quedar abierta la de robots.txt.
> 5. Pulsa el icono de la extensión (el del puzle si no se ve), deja el formato en **Netscape** y pulsa **Export**. Se guarda en Descargas como `www.youtube.com_cookies.txt`.
> 6. Cierra la ventana de incógnito entera y no vuelvas a abrir esa sesión. Si sigues navegando con ella, YouTube cambia las cookies y dejan de valer.
> 7. Avísame cuando esté en Descargas.
>
> Las cookies dan acceso a tu cuenta: solo las uso para estas descargas y las borro al terminar.

#### Qué hacer tú cuando las tenga

1. Mueve el archivo de Descargas a `.agents/tmp/<batalla>/cookies.txt` (en PowerShell, `Move-Item -LiteralPath`). Así no queda una copia en Descargas.
2. Nunca imprimas el contenido. Para comprobarlo, basta con que la primera línea sea `# Netscape HTTP Cookie File` o `# HTTP Cookie File`.
3. Comprueba que `node --version` funciona. Sin node, con cookies sale otro error: YouTube pide resolver un reto JS. `yt.py` añade `js_runtimes` y `remote_components` automáticamente si encuentra node.
4. Prueba con algo ligero antes de bajar en lote: `python S/scripts/yt.py --cookies .agents/tmp/<batalla>/cookies.txt storyboard prueba=<ID> --out scratch/<batalla>`.
5. Baja todos los tramos seguidos, en uno o dos comandos. Las cookies duran poco: en Azuky vs Chuty dejaron de servir el mismo día.
6. Al terminar, borra `cookies.txt` y díselo al usuario.

#### Errores y qué significan

| Mensaje | Causa | Solución |
|---|---|---|
| `Sign in to confirm you're not a bot` sin cookies | IP bloqueada | Pide cookies con el mensaje de arriba |
| `Sign in to confirm you're not a bot` con cookies | Cookies caducadas o cambiadas por YouTube (la sesión se siguió usando en el navegador) | Pide otras, insistiendo en el paso 6 |
| Avisos que mencionan `challenge` o `JavaScript runtime`, o solo hay formatos de imagen | Falta node o el componente `ejs` | Instala node; usa `yt.py`, que lo configura |
| `does not look like a Netscape format cookies file` | Lo exportó en JSON o copió el texto a mano | Pide que lo exporte otra vez con el formato en Netscape |
| La extensión no aparece en incógnito | No tiene permiso | Paso 2 del mensaje |

Wikimedia también limita (error 429). Espera unos minutos o usa otra foto.

## 5. Proyecto

```
copy S\scripts\plantilla_builder.py proyectos\<batalla>.build.py
```

Rellena CONFIGURACIÓN (rutas, tramo, familias de rima, cuentas atrás, créditos) y escribe la edición en la sección EDICIÓN. Todo se ancla con `A("palabra", tiempo_de_fuente)`.

Recetas que funcionaron:

| Barra | Receta |
|---|---|
| Nombra a alguien | `cut()` con un clip de esa persona, de la palabra hasta el final de la frase, con `whip` o `glitch` |
| Pulla sobre dos freestylers | Los dos juntos en dos planos: uno donde se reconozcan (nombre en la camiseta) y otro de cerca |
| Doble sentido con un objeto | Foto del objeto a pantalla completa y corte al personaje en la palabra clave |
| «Dios» (Chuty) | `god()`: resplandor dorado, light leak y flash |
| Color con significado | `bw` + `color_pop` (solo queda un color), duotono rojo para «diabla» |
| Antítesis (A mí… / a ti…) | Bloque de texto propio en columnas, cada palabra salta con su pareja. Parte los subtítulos alrededor |
| El público, la grada, los fans | Plano del público de la misma batalla; recorta el banner del canal con `crop` |
| Arma, golpe, explosión | Imagen grande con `shake`, `strobe` y sonidos `impact` en ráfaga |
| Paradas | `countdown()` sincronizado con el presentador; el clip más fuerte de la batalla puede ir aquí a pantalla completa |
| Final | Cartel de rivalidad y el cara a cara en video |

Parámetros útiles de `cut()`:

- `**{"in": 0.4}`: desde qué segundo del clip empieza.
- `zoom_to=1.1`: empuje durante el inserto (el preset ya trae 1.08).
- `crop=[x, y, w, h]` con `width=1920, height=818`: encuadra otra zona. El estilo `ryker` muestra el video en una franja 2.35:1, así que un inserto 16:9 pierde un 12 % arriba y otro 12 % abajo. Si una cara queda cortada arriba, recorta la parte de arriba: `crop=[0, 0, 1, 0.76]`.
- `tin`/`tout`: `TR("whip", 0.3, direction="left")`, `TR("glitch", 0.24)`, `TR("flash", 0.2, color="#ffffff")`, `TR("zoom_in", 0.3)`, `TR("zoom_out", 0.3)`, `TR("wipe", 0.3, direction="right")`, `TR("slide", 0.32, direction="left")`.

Orden de capas: lo que se añade después queda encima. Una capa en modo `screen` (leche, fuego) va después del `cut()` sobre el que se mezcla.

## 6. Verificación

```
python proyectos/<batalla>.build.py
cmd /c "montaje check proyectos\<batalla>.json > scratch\<batalla>\check.json 2> scratch\<batalla>\check.err"
cmd /c "montaje preview proyectos\<batalla>.json --at 12.1,14.5,15.7 --scale 0.35 --cols 4 -o %CD%\scratch\<batalla>\p.png > nul 2> scratch\<batalla>\prev.err"
cmd /c "montaje render proyectos\<batalla>.json --draft -o %CD%\scratch\<batalla>\draft.mp4 > nul 2> scratch\<batalla>\render.err"
python S/scripts/sync.py proyectos/<batalla>.build.py scratch/<batalla>/draft.mp4
ffmpeg -hide_banner -nostats -i scratch/<batalla>/draft.mp4 -vn -af ebur128 -f null -
cmd /c "montaje render proyectos\<batalla>.json > nul 2> scratch\<batalla>\render.err"
python S/scripts/frames.py proyectos/<batalla>.mp4 scratch/<batalla>/final.png 12.4 13.4 14.9 15.9
```

- `check` debe dar `ok: true` y `warnings: []`.
- Mira siempre las imágenes de `preview` y de `frames.py`. Comprueba que no haya caras cortadas, banners del canal, rótulos encima de caras ni planos de otra persona.
- `sync.py` debe dar 0 ms en todos los puntos.
- Volumen: el valor `I:` del render tiene que quedar a ±0.5 LU del de la fuente. Azuky vs Chuty: −11.7 LUFS en el render y −11.9 en la fuente.
- El draft de Azuky vs Chuty (258 s, fuente a 60 fps, salida a 30) iba a unos 115 fps: poco más de un minuto.

## 7. Informe y notas

- Dile al usuario qué va en cada momento (tiempo de salida → qué se ve), los créditos y lo que quedó pendiente.
- Actualiza `S/contexto/batallas/<batalla>.md` con la ronda, lo que le gustó y lo que no.

## Errores conocidos

- En PowerShell, redirige la salida de `montaje` con `cmd /c "... > out 2> err"`. La barra de progreso en stderr ensucia la salida y a veces la corta.
- `preview -o` necesita una ruta absoluta (`%CD%\...`); con una relativa la imagen puede acabar en otra carpeta.
- Con `$env:PYTHONIOENCODING="utf-8"`, Python imprime bien las tildes en PowerShell.
- Los textos sin preset con `style: "ryker"` salen palabra a palabra como subtítulos. `lay()` del builder les pone `reveal: "none"`.
- Las capas de la batalla con `src="batalla"` usan el tiempo de la fuente en `"in"`, no el de salida.
- Al rehacer una ronda, no vuelvas a ejecutar parches viejos sobre el builder. Haz uno nuevo con reemplazos que comprueben que el texto existe (`assert s.count(old) == 1`), y guarda una copia antes.
- Formato de descarga: `bv*[height<=1080][vcodec^=avc1]+ba[ext=m4a]/bv*[height<=1080]+ba/b`. El H.264 se decodifica más rápido que VP9/AV1.
- Los clips de FMS España llevan rótulos de «palabra incentivo» abajo en algunas entradas. Elige planos sin ellos.
