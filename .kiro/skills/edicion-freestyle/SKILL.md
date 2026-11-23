---
name: edicion-freestyle
description: Edita batallas de freestyle (4x4, minutos, réplicas de FMS, Red Bull Batalla, God Level) con montaje - subtítulos sincronizados con las rimas resaltadas, insertos de video a pantalla completa con transiciones, efectos en los punchlines y referencias explicadas con imágenes. Úsala cuando pidan editar una batalla o un 4x4, o cuando el usuario cuente contexto de freestylers (Chuty, Aczino, Nota Roja, Azuky, Skone, jueces, presentadores) para anotarlo.
---

# Edición de batallas de freestyle

Antes de empezar, lee:

- `docs/GUIA_IA.md`: el formato de proyecto de montaje.
- `contexto/personajes.md`: quién es quién, apodos y pullas recurrentes.
- `contexto/batallas/`: batallas ya editadas. `azuky-vs-chuty.md` es el ejemplo completo, barra por barra. Para una batalla nueva, copia `_plantilla.md`.
- `references/proceso-4x4.md`: el proceso entero con comandos, recetas por tipo de barra y errores conocidos.

Scripts (en `scripts/`, se ejecutan desde la raíz del repo):

- `yt.py`: buscar en YouTube, storyboard de un video, bajar solo un tramo y hacer hojas de contacto. Acepta `--cookies`.
- `recortar.py`: recorta planos a insertos 1920x1080 sin audio.
- `plantilla_builder.py`: builder en Python que escribe el proyecto JSON con anclas a palabras.
- `frames.py`: cuadrícula de frames exactos de cualquier video.
- `sync.py`: comprueba que el audio del render va a 0 ms de la fuente.

Las rutas de esta skill son relativas a `.kiro/skills/edicion-freestyle/`.

## El contexto lo da el usuario

Las barras están llenas de referencias que no salen del audio: apodos, rumores, batallas viejas, noticias locales, jueces. Sin ese contexto la edición queda genérica.

1. Transcribe y busca en `contexto/` lo que ya sabes de cada persona.
2. Haz una lista corta de las barras que no entiendas y pregúntale al usuario antes de diseñar. No inventes significados.
3. Anota lo que el usuario explique en el momento, no al final:
   - Personas: `contexto/personajes.md`. Si la persona ya está, añade a su entrada.
   - Barras de esa batalla: `contexto/batallas/<batalla>.md`, a partir de `contexto/batallas/_plantilla.md`.
4. Marca como broma lo que sea pulla de batalla o rumor ("pulla, no es real").

## Lo que funciona y lo que no

Esto es lo que aprobó y lo que rechazó el usuario en Azuky vs Chuty, a lo largo de cuatro rondas.

Funciona:

- Insertos de video a pantalla completa anclados a la palabra, con transición (whip, glitch, zoom, flash, wipe, slide). El audio de la batalla sigue sonando por debajo.
- Clips de batallas reales de los personajes. El favorito del usuario fue el clip de Nota Roja arrodillándose ante Azuky, a pantalla completa durante la parada, con `#NOTAZUKY` y un corazón que se rompe.
- Un bloque diseñado para una línea de contraste. En «pocos me ven ganar / todos te ven perder» las dos líneas van en columnas, y cada palabra salta cuando suena la que le contesta.
- Color con significado. En el pañuelo, blanco y negro con solo el rojo de la sangre (`color_pop`). En «Dios», resplandor dorado. En «diabla», duotono rojo con distorsión de color.
- Tarjetas propias a pantalla completa: las dos mitades en diagonal de Quesada (MX) y Quezada (ES), y el cartel de rivalidad del final.
- El público de la misma batalla cuando la barra habla del público, de la grada o de los fans.

No funciona:

- Texto que explica la rima o la referencia. El usuario lo rechazó en la ronda 2.
- Stickers: PNG pequeños flotando con movimiento. El usuario dijo «se siente como poner stickers, sosa, aburrida». Usa video a pantalla completa.
- Sonidos de notificación (`ding`).
- Cortar la batalla. El 4x4 va entero, con las paradas y con un «3, 2, 1» en pantalla sincronizado con la cuenta atrás del presentador.
- Emojis o rótulos encima de caras, y el banner de «suscríbete» del canal en los planos del público (recórtalo con `crop`).

## Flujo resumido

1. **Fuente y tramo.** `montaje probe`, y `montaje sheet` para localizar el inicio y el final. Un solo clip continuo con el tramo entero (paradas incluidas) y un cierre corto.
2. **Transcripción.** `montaje transcribe <video> --from A --to B --lang es`. Revisa las palabras `doubtful`. Si corriges un segmento, ejecuta `montaje align <transcript> --segments N`.
3. **Mapa de la batalla.** Lista las barras con su significado y una idea visual para cada una. Pregunta lo que falte y anótalo en `contexto/`.
4. **Material.** Antes de la primera descarga de YouTube, pide al usuario unas cookies con el mensaje de `references/proceso-4x4.md` («Cookies de YouTube»). Sin ellas YouTube bloquea las descargas a los pocos minutos. Busca y descarga tramos cortos con `scripts/yt.py --cookies ...`, haz hojas de contacto y recorta cada plano con `scripts/recortar.py` en `proyectos/refs/<batalla>/`.
5. **Proyecto.** Copia `scripts/plantilla_builder.py` a `proyectos/<batalla>.build.py`, rellena la configuración y escribe la edición con sus ayudas. Todo lo que va pegado a una palabra usa anclas `@palabra#n`.
6. **Verificación.**
   - `montaje check` hasta que no queden avisos.
   - `montaje preview --at ...` y `scripts/frames.py` en los momentos clave. Mira las imágenes.
   - Render `--draft`, sincronía con `scripts/sync.py` (debe dar 0 ms) y volumen con `ebur128` (que quede igual que la fuente, ±0.5 LU).
   - Render final y comprobación de frames del archivo final.
7. **Informe.** Qué va en cada momento, créditos, dudas pendientes. Después, actualiza `contexto/batallas/<batalla>.md` con lo que le gustó y lo que no.

## Cuidado con terceros

- Los rumores sexuales o de pareja sobre personas que no son freestylers (parejas, mánagers, familia) se ilustran con los freestylers implicados, nunca con la imagen de esa persona.
- No pongas texto en pantalla que presente un rumor como hecho.
- Los créditos van en el cierre: canal o evento de cada clip y licencia de cada foto.
