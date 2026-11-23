"""Recorta planos de los tramos bajados y los deja listos como insertos (1920x1080, 30 fps, sin audio, H.264 CRF 17).

Uso (desde la raíz del repo):
  python recortar.py --dir proyectos/refs/<batalla> chuty_skone_cerca=.agents/tmp/<batalla>/dl/cs.mp4@17.2-20.8 ...

- Los tiempos son del archivo de entrada (los de la hoja de `yt.py hoja`).
- Deja unos 0.3-0.5 s de margen a cada lado del plano. En el proyecto se elige el punto exacto con "in".
- Corta dentro de un mismo plano. Los cambios de plano se ven con: montaje analyze <archivo> --scenes
- La salida es siempre 16:9 a pantalla completa (escala y recorta al centro). Para encuadrar otra zona
  usa "crop" en la capa del proyecto, no aquí.
"""
import argparse, os, subprocess
from concurrent.futures import ThreadPoolExecutor


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("planos", nargs="+", help="nombre=archivo@inicio-fin (segundos)")
    ap.add_argument("--dir", required=True, help="carpeta de salida, p. ej. proyectos/refs/<batalla>")
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--crf", type=int, default=17)
    args = ap.parse_args()
    os.makedirs(args.dir, exist_ok=True)

    def cut(spec):
        name, rest = spec.split("=", 1)
        path, rng = rest.rsplit("@", 1)
        a, b = (float(x) for x in rng.split("-", 1))
        out = os.path.join(args.dir, name + ".mp4")
        cmd = ["ffmpeg", "-v", "error", "-y", "-ss", str(a), "-i", path, "-t", str(round(b - a, 3)), "-an",
               "-vf", "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,fps=%d,format=yuv420p" % args.fps,
               "-c:v", "libx264", "-preset", "medium", "-crf", str(args.crf), out]
        r = subprocess.run(cmd, capture_output=True, text=True)
        return name, "ok" if r.returncode == 0 else "ERROR " + r.stderr[-200:], out

    with ThreadPoolExecutor(6) as ex:
        for r in ex.map(cut, args.planos):
            print(*r)


if __name__ == "__main__":
    main()
