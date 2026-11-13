"""Place the 390x508 keyboard-open capture on a 390x844 canvas with a
labelled stand-in for the iOS keyboard (headless Chrome has no soft
keyboard; the app's layout is driven by visualViewport height, which the
shrunken viewport reproduces)."""
import sys
from PIL import Image, ImageDraw

raw, out = sys.argv[1], sys.argv[2]
top = Image.open(raw).convert("RGB")
scale = top.width / 390
W, H = top.width, int(844 * scale)
canvas = Image.new("RGB", (W, H), (44, 44, 46))
canvas.paste(top, (0, 0))
d = ImageDraw.Draw(canvas)
ky = top.height
rows = [10, 9, 7]
key_h = int(42 * scale)
gap = int(6 * scale)
y = ky + int(14 * scale)
for n in rows:
    kw = (W - gap * (n + 1)) / max(n, 10)
    x = (W - (kw * n + gap * (n - 1))) / 2
    for _ in range(n):
        d.rounded_rectangle([x, y, x + kw, y + key_h], radius=int(5 * scale), fill=(96, 96, 100))
        x += kw + gap
    y += key_h + int(12 * scale)
d.rounded_rectangle([W * 0.25, y, W * 0.75, y + key_h], radius=int(5 * scale), fill=(96, 96, 100))
d.text((int(12 * scale), H - int(26 * scale)), "simulated keyboard (336 px) - visualViewport height 508", fill=(170, 170, 175))
canvas.save(out)
