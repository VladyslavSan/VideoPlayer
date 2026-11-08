"""Draws the app icon. Usage: python scripts/make_icon.py

Writes assets/app.ico (16-256 px, embedded in the exe) and assets/app.png
(1024 px preview). Everything is drawn at 4x and downsampled for smooth edges.
"""
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
SIZE = 1024
SS = 4  # supersampling factor
S = SIZE * SS


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def rounded_square_mask(size, radius):
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius, fill=255)
    return mask


def gradient(size, top_left, bottom_right):
    # Diagonal gradient, computed on a small image and scaled up (it's smooth anyway).
    n = 256
    small = Image.new("RGB", (n, n))
    px = small.load()
    for y in range(n):
        for x in range(n):
            px[x, y] = lerp(top_left, bottom_right, (x + y) / (2 * (n - 1)))
    return small.resize((size, size), Image.BICUBIC)


def rounded_triangle(draw, points, radius, fill):
    """Triangle with rounded corners: shrink toward the centre, then dilate."""
    cx = sum(p[0] for p in points) / 3
    cy = sum(p[1] for p in points) / 3
    inner = []
    for x, y in points:
        dx, dy = x - cx, y - cy
        d = math.hypot(dx, dy)
        # Moving a vertex inward by 2r keeps the dilated edges where they were.
        k = (d - 2 * radius) / d
        inner.append((cx + dx * k, cy + dy * k))
    draw.polygon(inner, fill=fill)
    draw.line(inner + [inner[0]], fill=fill, width=round(2 * radius), joint="curve")
    for x, y in inner:
        draw.ellipse((x - radius, y - radius, x + radius, y + radius), fill=fill)


def draw_icon(ring=True):
    """`ring=False` is the simplified version for tiny sizes."""
    corner = S * 0.225
    base = gradient(S, (76, 141, 255), (122, 64, 240)).convert("RGBA")

    # Soft light from the top for depth: a linear fade over the upper half.
    fade = Image.linear_gradient("L").resize((S, S))  # 0 at top, 255 at bottom
    highlight = fade.point(lambda v: max(0, round(60 * (1 - v / 140))))
    white = Image.new("RGBA", (S, S), (255, 255, 255, 255))
    icon = Image.composite(white, base, highlight)
    icon.putalpha(rounded_square_mask(S, corner))

    centre = S / 2
    ring_r = S * 0.335
    ring_w = S * 0.05
    box = (centre - ring_r, centre - ring_r, centre + ring_r, centre + ring_r)

    # Resume ring: a faint full track and a bright three-quarter arc.
    ring_layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    rd = ImageDraw.Draw(ring_layer)
    rd.ellipse(box, outline=(255, 255, 255, 70), width=round(ring_w))
    rd.arc(box, start=-90, end=180, fill=(255, 255, 255, 255), width=round(ring_w))
    # Round caps on the bright arc.
    for angle in (-90, 180):
        a = math.radians(angle)
        r_mid = ring_r - ring_w / 2
        x, y = centre + r_mid * math.cos(a), centre + r_mid * math.sin(a)
        rd.ellipse((x - ring_w / 2, y - ring_w / 2, x + ring_w / 2, y + ring_w / 2),
                   fill=(255, 255, 255, 255))

    # Play triangle, nudged right so it looks centred.
    h = S * (0.30 if ring else 0.46)
    w = h * 0.92
    left = centre - w * 0.42
    tri = [(left, centre - h / 2), (left, centre + h / 2), (left + w, centre)]
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    rounded_triangle(ImageDraw.Draw(shadow), [(x, y + S * 0.012) for x, y in tri],
                     S * 0.035, (20, 10, 60, 110))
    shadow = shadow.filter(ImageFilter.GaussianBlur(S * 0.018))
    play = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    rounded_triangle(ImageDraw.Draw(play), tri, S * 0.035, (255, 255, 255, 255))

    layers = (ring_layer, shadow, play) if ring else (shadow, play)
    for layer in layers:
        icon = Image.alpha_composite(icon, layer)
    return icon.resize((SIZE, SIZE), Image.LANCZOS)


def main():
    icon = draw_icon()
    small = draw_icon(ring=False)
    assets = ROOT / "assets"
    icon.save(assets / "app.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
    frames = [(small if s <= 24 else icon).resize((s, s), Image.LANCZOS) for s in sizes]
    frames[-1].save(assets / "app.ico", sizes=[(s, s) for s in sizes],
                    append_images=frames[:-1])
    print("wrote", assets / "app.ico", "and", assets / "app.png")


if __name__ == "__main__":
    main()
