"""Generate circular versions of the Trigr logo for social profile icons.

Source logos are rounded squares whose white inner tile gets clipped when
platforms apply a circular crop. This script rebuilds each size as a clean
gradient circle with the white tile centered safely inside.
"""
import numpy as np
from PIL import Image, ImageDraw, ImageChops

TOP_COLOR = (238, 182, 63)   # sampled from y=60 of trigr-logo-1024.png
BOT_COLOR = (202, 137, 13)   # sampled from y=960 of trigr-logo-1024.png

# White tile bounds within the 1024 source, including drop shadow
TILE_BBOX_1024 = (123, 115, 900, 800)
TILE_RADIUS_1024 = 90

SAFETY = 0.98  # scale tile so corners sit ~2% inside the inscribed circle


def make_gradient_circle(size, top_rgb, bot_rgb, supersample=4):
    s = supersample
    sH = size * s
    top = np.array(top_rgb + (255,), dtype=np.float64)
    bot = np.array(bot_rgb + (255,), dtype=np.float64)
    t = np.linspace(0, 1, sH).reshape(sH, 1, 1)
    grad = (top + (bot - top) * t).astype(np.uint8)
    grad = np.broadcast_to(grad, (sH, sH, 4)).copy()
    grad_img = Image.fromarray(grad, 'RGBA')
    mask = Image.new('L', (sH, sH), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, sH, sH), fill=255)
    grad_img.putalpha(mask)
    return grad_img.resize((size, size), Image.LANCZOS)


def make_rounded_logo(src_path, out_path, size):
    src = Image.open(src_path).convert('RGBA')
    if src.size != (size, size):
        src = src.resize((size, size), Image.LANCZOS)

    circle = make_gradient_circle(size, TOP_COLOR, BOT_COLOR)

    scale = size / 1024.0
    x1, y1, x2, y2 = (int(v * scale) for v in TILE_BBOX_1024)
    radius = max(1, int(TILE_RADIUS_1024 * scale))

    # Mask source to just the white tile (rounded-rect)
    tile_mask = Image.new('L', src.size, 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle((x1, y1, x2, y2), radius=radius, fill=255)
    masked = src.copy()
    masked.putalpha(ImageChops.multiply(src.split()[-1], tile_mask))
    tile = masked.crop((x1, y1, x2, y2))

    tw, th = tile.size
    half_diag = ((tw ** 2 + th ** 2) ** 0.5) / 2
    safe_r = (size / 2) * SAFETY
    if half_diag > safe_r:
        f = safe_r / half_diag
        tile = tile.resize((max(1, int(tw * f)), max(1, int(th * f))), Image.LANCZOS)

    tw, th = tile.size
    circle.alpha_composite(tile, ((size - tw) // 2, (size - th) // 2))
    circle.save(out_path, optimize=True)


if __name__ == '__main__':
    import os
    here = os.path.dirname(os.path.abspath(__file__))
    for s in [200, 400, 512, 800, 1024, 1080]:
        src = os.path.join(here, f'trigr-logo-{s}.png')
        out = os.path.join(here, f'trigr-logo-circle-{s}.png')
        make_rounded_logo(src, out, s)
        print(f'Wrote {out}')
