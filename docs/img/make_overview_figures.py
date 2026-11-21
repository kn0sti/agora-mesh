#!/usr/bin/env python3
"""Draws the explanatory figures at the top of the README.

Run: python3 docs/img/make_overview_figures.py
Needs Pillow and the fonts Noto Sans and DejaVu Sans Mono (or their folders in
FIG_NOTO and FIG_DEJAVU). The figures carry their own dark background, so they
read the same on GitHub's light and dark themes and in its mobile app.
"""
import os

from PIL import Image, ImageDraw, ImageFont

OUT = os.path.dirname(os.path.abspath(__file__))
NOTO = os.environ.get("FIG_NOTO", "/usr/share/fonts/truetype/noto")
DEJAVU = os.environ.get("FIG_DEJAVU", "/usr/share/fonts/truetype/dejavu")
SCALE = 2  # drawn at twice the size, then reduced, for smooth edges

BG, CARD, CARD2, LINE = "#101418", "#1a2129", "#232c36", "#36424e"
TEXT, SOFT, DIM = "#f1f5f8", "#c3ccd5", "#8c98a4"
CYAN, PINK, GREEN, AMBER = "#4fd1e8", "#e78ad8", "#6fdc8c", "#f5b84b"


def font(kind, size):
    path = {
        "bold": f"{NOTO}/NotoSans-Bold.ttf",
        "regular": f"{NOTO}/NotoSans-Regular.ttf",
        "mono": f"{DEJAVU}/DejaVuSansMono.ttf",
    }[kind]
    return ImageFont.truetype(path, size * SCALE)


class Fig:
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.img = Image.new("RGBA", (w * SCALE, h * SCALE), BG)
        self.d = ImageDraw.Draw(self.img)

    def box(self, x, y, w, h, fill=CARD, outline=LINE, width=2, r=20):
        self.d.rounded_rectangle([x * SCALE, y * SCALE, (x + w) * SCALE, (y + h) * SCALE], radius=r * SCALE, fill=fill, outline=outline, width=width * SCALE)

    def text(self, x, y, s, kind="regular", size=30, fill=TEXT, anchor="la"):
        self.d.text((x * SCALE, y * SCALE), s, font=font(kind, size), fill=fill, anchor=anchor)

    def line(self, pts, fill=LINE, width=4):
        self.d.line([(x * SCALE, y * SCALE) for x, y in pts], fill=fill, width=width * SCALE, joint="curve")

    def head(self, x, y, direction, fill, size=15):
        dx, dy = {"r": (1, 0), "l": (-1, 0), "d": (0, 1), "u": (0, -1)}[direction]
        px, py = -dy, dx
        pts = [(x, y), (x - dx * size * 1.5 + px * size, y - dy * size * 1.5 + py * size), (x - dx * size * 1.5 - px * size, y - dy * size * 1.5 - py * size)]
        self.d.polygon([(a * SCALE, b * SCALE) for a, b in pts], fill=fill)

    def pill(self, cx, cy, s, fill=CARD2, colour=TEXT, kind="mono", size=24, pad=22, h=50):
        w = self.d.textlength(s, font=font(kind, size)) / SCALE + pad * 2
        self.box(cx - w / 2, cy - h / 2, w, h, fill=fill, outline=fill, r=h / 2 if h < 40 else 12)
        self.text(cx, cy, s, kind, size, colour, "mm")
        return w

    def save(self, name):
        small = self.img.resize((self.w, self.h), Image.Resampling.LANCZOS)
        mask = Image.new("L", (self.w * SCALE, self.h * SCALE), 0)
        ImageDraw.Draw(mask).rounded_rectangle([0, 0, self.w * SCALE - 1, self.h * SCALE - 1], radius=28 * SCALE, fill=255)
        small.putalpha(mask.resize((self.w, self.h), Image.Resampling.LANCZOS))
        small.save(os.path.join(OUT, name), optimize=True)


def machine(f, x, y, w, h, name, agent, colour):
    f.box(x, y, w, h, outline=colour, width=3)
    f.text(x + w / 2, y + 44, name, "bold", 38, colour, "mm")
    f.text(x + w / 2, y + 100, agent, "bold", 30, TEXT, "mm")
    f.text(x + w / 2, y + 146, "its own login", "regular", 27, SOFT, "mm")
    f.text(x + w / 2, y + 186, "its own git clone", "regular", 27, SOFT, "mm")


def overview():
    f = Fig(1600, 800)
    f.text(800, 66, "One task, shared across your machines", "bold", 46, TEXT, "mm")
    cw, ch, top = 470, 232, 134
    lx, rx = 90, 1600 - 90 - cw
    machine(f, lx, top, cw, ch, "Laptop", "Claude Code", CYAN)
    machine(f, rx, top, cw, ch, "Desktop", "Codex", PINK)
    my = top + ch / 2 - 8
    f.line([(lx + cw + 24, my), (rx - 24, my)], GREEN, 5)
    f.head(lx + cw + 14, my, "l", GREEN)
    f.head(rx - 14, my, "r", GREEN)
    f.text(800, my - 48, "plan and critique", "bold", 33, GREEN, "mm")
    f.text(800, my + 44, "encrypted link", "regular", 26, SOFT, "mm")
    for cx, colour, branch in ((lx + cw / 2, CYAN, "branch: laptop"), (rx + cw / 2, PINK, "branch: desktop")):
        f.line([(cx, top + ch + 8), (cx, 462)], colour, 4)
        f.pill(cx, 494, branch, fill=CARD2, colour=colour, size=26, h=58)
        f.line([(cx, 526), (cx, 610)], colour, 4)
        f.head(cx, 626, "d", colour)
    f.text(lx + cw / 2 + 24, 570, "builds its part", "regular", 26, SOFT, "lm")
    f.text(rx + cw / 2 - 24, 570, "builds its part", "regular", 26, SOFT, "rm")
    f.box(lx, 632, 1600 - 2 * lx, 112, fill=CARD2, outline=GREEN, width=3)
    f.text(800, 672, "Your git repository", "bold", 34, TEXT, "mm")
    f.text(800, 714, "You review the branches and merge them", "regular", 27, SOFT, "mm")
    f.save("overview.png")


def room_steps():
    f = Fig(1600, 640)
    f.text(800, 66, "What happens in a room", "bold", 46, TEXT, "mm")
    steps = [
        ("1", "Brief", ["Who knows", "what?"], CYAN),
        ("2", "Propose", ["A plan and", "who does what"], CYAN),
        ("3", "Critique", ["One agent looks", "for the weak spot"], AMBER),
        ("4", "Agree", ["Or ask you", "to decide"], GREEN),
        ("5", "Build", ["At the same time,", "one branch each"], PINK),
    ]
    w, gap, x0, y, h = 268, 41, 42, 230, 270
    for i, (num, name, lines, colour) in enumerate(steps):
        x = x0 + i * (w + gap)
        f.box(x, y, w, h, outline=colour, width=3)
        f.d.ellipse([(x + w / 2 - 30) * SCALE, (y + 26) * SCALE, (x + w / 2 + 30) * SCALE, (y + 86) * SCALE], fill=colour)
        f.text(x + w / 2, y + 56, num, "bold", 34, BG, "mm")
        f.text(x + w / 2, y + 126, name, "bold", 36, TEXT, "mm")
        for j, line in enumerate(lines):
            f.text(x + w / 2, y + 180 + j * 38, line, "regular", 27, SOFT, "mm")
        if i < 4:
            ay = y + h / 2
            f.line([(x + w + 5, ay), (x + w + gap - 14, ay)], SOFT, 4)
            f.head(x + w + gap - 4, ay, "r", SOFT, 11)
    x2, x3 = x0 + 1 * (w + gap) + w / 2, x0 + 2 * (w + gap) + w / 2
    f.line([(x3, y - 6), (x3, 158), (x2, 158), (x2, y - 22)], AMBER, 4)
    f.head(x2, y - 6, "d", AMBER, 13)
    f.text((x2 + x3) / 2, 128, "revise the plan", "bold", 27, AMBER, "mm")
    f.text(800, 562, "Every room has a budget. If the agents cannot agree, it stops and tells you where they differ.", "regular", 28, SOFT, "mm")
    f.save("room-steps.png")


def human_gates():
    f = Fig(1600, 560)
    f.text(800, 66, "You decide how much runs without you", "bold", 46, TEXT, "mm")
    y, h = 170, 220
    nodes = [(50, 320, "Agents", "discuss", "plan and split", CYAN, False), (420, 340, "Opener approves", "optional", "discuss --approve", AMBER, True), (810, 340, "Receiver approves", "optional", "trust --work ask", AMBER, True), (1200, 350, "Agents build", "on separate branches", "nothing is merged", PINK, False)]
    for i, (x, w, title, sub, code, colour, gate) in enumerate(nodes):
        f.box(x, y, w, h, fill=CARD2 if gate else CARD, outline=colour, width=4 if gate else 3)
        f.text(x + w / 2, y + 58, title, "bold", 33, colour, "mm")
        f.text(x + w / 2, y + 112, sub, "regular", 27, SOFT, "mm")
        f.text(x + w / 2, y + 166, code, "mono" if gate else "regular", 25 if gate else 26, TEXT if gate else SOFT, "mm")
        if i < 3:
            nx = nodes[i + 1][0]
            f.line([(x + w + 5, y + h / 2), (nx - 14, y + h / 2)], SOFT, 4)
            f.head(nx - 3, y + h / 2, "r", SOFT, 11)
    f.text(800, 454, "Without the two gates, a room that agrees starts working right away.", "regular", 29, TEXT, "mm")
    f.text(800, 502, "agora-mesh does not merge anything. That stays with you.", "bold", 29, GREEN, "mm")
    f.save("human-gates.png")


if __name__ == "__main__":
    overview()
    room_steps()
    human_gates()
