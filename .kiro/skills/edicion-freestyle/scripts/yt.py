"""Material de YouTube para las ediciones: buscar, ver de un vistazo, bajar solo el tramo que hace falta.

Uso (desde la raíz del repo):
  python yt.py buscar "chuty vs skone fms" "aczino chuty final"
  python yt.py storyboard fms23=ZR-L8rhqRmA final20=P40yn--QzCo --out scratch/<batalla>
  python yt.py tramos cs=ZR-L8rhqRmA@9:46-10:38 --dir .agents/tmp/<batalla>/dl
  python yt.py hoja .agents/tmp/<batalla>/dl/cs.mp4 --paso 0.4 --out scratch/<batalla>

Opción común (va antes del subcomando):
  --cookies FICHERO  cookies.txt (formato Netscape) si YouTube pide «confirma que no eres un bot».
                     Ejemplo: python yt.py --cookies .agents/tmp/<batalla>/cookies.txt tramos ...
                     Con cookies hace falta además node en el PATH (resuelve el reto JS de YouTube).
                     Las cookies duran poco (en Azuky vs Chuty dejaron de servir el mismo día). Si vuelve el
                     aviso de bot, hay que exportarlas otra vez.

Requisitos: pip install yt-dlp pillow; ffmpeg y ffprobe en el PATH.
"""
import argparse, email, io, os, shutil, subprocess, sys
from concurrent.futures import ThreadPoolExecutor


def ydl_opts(args, **extra):
    o = {"quiet": True, "no_warnings": True}
    if args.cookies:
        if not os.path.exists(args.cookies):
            sys.exit("no existe el fichero de cookies: " + args.cookies)
        o["cookiefile"] = args.cookies
    if shutil.which("node"):
        o["js_runtimes"] = {"node": {}}
        o["remote_components"] = ["ejs:github"]
    o.update(extra)
    return o


def hms(s):
    """'9:46' o '1:02:03' o '586.5' -> segundos"""
    p = [float(x) for x in s.split(":")]
    t = 0.0
    for x in p:
        t = t * 60 + x
    return t


def buscar(args):
    import yt_dlp

    def run(q):
        try:
            with yt_dlp.YoutubeDL(ydl_opts(args, extract_flat=True, skip_download=True)) as y:
                r = y.extract_info("ytsearch%d:%s" % (args.n, q), download=False)
            return q, [(e.get("id"), e.get("duration"), e.get("channel"), e.get("title")) for e in r.get("entries", [])]
        except Exception as ex:
            return q, [("ERR", 0, "", str(ex)[:200])]

    with ThreadPoolExecutor(8) as ex:
        for q, es in ex.map(run, args.consultas):
            print("##", q)
            for e in es:
                print("  %s | %s s | %s | %s" % e)


def storyboard(args):
    """Baja el storyboard (miniaturas que YouTube genera cada ~10 s) y lo junta en una hoja con tiempos."""
    import yt_dlp
    from PIL import Image, ImageDraw, ImageFont
    os.makedirs(args.out, exist_ok=True)
    tmp = os.path.join(args.out, "_sb")
    os.makedirs(tmp, exist_ok=True)
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/arialbd.ttf", 13)
    except OSError:
        font = ImageFont.load_default()

    def board(kv):
        k, v = kv.split("=", 1)
        with yt_dlp.YoutubeDL(ydl_opts(args, outtmpl=os.path.join(tmp, k + ".%(ext)s"), format="sb0")) as y:
            info = y.extract_info("https://www.youtube.com/watch?v=" + v, download=True)
        f = [x for x in info["formats"] if x["format_id"] == "sb0"][0]
        cols, rows, W, H = f["columns"], f["rows"], f["width"], f["height"]
        msg = email.message_from_bytes(open(os.path.join(tmp, k + ".mhtml"), "rb").read())
        imgs = [Image.open(io.BytesIO(p.get_payload(decode=True))).convert("RGB") for p in msg.walk()
                if p.get_content_type().startswith("image/")]
        tiles, t0 = [], 0.0
        for fr, im in zip(f["fragments"], imgs):
            n = cols * rows
            step = fr["duration"] / n
            for i in range(n):
                t = t0 + i * step
                if t > info["duration"]:
                    break
                x, y = (i % cols) * W, (i // cols) * H
                if x + W > im.width or y + H > im.height:
                    break
                tiles.append((t, im.crop((x, y, x + W, y + H))))
            t0 += fr["duration"]
        C = 12
        R = (len(tiles) + C - 1) // C
        sheet = Image.new("RGB", (C * W, R * H), "black")
        d = ImageDraw.Draw(sheet)
        for j, (t, im) in enumerate(tiles):
            x, y = (j % C) * W, (j // C) * H
            sheet.paste(im, (x, y))
            d.rectangle([x, y, x + 44, y + 15], fill="black")
            d.text((x + 2, y + 1), "%d:%02d" % (t // 60, t % 60), fill="yellow", font=font)
        out = os.path.join(args.out, "storyboard_" + k + ".png")
        sheet.save(out)
        return k, info.get("title"), len(tiles), out

    def safe(kv):
        try:
            return board(kv)
        except Exception as e:
            return kv, "ERROR", str(e)[:200]

    with ThreadPoolExecutor(4) as ex:
        for r in ex.map(safe, args.videos):
            print(*r)
    shutil.rmtree(tmp, ignore_errors=True)


def tramos(args):
    """Baja solo los tramos pedidos (con audio, hasta 1080p, H.264 si existe)."""
    import yt_dlp
    from yt_dlp.utils import download_range_func
    os.makedirs(args.dir, exist_ok=True)
    segs = []
    for s in args.tramos:
        name, rest = s.split("=", 1)
        vid, rng = rest.split("@", 1)
        a, b = rng.split("-", 1)
        segs.append((name, vid, hms(a), hms(b)))

    def get(s):
        k, v, a, b = s
        for e in ("mp4", "webm", "mkv"):
            p = os.path.join(args.dir, k + "." + e)
            if os.path.exists(p):
                os.remove(p)
        o = ydl_opts(args, outtmpl=os.path.join(args.dir, k + ".%(ext)s"),
                     format="bv*[height<=1080][vcodec^=avc1]+ba[ext=m4a]/bv*[height<=1080]+ba/b",
                     merge_output_format="mp4", download_ranges=download_range_func(None, [(a, b)]),
                     force_keyframes_at_cuts=True)
        try:
            with yt_dlp.YoutubeDL(o) as y:
                y.download(["https://www.youtube.com/watch?v=" + v])
            return k, "ok", "%.1f-%.1f s" % (a, b)
        except Exception as e:
            return k, "ERROR", str(e)[:200]

    with ThreadPoolExecutor(6) as ex:
        for r in ex.map(get, segs):
            print(*r)


def hoja(args):
    """Hoja de contacto de un tramo bajado, con un frame cada --paso segundos (tiempo local del archivo)."""
    from PIL import Image, ImageDraw, ImageFont
    os.makedirs(args.out, exist_ok=True)
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/arialbd.ttf", 16)
    except OSError:
        font = ImageFont.load_default()

    def one(path):
        name = os.path.splitext(os.path.basename(path))[0]
        W, H = 288, 162
        raw = subprocess.run(["ffmpeg", "-v", "error", "-i", path, "-vf", "fps=%g,scale=%d:%d" % (1 / args.paso, W, H),
                              "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], capture_output=True).stdout
        n = len(raw) // (W * H * 3)
        C = 10
        R = (n + C - 1) // C
        sh = Image.new("RGB", (C * W, max(R, 1) * H))
        d = ImageDraw.Draw(sh)
        for i in range(n):
            im = Image.frombytes("RGB", (W, H), raw[i * W * H * 3:(i + 1) * W * H * 3])
            x, y = (i % C) * W, (i // C) * H
            sh.paste(im, (x, y))
            d.rectangle([x, y, x + 50, y + 19], fill="black")
            d.text((x + 3, y + 1), "%.1f" % (i * args.paso), fill="yellow", font=font)
        out = os.path.join(args.out, "hoja_" + name + ".jpg")
        sh.save(out, quality=80)
        return name, n, out

    with ThreadPoolExecutor(6) as ex:
        for r in ex.map(one, args.archivos):
            print(*r)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--cookies", help="cookies.txt de YouTube (formato Netscape)")
    sp = ap.add_subparsers(dest="cmd", required=True)
    p = sp.add_parser("buscar")
    p.add_argument("consultas", nargs="+")
    p.add_argument("-n", type=int, default=8, help="resultados por consulta")
    p.set_defaults(fn=buscar)
    p = sp.add_parser("storyboard")
    p.add_argument("videos", nargs="+", help="nombre=ID")
    p.add_argument("--out", required=True)
    p.set_defaults(fn=storyboard)
    p = sp.add_parser("tramos")
    p.add_argument("tramos", nargs="+", help="nombre=ID@inicio-fin (m:ss o segundos)")
    p.add_argument("--dir", required=True)
    p.set_defaults(fn=tramos)
    p = sp.add_parser("hoja")
    p.add_argument("archivos", nargs="+")
    p.add_argument("--paso", type=float, default=0.4)
    p.add_argument("--out", required=True)
    p.set_defaults(fn=hoja)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
