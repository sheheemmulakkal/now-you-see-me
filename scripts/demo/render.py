#!/usr/bin/env python3
"""Motion-graphics feature video for Now You See Me, from real captures.

    scripts/demo/capture.sh                      # assets -> target/demo/capture
    scripts/demo/render.py [CAPTURE] [OUT] [--preview]

Writes OUT/now-you-see-me.mp4 (1920x1080, H.264) and OUT/poster.png;
--preview writes one still per scene instead. Captions use Ubuntu Sans;
windows zoom inside a fixed frame on their side, so captions are never
covered.
"""
import json, math, os, subprocess, sys
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
CAP = ARGS[0] if ARGS else "target/demo/capture"
OUT = ARGS[1] if len(ARGS) > 1 else "target/demo/out"
PREVIEW = "--preview" in sys.argv
os.makedirs(OUT, exist_ok=True)

W, H, FPS = 1920, 1080, 30
SANS = "/usr/share/fonts/truetype/ubuntu/UbuntuSans[wdth,wght].ttf"
MONO = "/usr/share/fonts/truetype/ubuntu/UbuntuSansMono[wght].ttf"
BG0, BG1 = (9, 12, 24), (20, 24, 44)
TEAL, PURPLE = (46, 196, 181), (166, 107, 255)
FG, MUTED = (236, 239, 246), (150, 158, 180)


def font(size, weight="Regular", mono=False):
    f = ImageFont.truetype(MONO if mono else SANS, size)
    try:
        f.set_variation_by_name(weight)
    except Exception:
        pass
    return f


def clamp(x, a=0.0, b=1.0):
    return max(a, min(b, x))


def ease_out(t):
    return 1 - (1 - clamp(t)) ** 3


def ease_in_out(t):
    t = clamp(t)
    return 4 * t ** 3 if t < 0.5 else 1 - (-2 * t + 2) ** 3 / 2


def ramp(t, a, b):
    return clamp((t - a) / (b - a)) if b > a else float(t >= a)


def background():
    bg = Image.new("RGB", (W, H))
    d = ImageDraw.Draw(bg)
    for y in range(H):
        k = y / H
        d.line([(0, y), (W, y)], fill=tuple(int(BG0[i] * (1 - k) + BG1[i] * k) for i in range(3)))
    glow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    g = ImageDraw.Draw(glow)
    g.ellipse([W * 0.55, -H * 0.4, W * 1.3, H * 0.6], fill=TEAL + (36,))
    g.ellipse([-W * 0.3, H * 0.5, W * 0.4, H * 1.4], fill=PURPLE + (34,))
    out = bg.convert("RGBA")
    out.alpha_composite(glow.filter(ImageFilter.GaussianBlur(160)))
    return out


BG = background()


def gradient_bar(w, h):
    """Horizontal teal -> purple bar (brand accent)."""
    bar = Image.new("RGBA", (w, h))
    d = ImageDraw.Draw(bar)
    for x in range(w):
        k = x / max(1, w - 1)
        d.line([(x, 0), (x, h)], fill=tuple(int(TEAL[i] * (1 - k) + PURPLE[i] * k) for i in range(3)) + (255,))
    m = Image.new("L", (w, h), 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, w - 1, h - 1], h // 2, fill=255)
    bar.putalpha(m)
    return bar


def rounded(img, r):
    m = Image.new("L", img.size, 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, img.size[0] - 1, img.size[1] - 1], r, fill=255)
    out = img.convert("RGBA")
    out.putalpha(Image.composite(out.getchannel("A"), Image.new("L", img.size, 0), m))
    return out


def with_alpha(layer, k):
    if k >= 0.999:
        return layer
    out = layer.copy()
    out.putalpha(out.getchannel("A").point(lambda a: int(a * k)))
    return out


def text_layer(lines):
    tmp = ImageDraw.Draw(Image.new("RGBA", (1, 1)))
    boxes = [tmp.textbbox((0, 0), t, font=f) for t, f, c, sp in lines]
    wid = max(b[2] for b in boxes) + 4
    hgt = sum(b[3] + sp for b, (t, f, c, sp) in zip(boxes, lines)) + 8
    layer = Image.new("RGBA", (wid, hgt), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    y = 0
    for (t, f, c, sp), b in zip(lines, boxes):
        d.text((0, y), t, font=f, fill=c)
        y += b[3] + sp
    return layer


def caption(title, sub, chip=None):
    lines = []
    if chip:
        lines.append((chip.upper(), font(22, "SemiBold"), TEAL, 18))
    tl = title.split("\n")
    for i, t in enumerate(tl):
        lines.append((t, font(58, "Bold"), FG, 8 if i < len(tl) - 1 else 22))
    for s in sub.split("\n"):
        lines.append((s, font(30), MUTED, 10))
    lay = text_layer(lines)
    # Brand accent under the chip.
    out = Image.new("RGBA", (lay.width, lay.height + 0), (0, 0, 0, 0))
    out.alpha_composite(lay)
    if chip:
        out.alpha_composite(gradient_bar(56, 5), (0, 34))
    return out


def place(frame, layer, x, y, k=1.0):
    if k > 0.003:
        frame.alpha_composite(with_alpha(layer, k), (int(x), int(y)))


def scaled(layer, s):
    return layer if abs(s - 1) < 1e-3 else layer.resize(
        (max(1, int(layer.width * s)), max(1, int(layer.height * s))), Image.LANCZOS)


def load(name):
    return Image.open(os.path.join(CAP, name)).convert("RGBA")


LOGO = load("logo.png")


def logo(size):
    return LOGO.resize((size, size), Image.LANCZOS)


def tui_image(view):
    pal = {"Reset": FG, "Cyan": (86, 214, 201), "Green": (122, 217, 122), "Yellow": (245, 196, 81),
           "Magenta": (199, 146, 234), "DarkGray": (110, 117, 135), "Red": (240, 113, 120),
           "Blue": (120, 160, 255), "White": (255, 255, 255), "Gray": (170, 175, 190)}
    bgc = (16, 19, 32)
    host = os.uname().nodename
    for l in open(os.path.join(CAP, "tui.jsonl")):
        d = json.loads(l)
        if d["view"] != view:
            continue
        for row in d["cells"]:  # hide the machine name
            line = "".join(c[0] for c in row)
            i = line.find(host)
            if i >= 0:
                for k, ch in enumerate("workstation".ljust(len(host))):
                    row[i + k][0] = ch
        fr, fb = font(20, "Regular", True), font(20, "Bold", True)
        cw, ch = fr.getbbox("M")[2], 27
        img = Image.new("RGBA", (d["w"] * cw + 40, d["h"] * ch + 30), bgc + (255,))
        dr = ImageDraw.Draw(img)
        for y, row in enumerate(d["cells"]):
            for x, (sym, fg, bg, bold, dim, rev) in enumerate(row):
                fgc = pal.get(fg, FG)
                bgcol = pal.get(bg) if bg != "Reset" else None
                if rev:
                    fgc, bgcol = (bgcol or bgc), fgc
                px, py = 20 + x * cw, 15 + y * ch
                if bgcol:
                    dr.rectangle([px, py, px + cw, py + ch], fill=bgcol)
                if sym.strip():
                    if dim:
                        fgc = tuple(int(c * 0.6) for c in fgc)
                    dr.text((px, py + 2), sym, font=fb if bold else fr, fill=fgc)
        return img
    raise SystemExit(f"no TUI view {view}")


def terminal_frame(img, title):
    bar = 44
    out = Image.new("RGBA", (img.width, img.height + bar), (30, 34, 50, 255))
    d = ImageDraw.Draw(out)
    for i, c in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        d.ellipse([18 + i * 26, 15, 32 + i * 26, 29], fill=c)
    tf = font(18, "Medium")
    d.text(((img.width - d.textlength(title, font=tf)) / 2, 11), title, font=tf, fill=MUTED)
    out.alpha_composite(img, (0, bar))
    return out


SCENES = []


def window_scene(img, title, sub, chip, zoom_to=None, side="right", zoom_at=1.5):
    cap = caption(title, sub, chip)
    img = rounded(img, 14)
    vx0, vx1 = (int(W * 0.40), int(W * 0.965)) if side == "right" else (int(W * 0.035), int(W * 0.60))
    vy0, vy1 = int(H * 0.09), int(H * 0.91)
    vw, vh = vx1 - vx0, vy1 - vy0
    fit = min(vw / img.width, vh / img.height) * 0.94

    def draw(frame, t, dur):
        k_in = ease_out(ramp(t, 0.0, 0.8))
        kz = ease_in_out(ramp(t, zoom_at, zoom_at + 1.2)) if zoom_to else 0.0
        s0 = fit * (1 + 0.03 * t / dur)
        c0 = (img.width / 2, img.height / 2)
        if zoom_to:
            x0, y0, x1, y1 = zoom_to
            s1 = max(min(vw / (x1 - x0), vh / (y1 - y0)) * 0.96, s0)
            c1 = ((x0 + x1) / 2, (y0 + y1) / 2)
        else:
            s1, c1 = s0, c0
        sc = s0 * (1 - kz) + s1 * kz
        cx = c0[0] * (1 - kz) + c1[0] * kz
        cy = c0[1] * (1 - kz) + c1[1] * kz
        sx0, sy0 = max(0.0, cx - vw / 2 / sc), max(0.0, cy - vh / 2 / sc)
        sx1, sy1 = min(img.width, cx + vw / 2 / sc), min(img.height, cy + vh / 2 / sc)
        crop = img.crop((int(sx0), int(sy0), int(math.ceil(sx1)), int(math.ceil(sy1))))
        crop = crop.resize((max(1, int(crop.width * sc)), max(1, int(crop.height * sc))), Image.LANCZOS)
        px = vx0 + vw / 2 - (cx - int(sx0)) * sc
        py = vy0 + vh / 2 - (cy - int(sy0)) * sc + (1 - k_in) * 90
        sh = Image.new("RGBA", (crop.width + 120, crop.height + 120), (0, 0, 0, 0))
        ImageDraw.Draw(sh).rounded_rectangle([60, 74, 60 + crop.width, 74 + crop.height], 18, fill=(0, 0, 0, 140))
        place(frame, sh.filter(ImageFilter.GaussianBlur(26)), px - 60, py - 60, k_in)
        if kz > 0.01:
            crop = rounded(crop, 18)
        place(frame, crop, px, py, k_in)
        capx = 110 if side == "right" else W - 110 - cap.width
        place(frame, cap, capx - (1 - k_in) * 40 * (1 if side == "right" else -1), H / 2 - cap.height / 2 - 30, k_in)
    return draw


def strip_bar(strip, scale):
    st = strip.resize((int(strip.width * scale / 3), int(strip.height * scale / 3)), Image.LANCZOS)
    bh = int(32 * scale)
    bar = Image.new("RGBA", (st.width + int(260 * scale), bh), (14, 14, 16, 255))
    ImageDraw.Draw(bar).text((int(20 * scale), int(8 * scale)), "Oct 7  10:24",
                             font=font(int(13.5 * scale), "Bold"), fill=(235, 235, 235))
    bar.alpha_composite(st, (int(150 * scale), (bh - st.height) // 2))
    return bar, int(150 * scale), st.width


def intro(frame, t, dur):
    k = ease_out(ramp(t, 0.2, 1.2))
    s = 0.8 + 0.2 * k
    lm = logo(int(200 * s))
    place(frame, lm, W / 2 - lm.width / 2, 210 - 30 * (1 - k), k)
    title = text_layer([("Now You See Me", font(96, "Bold"), FG, 0)])
    kt = ease_out(ramp(t, 0.5, 1.5))
    place(frame, title, W / 2 - title.width / 2, 460 + 20 * (1 - kt), kt)
    kb = ease_out(ramp(t, 0.8, 1.6))
    bar = gradient_bar(max(1, int(240 * kb)), 6)
    place(frame, bar, W / 2 - bar.width / 2, 590, kb)
    tag = text_layer([("A precise, lightweight resource monitor for Linux", font(38), MUTED, 0)])
    place(frame, tag, W / 2 - tag.width / 2, 625, ease_out(ramp(t, 0.9, 1.9)))
    chips = ["Top bar", "Desktop app", "Terminal UI", "CLI"]
    x = W / 2 - (len(chips) * 220) / 2
    for i, c in enumerate(chips):
        kc = ease_out(ramp(t, 1.5 + i * 0.15, 2.2 + i * 0.15))
        chip = Image.new("RGBA", (200, 56), (0, 0, 0, 0))
        dd = ImageDraw.Draw(chip)
        dd.rounded_rectangle([0, 0, 199, 55], 28, fill=(255, 255, 255, 18), outline=TEAL + (160,), width=2)
        f = font(24, "Medium")
        dd.text(((200 - dd.textlength(c, font=f)) / 2, 13), c, font=f, fill=FG)
        place(frame, chip, x + i * 220, 720 + 20 * (1 - kc), kc)


def main():
    strip_n = load("strip-names@3x.png")
    pg = {k: load(f"page-{k}.png") for k in
          ["overview", "cpu", "memory", "storage", "groups", "about", "overview-light"]}
    settings = load("scene-settings.png")
    settings = settings.crop((0, 0, settings.width, int(settings.height * 0.62)))
    tui = terminal_frame(tui_image("Overview"), "nysm tui")

    SCENES.append((4.0, intro))

    parts = [("CPU usage", 0.0), ("Memory in use", 0.15), ("Network ↓ ↑", 0.31), ("Storage used", 0.50),
             ("Disk activity", 0.66), ("Disk read / write", 0.80)]

    def topbar(frame, t, dur):
        cap = caption("Live in your top bar", "CPU, memory, network, storage and disk —\nfixed width, so nothing jumps.", "Top bar")
        place(frame, cap, W / 2 - cap.width / 2, 130, ease_out(ramp(t, 0, 0.8)))
        bar, sx, sw = strip_bar(strip_n, 1.9)
        bx, by = W / 2 - bar.width / 2, 540
        kb = ease_out(ramp(t, 0.3, 1.1))
        sh = Image.new("RGBA", (bar.width + 80, bar.height + 80), (0, 0, 0, 0))
        ImageDraw.Draw(sh).rounded_rectangle([40, 52, 40 + bar.width, 52 + bar.height], 16, fill=(0, 0, 0, 150))
        place(frame, sh.filter(ImageFilter.GaussianBlur(20)), bx - 40, by - 40 + 40 * (1 - kb), kb)
        place(frame, rounded(bar, 16), bx, by + 40 * (1 - kb), kb)
        n = len(parts)
        seg = (dur - 2.0) / n
        if t > 1.4:
            i = int(clamp((t - 1.4) / seg, 0, n - 1))
            name, frac = parts[i]
            nxt = parts[i + 1][1] if i + 1 < n else 1.0
            hx0, hx1 = bx + sx + sw * frac - 10, bx + sx + sw * nxt - 18
            kk = ease_out(ramp(t, 1.4 + i * seg, 1.4 + i * seg + 0.35))
            hl = Image.new("RGBA", (int(hx1 - hx0), bar.height + 20), (0, 0, 0, 0))
            ImageDraw.Draw(hl).rounded_rectangle([0, 0, hl.width - 1, hl.height - 1], 14, outline=TEAL + (230,), width=4)
            place(frame, hl, hx0, by - 10, kk)
            lab = text_layer([(name, font(40, "SemiBold"), TEAL, 0)])
            place(frame, lab, (hx0 + hx1) / 2 - lab.width / 2, by + bar.height + 40, kk)

    SCENES.append((7.0, topbar))
    # Zoom regions are in capture pixels (1024x768 windows).
    SCENES.append((4.8, window_scene(pg["overview"], "Everything\nat a glance",
                                     "CPU, memory, network, storage and disks\nwith minutes of history.", "Desktop app")))
    SCENES.append((4.6, window_scene(pg["cpu"], "Every core,\nexplained",
                                     "Usage split by kind, load and pressure,\nand a live chart per core.", "CPU",
                                     zoom_to=(205, 40, 1024, 330), side="left")))
    SCENES.append((4.2, window_scene(pg["memory"], "Memory, structured",
                                     "Used, available, cache, swap\nand memory pressure.", "Memory",
                                     zoom_to=(205, 40, 1024, 250))))
    SCENES.append((5.6, window_scene(load("scene-group.png"), "Which app uses\nthe most memory?",
                                     "Group processes by app — and see real\nmemory (PSS), not double-counted RSS.", "Memory by app",
                                     zoom_to=(205, 90, 1024, 700), side="left")))
    SCENES.append((5.2, window_scene(load("scene-watch.png"), "Select & watch\nprocesses",
                                     "Pin several processes and follow their\nCPU and memory — plus live details.", "Processes",
                                     zoom_to=(205, 150, 1024, 600))))
    SCENES.append((4.6, window_scene(pg["storage"], "Is the disk\nfilling up?",
                                     "Throughput, latency, every volume\nand its growth trend.", "Storage",
                                     zoom_to=(205, 330, 1024, 470), side="left")))
    SCENES.append((4.6, window_scene(pg["groups"], "Containers\n& services",
                                     "Docker, systemd services and apps —\nwith real container names.", "cgroups",
                                     zoom_to=(205, 150, 1024, 560))))
    SCENES.append((4.2, window_scene(settings, "Make it yours",
                                     "History length with its cost, and exactly\nwhat the top bar shows.", "Settings", side="left")))
    SCENES.append((3.6, window_scene(pg["overview-light"], "Light or dark",
                                     "Follows your desktop theme,\nor pick one.", "Themes")))
    SCENES.append((4.8, window_scene(tui, "Same data in\nthe terminal",
                                     "nysm tui — and over SSH:\nnysm tui --remote user@host", "Terminal UI", side="left")))

    summ = open(os.path.join(CAP, "cli-summary.txt")).read().splitlines()[:13]
    net = open(os.path.join(CAP, "cli-net.txt")).read().splitlines()
    sessions = [("nysm summary", summ), ("nysm net check github.com", net)]

    def cli(frame, t, dur):
        cap = caption("Scriptable CLI", "One-shot answers, JSON for scripts,\nrecordings and comparisons.", "Command line")
        place(frame, cap, 120, H / 2 - cap.height / 2 - 40, ease_out(ramp(t, 0, 0.8)))
        term = Image.new("RGBA", (1060, 760), (16, 19, 32, 255))
        d = ImageDraw.Draw(term)
        f, fb = font(19, "Regular", True), font(19, "Bold", True)
        y, clock = 18, 0.6
        for cmd, out in sessions:
            if t < clock:
                break
            typed = int(clamp((t - clock) / 0.045, 0, len(cmd)))
            d.text((20, y), "$ ", font=fb, fill=TEAL)
            d.text((44, y), cmd[:typed] + ("▍" if typed < len(cmd) else ""), font=fb, fill=FG)
            y += 30
            clock += len(cmd) * 0.045 + 0.25
            for line in out[:int(clamp((t - clock) / 0.06, 0, len(out)))]:
                d.text((20, y), line[:96], font=f, fill=(200, 205, 220))
                y += 26
            clock += len(out) * 0.06 + 0.6
            y += 14
        term = terminal_frame(term, "bash")
        k = ease_out(ramp(t, 0.2, 1.0))
        sh = Image.new("RGBA", (term.width + 120, term.height + 120), (0, 0, 0, 0))
        ImageDraw.Draw(sh).rounded_rectangle([60, 74, 60 + term.width, 74 + term.height], 16, fill=(0, 0, 0, 140))
        tx, ty = W - term.width - 70, H / 2 - term.height / 2 + 60 * (1 - k)
        place(frame, sh.filter(ImageFilter.GaussianBlur(26)), tx - 60, ty - 60, k)
        place(frame, rounded(term, 14), tx, ty, k)

    SCENES.append((8.5, cli))
    SCENES.append((4.8, window_scene(pg["about"], "Tiny footprint,\nmeasured live",
                                     "The About page shows what the app\ncosts right now.", "About",
                                     zoom_to=(205, 470, 1024, 768), side="left", zoom_at=1.4)))

    def outro(frame, t, dur):
        k = ease_out(ramp(t, 0.1, 1.0))
        lm = logo(170)
        place(frame, lm, W / 2 - 85, 200 - 30 * (1 - k), k)
        title = text_layer([("Now You See Me", font(88, "Bold"), FG, 0)])
        place(frame, title, W / 2 - title.width / 2, 420, ease_out(ramp(t, 0.3, 1.2)))
        kb = ease_out(ramp(t, 0.5, 1.3))
        bar = gradient_bar(max(1, int(220 * kb)), 6)
        place(frame, bar, W / 2 - bar.width / 2, 540, kb)
        sub = text_layer([("Open source · MIT OR Apache-2.0 · offline · no telemetry", font(34), MUTED, 0)])
        place(frame, sub, W / 2 - sub.width / 2, 575, ease_out(ramp(t, 0.6, 1.5)))
        url = text_layer([("github.com/sheheemmulakkal/now-you-see-me", font(36, "SemiBold"), TEAL, 0)])
        place(frame, url, W / 2 - url.width / 2, 660, ease_out(ramp(t, 0.9, 1.8)))

    SCENES.append((5.0, outro))

    if PREVIEW:
        for si, (dur, fn) in enumerate(SCENES):
            frame = BG.copy()
            layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
            fn(layer, dur * 0.72, dur)
            frame.alpha_composite(layer)
            frame.convert("RGB").resize((960, 540), Image.LANCZOS).save(os.path.join(OUT, f"preview-{si:02d}.png"))
        print(f"previews in {OUT}")
        return

    total = sum(d for d, _ in SCENES)
    print(f"{len(SCENES)} scenes, {total:.1f} s", file=sys.stderr)
    mp4 = os.path.join(OUT, "now-you-see-me.mp4")
    ff = subprocess.Popen(["ffmpeg", "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24",
                           "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-", "-c:v", "libx264", "-preset", "slow",
                           "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", mp4],
                          stdin=subprocess.PIPE)
    fade = 0.45
    for si, (dur, fn) in enumerate(SCENES):
        nfr = int(round(dur * FPS))
        for fi in range(nfr):
            t = fi / FPS
            frame = BG.copy()
            layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
            fn(layer, t, dur)
            k = min(ramp(t, 0, fade) if si > 0 else 1.0,
                    1 - ramp(t, dur - fade, dur) if si < len(SCENES) - 1 else 1.0)
            frame.alpha_composite(with_alpha(layer, k))
            rgb = frame.convert("RGB")
            ff.stdin.write(rgb.tobytes())
            if si == 1 and fi == int(nfr * 0.8):
                rgb.save(os.path.join(OUT, "poster.png"))
        print(f"  scene {si + 1}/{len(SCENES)}", file=sys.stderr)
    ff.stdin.close()
    ff.wait()
    print(mp4)


if __name__ == "__main__":
    main()
