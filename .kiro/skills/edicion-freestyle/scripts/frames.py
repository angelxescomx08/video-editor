"""Cuadrícula de frames exactos de un video, con la marca de tiempo. Sirve para la fuente, para los tramos
bajados y para el render final (comprobar que lo que sale es lo que se vio en preview).

Uso (desde la raíz del repo):
  python frames.py VIDEO salida.png --cols 4 12.4 13.4 14.9 15.9
"""
import argparse, os, subprocess


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("video")
    ap.add_argument("salida", help="PNG de salida (se convierte a ruta absoluta)")
    ap.add_argument("tiempos", nargs="+", type=float)
    ap.add_argument("--cols", type=int, default=4)
    ap.add_argument("--ancho", type=int, default=480)
    args = ap.parse_args()
    W = args.ancho
    H = round(W * 9 / 16)
    out = os.path.abspath(args.salida)
    font = "C:/Windows/Fonts/arial.ttf"
    ff = ("fontfile=" + font.replace(":", "\\\\:") + ":") if os.path.exists(font) else ""
    cmd = ["ffmpeg", "-v", "error", "-y"]
    for t in args.tiempos:
        cmd += ["-ss", str(t), "-i", args.video]
    f = []
    for i, t in enumerate(args.tiempos):
        f.append("[%d:v]trim=end_frame=1,scale=%d:%d,drawtext=%stext='%.2f':x=8:y=6:fontsize=%d:fontcolor=yellow:"
                 "box=1:boxcolor=black@0.6[v%d]" % (i, W, H, ff, t, round(W / 16), i))
    n = len(args.tiempos)
    if n == 1:
        f.append("[v0]null[o]")
    else:
        lay = "|".join("%d_%d" % ((i % args.cols) * W, (i // args.cols) * H) for i in range(n))
        f.append("".join("[v%d]" % i for i in range(n)) + "xstack=inputs=%d:layout=%s:fill=black[o]" % (n, lay))
    cmd += ["-filter_complex", ";".join(f), "-map", "[o]", "-frames:v", "1", out]
    subprocess.run(cmd, check=True)
    print(out)


if __name__ == "__main__":
    main()
