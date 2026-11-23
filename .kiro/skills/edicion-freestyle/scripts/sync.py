"""Comprueba que el audio del render va sincronizado con la fuente.

Compara la envolvente de la voz del render con la de la fuente en varios puntos y busca el desfase que
mejor encaja (±300 ms, en pasos de 10 ms). Debe salir 0 ms en todos; si no, algún clip o transición
está desplazando el audio.

Uso (desde la raíz del repo):
  python sync.py proyectos/<batalla>.build.py scratch/<batalla>/draft.mp4
  python sync.py proyectos/<batalla>.build.py scratch/<batalla>/draft.mp4 --puntos 690,718.5,745

El builder tiene que definir SRC (video fuente), CL (tramos) y O(t) (tiempo de fuente -> salida), como
plantilla_builder.py. Se carga con runpy, así que no escribe el proyecto.
"""
import argparse, math, runpy, struct, subprocess


def env(path, a, d, step=0.01):
    raw = subprocess.run(["ffmpeg", "-v", "quiet", "-ss", str(a), "-t", str(d), "-i", path, "-ac", "1", "-ar", "8000",
                          "-af", "highpass=f=250,lowpass=f=3500", "-f", "s16le", "-"], capture_output=True).stdout
    n = len(raw) // 2
    x = struct.unpack("<%dh" % n, raw[:n * 2])
    hop = int(8000 * step)
    return [math.sqrt(sum(v * v for v in x[i:i + hop]) / hop) for i in range(0, n - hop, hop)]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("builder")
    ap.add_argument("render")
    ap.add_argument("--puntos", help="tiempos de la fuente separados por comas (por defecto, 12 repartidos)")
    args = ap.parse_args()
    g = runpy.run_path(args.builder)
    O, SRC, CL = g["O"], g["SRC"], g["CL"]
    if args.puntos:
        pts = [float(x) for x in args.puntos.split(",")]
    else:
        total = sum(b - a for a, b, *_ in CL)
        pts = []
        for k in range(12):
            q = (k + 0.5) / 12 * total
            for a, b, *_ in CL:
                if q <= b - a:
                    pts.append(round(a + min(q, b - a - 4), 2))
                    break
                q -= b - a
    worst = 0
    for src_t in pts:
        out_t = O(src_t)
        a = env(args.render, out_t, 3.0)
        b = env(SRC, src_t - 0.3, 3.6)
        best = max(range(0, 61), key=lambda k: sum(x * y for x, y in zip(a, b[k:k + len(a)])))
        ms = (best - 30) * 10
        worst = max(worst, abs(ms))
        print("fuente %.2f -> salida %.2f  desfase %+d ms" % (src_t, out_t, ms))
    print("OK" if worst == 0 else "REVISAR: desfase máximo %d ms" % worst)


if __name__ == "__main__":
    main()
