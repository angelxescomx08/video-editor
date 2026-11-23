"""Plantilla de builder para editar una batalla con montaje.

Cópiala a proyectos/<batalla>.build.py, rellena CONFIGURACIÓN y escribe la edición en la sección EDICIÓN.
  python proyectos/<batalla>.build.py        -> escribe proyectos/<batalla>.json
  montaje check proyectos/<batalla>.json

Por qué un builder en Python en vez de escribir el JSON a mano:
- A("palabra", t) busca la palabra en la transcripción y devuelve el ancla "@palabra#n". Si luego se corrige
  un tiempo (montaje align o montaje studio), todo lo anclado se mueve con la palabra.
- O(t) pasa tiempos de la fuente (los del video original) a tiempos de salida.
- cut(), countdown(), etc. dejan cada receta en una línea.
- Se pueden rehacer cambios con parches pequeños sin perder el resto.
"""
import json, re

# ================================================================ CONFIGURACIÓN
PROJ = "proyectos/BATALLA.json"                      # proyecto que se escribe
TRF = "BATALLA.transcript.json"                       # transcripción (relativa a proyectos/)
SRC = "C:/ruta/al/video/de/la/batalla.mp4"            # video fuente
REFS = "refs/BATALLA/"                                # insertos (relativo a proyectos/), de recortar.py
TITLE = "RIVAL A  VS  RIVAL B"
SUBTITLE = "4X4   ·   EVENTO"
# Tramos de la fuente que entran en el montaje: (inicio, fin, etiqueta, transición o None).
# Un 4x4 va en un solo tramo, con las paradas: cortarlo rompe el ritmo y se pierden las cuentas atrás.
CL = [(0.0, 60.0, "4x4 completo (con las paradas)", None)]
OUTRO = 2.6
# Rimas: color por sonido. Las palabras van en minúscula; no hace falta poner tildes.
FAM = {
    "#6a1b9a": [],   # é-e
    "#b71c1c": [],   # ó-a
    "#1565c0": [],   # á-a
    "#00838f": [],   # í-a
    "#2e7d32": [],   # á-o
    "#ad1457": [],   # ú-o
}
# «3, 2, 1» del presentador en cada parada: ((t3, t2, t1), inicio de la siguiente entrada), en tiempo de fuente.
# Se miden con `montaje analyze <video> --from A --to B` (golpes) o en la transcripción.
COUNTS = []
CREDITS = "Clips: ...  ·  Fotos: ... (licencia)"
# ================================================================ BASE (no hace falta tocar)
tr = json.load(open("proyectos/" + TRF, encoding="utf-8"))
ACC = str.maketrans("áàäâéèëêíìïîóòöôúùüû", "aaaaeeeeiiiioooouuuu")


def norm(w):
    return "".join(c for c in w.lower().translate(ACC) if c.isalnum())


starts = []
_acc = 0.0
for a, b, _, _ in CL:
    starts.append(_acc)
    _acc += b - a
END_MAIN = _acc
TOTAL = END_MAIN + OUTRO


def O(src):
    """tiempo de fuente -> tiempo de salida"""
    for (a, b, _, _), s in zip(CL, starts):
        if a - 1e-6 <= src <= b + 1e-6:
            return round(s + src - a, 3)
    raise ValueError("fuera de los clips: %s" % src)


# palabras que suenan en el montaje, en orden de salida (igual que anchors::output_words de montaje)
MW = []
for (a, b, _, _), s in zip(CL, starts):
    for seg in tr["segments"]:
        for w in seg["words"]:
            if a - 1e-3 <= w["start"] < b - 0.03:
                MW.append((norm(w["word"]), w["start"], w["end"], s + w["start"] - a, s + w["end"] - a))
MW.sort(key=lambda x: x[3])


def _find(word, near):
    n = norm(word)
    cands = [(i, m) for i, m in enumerate([m for m in MW if m[0] == n])]
    if not cands:
        raise ValueError("no está: " + word)
    k, m = min(cands, key=lambda c: abs(c[1][1] - near))
    if abs(m[1] - near) > 0.6:
        raise ValueError("'%s' más cercana a %.2f está en %.2f" % (word, near, m[1]))
    return n, k + 1, m


def A(word, near, end=False, off=0.0):
    """ancla a la palabra que suena cerca de `near` (tiempo de fuente): '@palabra#n[:end][+off]'"""
    n, k, _ = _find(word, near)
    s = "@%s#%d" % (n, k) + (":end" if end else "")
    if abs(off) > 1e-9:
        s += ("%+.2f" % off)
    return s


def T(word, near, end=False, off=0.0):
    """como A pero devuelve el número (tiempo de salida). Úsalo solo para cálculos."""
    _, _, m = _find(word, near)
    return round((m[4] if end else m[3]) + off, 3)


def shift(t, d):
    """suma d segundos a un tiempo o a un ancla"""
    if isinstance(t, (int, float)):
        return round(t + d, 3)
    m = re.match(r"^(@[^:+\-]+(?::end)?)([+-][0-9.]+)?$", t)
    off = float(m.group(2) or 0) + d
    return m.group(1) + ("%+.2f" % off if abs(off) > 1e-9 else "")


HL = [w + "|" + c for c, ws in FAM.items() for w in ws]
for c, ws in FAM.items():
    for w in ws:
        if not any(m[0] == norm(w) for m in MW):
            print("aviso: palabra de rima que no suena:", w)

L, FX, AU, AFX = [], [], [], []


def lay(**k):
    # con style "ryker" un texto sin preset sale palabra a palabra como subtítulo: aquí no se quiere
    if k.get("type") == "text" and "preset" not in k:
        k.setdefault("reveal", "none")
    L.append(k)
    return k


def fx(d):
    FX.append(d)


def au(d):
    AU.append(d)


def afx(d):
    AFX.append(d)


def TR(kind, dur, **o):
    """transición de capa: zoom_in, zoom_out, whip (direction), glitch, flash (color), wipe, slide..."""
    return dict({"type": kind, "duration": dur}, **o)


def cut(src, start, end, tin=None, tout=None, sfxv=0.28, **k):
    """Inserto a pantalla completa (video o imagen) con transición. El audio de la batalla sigue sonando.
    src: archivo dentro de REFS, o "batalla" para un plano de la propia fuente (con "in": segundo de la fuente).
    Extras útiles: **{"in": 0.4} (desde dónde empieza el clip), zoom_to=1.1 (empuje), y para encuadrar otra zona
    crop=[x, y, w, h] (fracciones del clip) con width/height en píxeles."""
    is_vid = src == "batalla" or src.endswith(".mp4")
    d = dict(type="video" if is_vid else "image", preset="cutaway", src=src if src == "batalla" else REFS + src,
             start=start, end=end)
    if tin:
        d["transition_in"] = dict(tin, sfx_volume=sfxv)
    if tout:
        d["transition_out"] = dict(tout, sfx_volume=sfxv)
    d.update(k)
    return lay(**d)


def echo_on(word, near, mix=0.3):
    """eco en la voz durante una palabra"""
    afx({"type": "echo", "target": "clips", "start": A(word, near), "end": A(word, near, end=True),
         "delay": 0.18, "feedback": 0.32, "mix": mix})


def countdown(nums, nxt, size=280, y=0.46, vol=0.42):
    """«3, 2, 1» en pantalla, a la vez que el presentador, en una parada"""
    for i, (num, t0) in enumerate(zip(["3", "2", "1"], nums)):
        t1 = nums[i + 1] if i < 2 else min(nums[2] + 0.55, nxt - 0.04)
        lay(type="text", preset="kinetic", text=num, font="anton", size=size, italic=False, color="#ffffff",
            stroke_color="#1a0b3a", stroke_width=8, glow_color="#7b2cffaa", glow_radius=28, y=y, start=O(t0), end=O(t1),
            animate_in={"type": "pop", "duration": 0.14, "intensity": 1.2}, animate_out={"type": "zoom_out", "duration": 0.1})
    fx({"type": "beat", "at": [O(t) for t in nums], "intensity": 1.0})
    au({"sfx": "hit", "at": [O(t) for t in nums], "volume": vol})
    fx({"type": "vignette", "start": O(nums[0] - 0.1), "end": O(min(nums[2] + 0.55, nxt - 0.04)), "amount": 0.55,
        "fade_in": 0.15, "fade_out": 0.12})


def god(word, near, color="#ffd36b"):
    """resplandor dorado en una palabra (p. ej. «Dios» cuando se refiere a Chuty)"""
    fx({"type": "glow", "start": A(word, near), "end": A(word, near, end=True, off=0.15), "amount": 0.9,
        "radius": 40, "threshold": 0.45, "color": color, "fade_out": 0.15})
    fx({"type": "light_leak", "start": A(word, near), "end": A(word, near, end=True, off=0.25),
        "colors": [color, "#fff1c0"], "amount": 0.45, "fade_in": 0.05, "fade_out": 0.2})
    fx({"type": "flash", "at": A(word, near), "duration": 0.16, "opacity": 0.45, "color": "#fff3cc"})


# ================================================================ EDICIÓN
# Subtítulos con las rimas coloreadas (palabra a palabra, la que suena se resalta).
CAP = dict(type="captions", max_words=10, highlight_words=HL, y=0.715, size=80, letter_spacing=0.1, max_width="84%",
           stroke_color="#0b0b18", stroke_width=3, shadow_color="#000000e6", shadow_offset=[2, 5], shadow_blur=10,
           animate_out={"type": "fade", "duration": 0.08})
lay(**CAP)   # si un bloque diseñado tapa los subtítulos, pártelo: lay(**CAP, end=...) y lay(**CAP, start=...)

for nums, nxt in COUNTS:
    countdown(nums, nxt)

# Ejemplos (descomenta y adapta; los tiempos son de la fuente):
# fx({"type": "punch", "at": A("leche", 698.62), "intensity": 1.1, "volume": 0.5})
# cut("chuty_skone_juntos.mp4", A("son", 696.08, off=-0.05), A("leche", 698.62, off=-0.04), tin=TR("zoom_out", 0.3))
# cut("chuty_skone_cerca.mp4", A("leche", 698.62, off=-0.04), A("eso", 700.62, off=-0.02),
#     tin=TR("flash", 0.2, color="#ffffff"), tout=TR("whip", 0.3, direction="left"), **{"in": 0.4})
# cut("batalla", A("esta", 845.40, off=-0.05), A("grada", 845.62, end=True, off=0.15), **{"in": 946.4},
#     crop=[0.05, 0.04, 0.9, 0.7], width=1920, height=818)   # público de la misma batalla, sin el banner
# god("dios", 759.76)

# ================================================================ CIERRE
lay(type="particles", particle="stars", count=90, colors=["#b8a4ff", "#8fb0ff", "#ffffff"], particle_size=1.7,
    particle_speed=0.35, start=END_MAIN, end=TOTAL)
lay(type="text", preset="title", text=TITLE, font="anton", size=128, letter_spacing=0.04, color="#f4f0ff",
    glow_color="#6a35ffcc", glow_radius=20, y=0.45, start=END_MAIN + 0.25, end=TOTAL - 0.1,
    animate_in={"type": "zoom_out", "duration": 0.35}, animate_out={"type": "fade", "duration": 0.3})
lay(type="text", text=CREDITS, font="segoe", size=21, color="#ffffff99", italic=False, uppercase=False,
    letter_spacing=0.0, shadow_color="#00000000", y=0.84, max_width="90%", start=END_MAIN + 0.6, end=TOTAL - 0.1,
    animate_in={"type": "fade", "duration": 0.3}, animate_out={"type": "fade", "duration": 0.3})
fx({"type": "fade", "start": TOTAL - 0.45, "end": TOTAL, "from": 0, "to": 1})
au({"sfx": "downlifter", "at": END_MAIN + 0.05, "duration": 1.6, "volume": 0.25})

# ================================================================ PROYECTO
clips = []
for (a, b, label, t), s in zip(CL, starts):
    c = {"src": "batalla", "in": a, "out": b, "label": label}
    if t:
        c["transition"] = dict(t, sfx_volume=0.32)
    clips.append(c)
clips.append({"label": "Cierre", "gradient": ["#170a33", "#05060f"], "gradient_angle": 115, "duration": OUTRO,
              "transition": {"type": "fade_black", "duration": 0.6, "sfx": False}})
proj = {
    "output": {"preset": "youtube", "fps": 30, "quality": 19},
    "style": "ryker",
    "sources": {"batalla": SRC},
    "transcripts": {"batalla": TRF},
    "clips": clips,
    "layers": L,
    "effects": FX,
    "audio": AU,
    "audio_effects": AFX,
}
if __name__ == "__main__":   # sync.py carga este archivo con runpy: no debe escribir el proyecto
    json.dump(proj, open(PROJ, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
    print("duración %.2f s; %d capas, %d efectos" % (TOTAL, len(L), len(FX)))
