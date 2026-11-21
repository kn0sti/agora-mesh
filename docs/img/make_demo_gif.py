#!/usr/bin/env python3
"""Records a real `agora demo` and draws it as a terminal animation.

Run: python3 docs/img/make_demo_gif.py target/release/agora
Needs Pillow and the font DejaVu Sans Mono (or its path in DEMO_FONT).
Nothing in the picture is typed by hand: every line is what the command printed.
"""
import os
import subprocess
import sys
import textwrap
import time

from PIL import Image, ImageDraw, ImageFont

WIDTH, HEIGHT = 1000, 640
COLUMNS, ROWS = 104, 30
SIZE, LINE = 15, 19
PAD_X, PAD_TOP = 22, 52
FONT_DIR = "/usr/share/fonts/truetype/dejavu"
REGULAR = os.environ.get("DEMO_FONT", f"{FONT_DIR}/DejaVuSansMono.ttf")
BOLD = os.environ.get("DEMO_FONT_BOLD", f"{FONT_DIR}/DejaVuSansMono-Bold.ttf")
BG, BAR, TEXT, DIM = "#101418", "#1b2128", "#d7dde3", "#7d8894"
CYAN, MAGENTA, GREEN, WHITE = "#4fd1e8", "#e78ad8", "#6fdc8c", "#ffffff"


def record(binary):
    """Runs the demo and returns batches of the lines printed together."""
    env = dict(os.environ, NO_COLOR="1", PYTHONUNBUFFERED="1")
    proc = subprocess.Popen([binary, "demo"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
    batches, last = [], time.time()
    for line in proc.stdout:
        now = time.time()
        if not batches or now - last > 0.12:
            batches.append([])
        batches[-1].append(line.rstrip("\n"))
        last = now
    if proc.wait() != 0:
        sys.exit("the demo failed:\n" + "\n".join(sum(batches, [])))
    return batches


def styled(line):
    """Colour of a line, from what the demo prints."""
    s = line.strip()
    if line.startswith("[") and "/4]" in line[:6] or line.startswith("agora demo") or s == "That was a room.":
        return WHITE, True
    if line.startswith("  Laptop "):
        return CYAN, True
    if line.startswith("  Desktop  "):
        return MAGENTA, True
    if s.startswith(("The daemons", "Everything happens", "A peer you connect")):
        return DIM, False
    if s.startswith("Both branches merge") or " and pushed." in s:
        return GREEN, False
    return TEXT, False


def wrap(line):
    if len(line) <= COLUMNS:
        return [line]
    indent = " " * (len(line) - len(line.lstrip()))
    return textwrap.wrap(line.strip(), COLUMNS, initial_indent=indent, subsequent_indent=indent, break_long_words=False)


def palette():
    """One palette for all frames, so that a colour stays the same from frame to frame."""
    rgb = lambda c: [int(c[i : i + 2], 16) for i in (1, 3, 5)]
    base = rgb(BG)
    colours = [TEXT, DIM, CYAN, MAGENTA, GREEN, WHITE, "#ff5f57", "#febc2e", "#28c840"]
    values = list(base) + rgb(BAR)
    for c in colours:
        for step in range(1, 25):
            values += [round(b + (x - b) * step / 24) for b, x in zip(base, rgb(c))]
    img = Image.new("P", (1, 1))
    img.putpalette(values + [0] * (768 - len(values)))
    return img


PALETTE = palette()


def frame(rows, fonts):
    img = Image.new("RGB", (WIDTH, HEIGHT), BG)
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, WIDTH, 34], fill=BAR)
    for i, c in enumerate(("#ff5f57", "#febc2e", "#28c840")):
        d.ellipse([16 + i * 22, 11, 28 + i * 22, 23], fill=c)
    d.text((WIDTH / 2, 17), "agora demo", font=fonts[0], fill=DIM, anchor="mm")
    y = PAD_TOP
    for text, (colour, bold) in rows[-ROWS:]:
        d.text((PAD_X, y), text, font=fonts[1 if bold else 0], fill=colour)
        y += LINE
    return img.quantize(palette=PALETTE, dither=Image.Dither.NONE)


def main():
    binary, out = sys.argv[1], os.path.join(os.path.dirname(os.path.abspath(__file__)), "demo.gif")
    fonts = (ImageFont.truetype(REGULAR, SIZE), ImageFont.truetype(BOLD, SIZE))
    rows, frames, times = [("$ agora demo", (GREEN, True))], [], []
    frames.append(frame(rows, fonts)); times.append(900)
    for batch in record(binary):
        for line in batch:
            style = styled(line)
            rows += [(part, style) for part in wrap(line)] or [("", style)]
        frames.append(frame(rows, fonts))
        words = sum(len(l.split()) for l in batch)
        times.append(min(3200, 900 + words * 110))
    times[-1] = 9000
    frames[0].save(out, save_all=True, append_images=frames[1:], duration=times, loop=0, optimize=True, disposal=1)
    print(f"{out}: {len(frames)} frames, {sum(times) / 1000:.0f} s, {os.path.getsize(out) / 1024:.0f} KB")
    frames[-1].convert("RGB").save(os.path.join(os.path.dirname(out), "demo-last.png"))
    frames[len(frames) // 2].convert("RGB").save(os.path.join(os.path.dirname(out), "demo-mid.png"))


main()
