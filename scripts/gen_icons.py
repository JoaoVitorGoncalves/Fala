"""
Gera os ícones do Fala (design v2) com Pillow — sem rasterizador SVG.

Design (viewBox 128 do mockup):
  tile escuro arredondado com hairline → cápsula do mic em gradiente prata
  → suporte no acento azul elétrico → haste/base prata → ticks coral (só app).

Saídas:
  src-tauri/icons/32x32.png, 128x128.png, 128x128@2x.png, icon.ico
  src-tauri/icons/tray/tray-{idle,rec,busy,polish,ok,err}.png (32px, glifo + dot)

Uso: python scripts/gen_icons.py
"""

import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"

# paleta do design v2
TILE_TOP = (0x1C, 0x1C, 0x23)
TILE_BOTTOM = (0x10, 0x10, 0x14)
HAIRLINE = (255, 255, 255, 14)  # #ffffff14
SILVER_TOP = (0xF4, 0xF4, 0xF7)
SILVER_BOTTOM = (0xB9, 0xB9, 0xC4)
SILVER_FLAT = (0xEC, 0xEC, 0xF1)
STEM = (0xD5, 0xD5, 0xDD)
ACCENT = (0x5E, 0xB1, 0xFF, 255)
CORAL = (0xFF, 0x5C, 0x5C, 255)
DARK_TILE = (0x17, 0x17, 0x1D, 255)

SS = 8  # supersampling: master 128*8 = 1024

STATE_COLORS = {
    "idle": (0x5F, 0x5F, 0x6B, 255),
    "rec": (0xFF, 0x5C, 0x5C, 255),
    "busy": (0xFF, 0xB4, 0x54, 255),
    "polish": (0xF2, 0x7D, 0xA8, 255),
    "ok": (0x3D, 0xDC, 0x97, 255),
    "err": (0xE5, 0x48, 0x4D, 255),
}


def vgrad(size: int, top, bottom) -> Image.Image:
    col = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        c = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
        col.putpixel((0, y), c)
    return col.resize((size, size))


def rmask(size: int, box, radius: int) -> Image.Image:
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle(box, radius=radius, fill=255)
    return m


def round_line(d: ImageDraw.ImageDraw, p1, p2, fill, width: int):
    """Linha com pontas arredondadas (PIL não tem line cap)."""
    d.line([p1, p2], fill=fill, width=width)
    r = width / 2
    for (x, y) in (p1, p2):
        d.ellipse((x - r, y - r, x + r, y + r), fill=fill)


def draw_app_icon(s: int, ticks: bool = True) -> Image.Image:
    """Tile + mic; s = escala (128 * SS). Coordenadas em unidades do viewBox."""
    size = 128 * s
    img = vgrad(size, TILE_TOP, TILE_BOTTOM).convert("RGBA")

    # tile arredondado com hairline
    mask = rmask(size, (4 * s, 4 * s, 124 * s, 124 * s), 30 * s)
    base = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    base.paste(img, (0, 0), mask)
    d = ImageDraw.Draw(base)
    d.rounded_rectangle(
        (4 * s, 4 * s, 124 * s, 124 * s), radius=30 * s, outline=HAIRLINE, width=max(1, s // 2)
    )

    # cápsula do microfone (gradiente prata)
    mic_grad = vgrad(size, SILVER_TOP, SILVER_BOTTOM).convert("RGBA")
    mic_mask = rmask(size, (51 * s, 26 * s, 77 * s, 68 * s), 13 * s)
    mic = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    mic.paste(mic_grad, (0, 0), mic_mask)
    base.alpha_composite(mic)

    d = ImageDraw.Draw(base)
    w = 5 * s
    # suporte em arco (acento) — bbox do path M40 62 a24 24 0 0 0 48 0
    d.arc((40 * s, 38 * s, 88 * s, 86 * s), start=0, end=180, fill=ACCENT, width=w)
    r = w / 2
    for cx, cy in ((40 * s, 62 * s), (88 * s, 62 * s)):
        d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=ACCENT)

    silver = STEM + (255,)
    round_line(d, (64 * s, 86 * s), (64 * s, 96 * s), silver, w)
    round_line(d, (52 * s, 100 * s), (76 * s, 100 * s), silver, w)

    if ticks:
        cw = max(1, int(3.5 * s))
        round_line(d, (88 * s, 38 * s), (88 * s, 48 * s), CORAL, cw)
        round_line(d, (95 * s, 33 * s), (95 * s, 53 * s), CORAL, cw)

    return base


def draw_tray_icon(state: str, s: int) -> Image.Image:
    """Glifo monocromático + dot de status no canto (viewBox 128)."""
    size = 128 * s
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((4 * s, 4 * s, 124 * s, 124 * s), radius=30 * s, fill=DARK_TILE)
    d.rounded_rectangle((4 * s, 4 * s, 124 * s, 124 * s), radius=30 * s, outline=HAIRLINE, width=max(1, s // 2))

    silver = SILVER_FLAT + (255,)
    w = 6 * s
    # cápsula flat
    d.rounded_rectangle((51 * s, 26 * s, 77 * s, 68 * s), radius=13 * s, fill=silver)
    d.arc((40 * s, 38 * s, 88 * s, 86 * s), start=0, end=180, fill=silver, width=w)
    for cx, cy in ((40 * s, 62 * s), (88 * s, 62 * s)):
        r = w / 2
        d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=silver)
    round_line(d, (64 * s, 86 * s), (64 * s, 96 * s), silver, w)
    round_line(d, (52 * s, 100 * s), (76 * s, 100 * s), silver, w)

    # dot de status
    color = STATE_COLORS[state]
    r = 18 * s
    d.ellipse((100 * s - r, 100 * s - r, 100 * s + r, 100 * s + r), fill=color)
    d.ellipse(
        (100 * s - r, 100 * s - r, 100 * s + r, 100 * s + r),
        outline=DARK_TILE,
        width=6 * s,
    )
    if state == "err":
        # "!" branco sobre o badge
        bw = 7 * s
        round_line(d, (100 * s, 90 * s), (100 * s, 103 * s), (255, 255, 255, 255), bw)
        d.ellipse((100 * s - bw / 2, 108 * s, 100 * s + bw / 2, 108 * s + bw), fill=(255, 255, 255, 255))

    return img


def main():
    ICONS.mkdir(exist_ok=True)
    (ICONS / "tray").mkdir(exist_ok=True)

    master = draw_app_icon(SS)
    out = {
        "32x32.png": (32, 32),
        "128x128.png": (128, 128),
        "128x128@2x.png": (256, 256),
    }
    for name, size in out.items():
        master.resize(size, Image.LANCZOS).save(ICONS / name)
    master.resize((256, 256), Image.LANCZOS).save(
        ICONS / "icon.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print("ícones do app: 32x32, 128x128, 128x128@2x, icon.ico OK")

    for state in STATE_COLORS:
        draw_tray_icon(state, SS).resize((32, 32), Image.LANCZOS).save(ICONS / "tray" / f"tray-{state}.png")
    print("tray: tray-{idle,rec,busy,polish,ok,err}.png OK")

    iconutil = shutil.which("iconutil")
    if iconutil:
        with tempfile.TemporaryDirectory() as tmp:
            iconset = Path(tmp) / "Fala.iconset"
            iconset.mkdir()
            icns_sizes = [
                ("icon_16x16.png", 16),
                ("icon_16x16@2x.png", 32),
                ("icon_32x32.png", 32),
                ("icon_32x32@2x.png", 64),
                ("icon_128x128.png", 128),
                ("icon_128x128@2x.png", 256),
                ("icon_256x256.png", 256),
                ("icon_256x256@2x.png", 512),
                ("icon_512x512.png", 512),
                ("icon_512x512@2x.png", 1024),
            ]
            for fname, px in icns_sizes:
                master.resize((px, px), Image.LANCZOS).save(iconset / fname)
            subprocess.run([iconutil, "-c", "icns", str(iconset), "-o", str(ICONS / "icon.icns")], check=True)
        print("macOS: icon.icns OK")


if __name__ == "__main__":
    main()
