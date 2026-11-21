#!/usr/bin/env python3
"""Draws the token cost figures from docs/measurements/data/2026-11-token-costs.json.

Run: python3 docs/img/make_cost_figures.py   (needs Pillow, fonts as in make_overview_figures.py)
Every number comes from that file. Nothing is estimated here.
"""
import json
import os
import statistics as st

from PIL import Image, ImageDraw

from make_overview_figures import BG, CARD2, DIM, LINE, OUT, SCALE, SOFT, TEXT, Fig, font

DATA = os.path.join(OUT, "..", "measurements", "data", "2026-11-token-costs.json")
BLUE, ORANGE, GREY = "#3987e5", "#d95926", "#5b6672"
RUNS = json.load(open(DATA))["runs"]


def of(exp):
    return [r for r in RUNS if r["exp"] == exp]


def k(n):
    return f"{n / 1000:.1f}k"


def ladder():
    f = Fig(1600, 840)
    f.text(70, 58, "What one action costs", "bold", 46, TEXT, "lm")
    f.text(70, 112, "Tokens, all agents together. A room re-reads the conversation on every turn.", "regular", 28, SOFT, "lm")
    rows = [("ask", "agora ask", "one short question"), ("review", "agora review", "a small diff"), ("solo", "One agent alone", "writes the plan"),
            ("lean2", "Room of 2", "agree on the same plan"), ("lean3", "Room of 3", "agree on the same plan")]
    x0, x1, top, step, h = 460, 1330, 180, 104, 42
    top_value = 100000
    sx = lambda v: x0 + (x1 - x0) * v / top_value
    for v in range(0, 100001, 20000):
        f.line([(sx(v), top - 16), (sx(v), top + step * 5 - 22)], "#232b33", 2)
        f.text(sx(v), top + step * 5 + 2, "0" if v == 0 else f"{v // 1000}k", "regular", 24, DIM, "mm")
    solo = st.median(r["tokens"] for r in of("solo"))
    for i, (exp, name, sub) in enumerate(rows):
        t = [r["tokens"] for r in of(exp)]
        med, y = st.median(t), top + i * step + 30
        f.text(70, y - 17, name, "bold", 31, TEXT, "lm")
        f.text(70, y + 21, sub, "regular", 24, DIM, "lm")
        room = exp.startswith("lean")
        f.d.rounded_rectangle([x0 * SCALE, (y - h / 2) * SCALE, max(sx(med), x0 + 8) * SCALE, (y + h / 2) * SCALE], radius=4 * SCALE, fill=BLUE if room else GREY)
        lo, hi = sx(min(t)), sx(max(t))
        f.line([(lo, y), (hi, y)], TEXT, 2)
        for x in (lo, hi):
            f.line([(x, y - 10), (x, y + 10)], TEXT, 2)
        label = k(med) + (f"  ·  {med / solo:.0f}× one agent" if room else "")
        f.text(max(hi, sx(med)) + 18, y, label, "bold", 27, TEXT, "lm")
    f.text(70, 790, "Bar: median of 6 runs (3 for ask and review). Thin line: lowest to highest. DeepSeek v4.1 flash; rooms and single agent without tools.", "regular", 24, DIM, "lm")
    f.save("token-costs.png")


def by_message(run):
    """The tokens of each model turn, in the order the room spoke."""
    left = {p: [t for s in ss for t in s] for p, ss in run["sessions"].items()}
    out = []
    for row in run["timeline"].strip().splitlines():
        _, _, who, kind = row.split("\t")
        if left.get(who):
            out.append((who, kind, *left[who].pop(0)))
    for who, rest in left.items():
        out += [(who, "RESULT", *t) for t in rest]
    return out


def growth():
    f = Fig(1600, 800)
    f.text(70, 58, "Why a room costs more: every turn re-reads the conversation", "bold", 42, TEXT, "lm")
    f.text(70, 112, "Input tokens of each turn, median of the rooms that reached consensus.", "regular", 28, SOFT, "lm")
    x0, x1, y0, y1 = 170, 1500, 660, 180
    top, last = 9000, 13
    sx = lambda i: x0 + (x1 - x0) * (i - 1) / (last - 1)
    sy = lambda v: y0 - (y0 - y1) * v / top
    for v in range(0, top + 1, 3000):
        f.line([(x0, sy(v)), (x1, sy(v))], "#232b33", 2)
        f.text(x0 - 18, sy(v), "0" if v == 0 else f"{v // 1000}k", "regular", 24, DIM, "rm")
    for i in range(1, last + 1):
        f.text(sx(i), y0 + 32, str(i), "regular", 24, DIM, "mm")
    f.text((x0 + x1) / 2, y0 + 74, "Turn in the room", "regular", 26, SOFT, "mm")
    for exp, colour, name in (("lean2", BLUE, "Room of 2"), ("lean3", ORANGE, "Room of 3")):
        rooms = [[m[2] for m in by_message(r)] for r in of(exp) if r.get("reason") == "consensus"]
        pts = []
        for i in range(last):
            vals = [r[i] for r in rooms if len(r) > i]
            if len(vals) >= 3:
                pts.append((i + 1, st.median(vals)))
        f.line([(sx(i), sy(v)) for i, v in pts], colour, 4)
        for i, v in pts:
            f.d.ellipse([(sx(i) - 7) * SCALE, (sy(v) - 7) * SCALE, (sx(i) + 7) * SCALE, (sy(v) + 7) * SCALE], fill=colour, outline=BG, width=2 * SCALE)
        i, v = pts[-1]
        f.text(sx(i) + 6, sy(v) - 34, f"{name}, turn {i}: {k(v)}", "bold", 26, TEXT, "rm")
    for i, (colour, name) in enumerate(((BLUE, "Room of 2"), (ORANGE, "Room of 3"))):
        x = x0 + 20 + i * 200
        f.line([(x, 206), (x + 36, 206)], colour, 4)
        f.d.ellipse([(x + 11) * SCALE, (206 - 7) * SCALE, (x + 25) * SCALE, (206 + 7) * SCALE], fill=colour)
        f.text(x + 48, 206, name, "regular", 25, SOFT, "lm")
    f.text(70, 768, "DeepSeek v4.1 flash, no tools.", "regular", 24, DIM, "lm")
    f.save("token-growth.png")


def room_gif():
    rooms = [r for r in of("lean2") if r.get("reason") == "consensus"]
    med = st.median(r["tokens"] for r in rooms)
    run = min(rooms, key=lambda r: abs(r["tokens"] - med))
    msgs = by_message(run)
    solo = st.median(r["tokens"] for r in of("solo"))
    w, h = 1200, 680
    names = {"A": ("Laptop", BLUE), "B": ("Desktop", ORANGE)}
    frames, times, total = [], [], 0
    fonts = {(kind, size): font(kind, size) for kind, size in (("bold", 34), ("bold", 25), ("regular", 23), ("regular", 21), ("bold", 66), ("regular", 25))}

    def draw(shown, total):
        img = Image.new("RGB", (w * SCALE, h * SCALE), BG)
        d = ImageDraw.Draw(img)
        t = lambda x, y, s, kind, size, fill=TEXT, anchor="la": d.text((x * SCALE, y * SCALE), s, font=fonts[(kind, size)], fill=fill, anchor=anchor)
        t(40, 44, "Watch a room spend tokens", "bold", 34, TEXT, "lm")
        t(40, 88, "A real room of two agents, turn by turn", "regular", 23, SOFT, "lm")
        y = 136
        for i, (who, kind, a, b) in enumerate(msgs):
            active = i < shown
            name, colour = names.get(who, (who, GREY))
            d.rounded_rectangle([40 * SCALE, (y + 7) * SCALE, 52 * SCALE, (y + 31) * SCALE], radius=3 * SCALE, fill=colour if active else "#232b33")
            t(66, y + 19, name, "regular", 21, TEXT if active else "#3a444e", "lm")
            t(166, y + 19, kind.replace("_", " ").title(), "regular", 21, TEXT if active else "#3a444e", "lm")
            t(620, y + 19, f"{a + b:,}" if active else "", "regular", 21, SOFT, "rm")
            y += 49
        t(690, 190, "Tokens so far", "regular", 25, SOFT, "lm")
        t(690, 262, f"{total:,}", "bold", 66, TEXT, "lm")
        t(690, 334, f"{total / solo:.1f}× one agent" if total else " ", "bold", 25, TEXT, "lm")
        bx, bw, by = 690, 460, 420
        top = max(run["tokens"], solo) * 1.04
        d.rounded_rectangle([bx * SCALE, by * SCALE, (bx + bw) * SCALE, (by + 34) * SCALE], radius=4 * SCALE, fill=CARD2)
        if total:
            d.rounded_rectangle([bx * SCALE, by * SCALE, (bx + max(8, bw * total / top)) * SCALE, (by + 34) * SCALE], radius=4 * SCALE, fill=BLUE)
        sx = bx + bw * solo / top
        d.line([(sx * SCALE, (by - 12) * SCALE), (sx * SCALE, (by + 46) * SCALE)], fill=TEXT, width=3 * SCALE)
        t(sx - 4, by + 72, f"one agent alone: {k(solo)}", "regular", 21, SOFT, "la")
        t(690, 610, "DeepSeek v4.1 flash, no tools", "regular", 21, DIM, "lm")
        return img.resize((w, h), Image.Resampling.LANCZOS)

    base = draw(0, 0)
    pal = Image.new("P", (1, 1))
    rgb = lambda c: [int(c[i : i + 2], 16) for i in (1, 3, 5)]
    values = []
    for c in (TEXT, SOFT, DIM, BLUE, ORANGE, "#3a444e", "#232b33", CARD2):
        for s in range(0, 31):
            values += [round(b + (x - b) * s / 30) for b, x in zip(rgb(BG), rgb(c))]
    pal.putpalette((values + [0] * 768)[:768])
    frames.append(base.quantize(palette=pal, dither=Image.Dither.NONE))
    times.append(1200)
    for i, (_, _, a, b) in enumerate(msgs):
        total += a + b
        frames.append(draw(i + 1, total).quantize(palette=pal, dither=Image.Dither.NONE))
        times.append(1000)
    times[-1] = 5000
    path = os.path.join(OUT, "token-room.gif")
    frames[0].save(path, save_all=True, append_images=frames[1:], duration=times, loop=0, optimize=True, disposal=1)
    frames[-1].convert("RGB").save(os.path.expanduser("~/bau/agora-assets/token-room-last.png"))
    print("room", run["run"], run["tokens"], len(msgs), os.path.getsize(path) // 1024, "KB")


if __name__ == "__main__":
    ladder()
    growth()
    room_gif()
