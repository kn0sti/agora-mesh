#!/usr/bin/env python3
"""Draws the figures of the README. Run: python3 docs/img/make_figures.py

Needs matplotlib, and Pillow for the animation. Every number comes from
docs/measurements; nothing here is measured anew. The first figure is a model
and says so in the image.
"""
import io
import os

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.lines import Line2D
from matplotlib.patches import FancyArrowPatch, FancyBboxPatch, Rectangle

HERE = os.path.dirname(os.path.abspath(__file__))

# Colours: the validated default palette, one set per mode.
THEMES = {
    "light": dict(
        page="#f9f9f7", surface="#fcfcfb", ink="#0b0b0b", ink2="#52514e", muted="#898781",
        grid="#e1e0d9", axis="#c3c2b7", border="#e1e0d9",
        s1="#2a78d6", s2="#eb6834", s3="#1baf7a", wash1="#e3effc", wash2="#fde9e0", wash3="#def4ec",
    ),
    "dark": dict(
        page="#0d0d0d", surface="#1a1a19", ink="#f0efec", ink2="#c3c2b7", muted="#898781",
        grid="#2c2c2a", axis="#383835", border="#383835",
        s1="#3987e5", s2="#d95926", s3="#199e70", wash1="#1b2b40", wash2="#3a2218", wash3="#16312a",
    ),
}

plt.rcParams.update({
    "font.family": ["Helvetica Neue", "Helvetica", "Arial", "DejaVu Sans"],
    "font.size": 11,
    "svg.fonttype": "path",
    # The same input gives the same file, so that a rerun changes nothing in the repository.
    "svg.hashsalt": "agora-mesh",
    "axes.spines.top": False,
    "axes.spines.right": False,
})

# From docs/measurements/2026-11-split-work.md: what a room spends before and
# after the work itself (49 s of briefing and discussion, 8 s for the summary).
ROOM_OVERHEAD_MIN = 57 / 60
MEASURED_WORK_MIN = 73 / 60      # the two packages, one after the other
MEASURED_ROOM_MIN = 108 / 60     # what the room took in total


def save(fig, name, mode, t):
    for ext in ("svg", "png"):
        metadata = {"Date": None} if ext == "svg" else None
        fig.savefig(os.path.join(HERE, f"{name}-{mode}.{ext}"), dpi=200, facecolor=t["surface"], metadata=metadata)
    plt.close(fig)


def style_axis(ax, t):
    ax.set_facecolor(t["surface"])
    ax.grid(True, color=t["grid"], linewidth=1, linestyle="-")
    ax.set_axisbelow(True)
    for side in ("left", "bottom"):
        ax.spines[side].set_color(t["axis"])
        ax.spines[side].set_linewidth(1)
    ax.tick_params(colors=t["muted"], length=0, labelsize=10)


def time_model(mode):
    """How long a task takes: one agent, two peers, three peers."""
    t = THEMES[mode]
    fig, axes = plt.subplots(1, 2, figsize=(11.5, 5.9), sharey=True)
    fig.patch.set_facecolor(t["surface"])
    # Room to the right of each panel for the labels at the line ends.
    fig.subplots_adjust(left=0.065, right=0.85, top=0.70, bottom=0.25, wspace=0.5)

    peers = [(1, "One agent", t["s1"]), (2, "Two peers", t["s2"]), (3, "Three peers", t["s3"])]
    panels = [
        (1.0, "All of the work can be split"),
        (0.7, "70 % can be split, 30 % stays with one agent"),
    ]
    top = 60
    for ax, (split, title) in zip(axes, panels):
        style_axis(ax, t)
        ends = []
        for n, name, color in peers:
            xs = [0, top]
            if n == 1:
                ys = [0, top]
            else:
                ys = [ROOM_OVERHEAD_MIN + x * (1 - split) + x * split / n for x in xs]
            ax.plot(xs, ys, color=color, linewidth=2, solid_capstyle="round", zorder=3)
            ax.plot([xs[1]], [ys[1]], "o", color=color, markersize=9, markeredgecolor=t["surface"], markeredgewidth=2, zorder=4, clip_on=False)
            ends.append((ys[1], name))
        for y, name in ends:
            ax.annotate(f"{name}, {y:.0f} min", (top, y), xytext=(11, 0), textcoords="offset points", ha="left", va="center",
                        color=t["ink"], fontsize=10, annotation_clip=False)
        ax.set_xlim(0, top)
        ax.set_ylim(0, top * 1.08)
        ax.set_xticks(range(0, top + 1, 10))
        ax.set_yticks(range(0, top + 1, 10))
        ax.set_title(title, loc="left", color=t["ink"], fontsize=12, fontweight="semibold", pad=12)
        ax.set_xlabel("Work in the task: minutes one agent needs", color=t["ink2"], fontsize=10.5, labelpad=8)
    axes[0].set_ylabel("Minutes until the task is done", color=t["ink2"], fontsize=10.5, labelpad=8)

    # The one measured run, where it sits in the model.
    ax = axes[0]
    ax.plot([MEASURED_WORK_MIN], [MEASURED_ROOM_MIN], "o", color=t["ink"], markersize=7, markeredgecolor=t["surface"], markeredgewidth=2, zorder=5)
    # The label sits in the free corner above the lines, away from all three.
    ax.annotate("Measured run: 73 s of work,\ntwo peers were done in 108 s", (MEASURED_WORK_MIN, MEASURED_ROOM_MIN), xytext=(3.2, 50),
                textcoords="data", ha="left", va="center", color=t["ink2"], fontsize=9.5, linespacing=1.4,
                arrowprops=dict(arrowstyle="-", color=t["muted"], linewidth=1, shrinkA=4, shrinkB=5, relpos=(0.02, 0)))

    fig.text(0.065, 0.945, "A room pays off once the work is large enough to split", color=t["ink"], fontsize=17, fontweight="semibold", va="top")
    fig.text(0.065, 0.878, "Time to finish a task with one agent, two peers or three. A model built on one measured run, not a measurement.",
             color=t["ink2"], fontsize=11, va="top")
    handles = [Line2D([0], [0], color=c, linewidth=2, marker="o", markersize=7, markeredgecolor=t["surface"], markeredgewidth=1.5, label=n) for _, n, c in peers]
    legend = fig.legend(handles=handles, loc="upper left", bbox_to_anchor=(0.058, 0.835), ncol=3, frameon=False, fontsize=10.5, handlelength=2.2, columnspacing=2.2)
    for text in legend.get_texts():
        text.set_color(t["ink"])
    fig.text(0.065, 0.118,
             "Assumptions: a room spends 57 s on briefing, discussion and summary (measured), then the peers work at the same time on equal packages.\n"
             "Not in the model: time to review and merge the branches, and peers that reach their usage limit. Below about two minutes of work, one agent is faster.",
             color=t["muted"], fontsize=9.5, va="top", linespacing=1.5)
    save(fig, "time-model", mode, t)


def session_cost(mode):
    """What one turn costs before anyone says anything."""
    t = THEMES[mode]
    rows = [
        ("Default headless session\n7 connectors, 188 tools", 195258),
        ("Session started by agora-mesh\nno connectors, built-in tools", 23994),
        ("The same without tools\nagora discuss --lean", 3743),
    ]
    fig, ax = plt.subplots(figsize=(11.5, 4.3))
    fig.patch.set_facecolor(t["surface"])
    fig.subplots_adjust(left=0.27, right=0.93, top=0.70, bottom=0.14)
    ax.set_facecolor(t["surface"])
    top = 200000
    height = 0.34
    for i, (name, value) in enumerate(rows):
        y = len(rows) - 1 - i
        # Rounded at the value end, square at the baseline.
        round_px = min(value, 2600)
        ax.add_patch(FancyBboxPatch((0, y - height / 2), value, height, boxstyle=f"round,pad=0,rounding_size={round_px}",
                                    mutation_aspect=height / (round_px * 2.0) if round_px else 1, linewidth=0, facecolor=t["s1"], zorder=3))
        ax.add_patch(Rectangle((0, y - height / 2), value / 2, height, linewidth=0, facecolor=t["s1"], zorder=3))
        ax.text(value + top * 0.012, y, f"{value:,}", va="center", ha="left", color=t["ink"], fontsize=11.5, fontweight="semibold")
        ax.text(-top * 0.02, y, name, va="center", ha="right", color=t["ink"], fontsize=10.5, linespacing=1.45)
    ax.set_xlim(0, top)
    ax.set_ylim(-0.6, len(rows) - 0.4)
    ax.set_yticks([])
    ax.set_xticks(range(0, top + 1, 50000))
    ax.set_xticklabels([f"{v // 1000}K" if v else "0" for v in range(0, top + 1, 50000)])
    ax.grid(True, axis="x", color=t["grid"], linewidth=1)
    ax.set_axisbelow(True)
    ax.spines["left"].set_color(t["axis"])
    ax.spines["bottom"].set_visible(False)
    ax.tick_params(colors=t["muted"], length=0, labelsize=10)
    fig.text(0.03, 0.93, "What a session costs before anyone says anything", color=t["ink"], fontsize=17, fontweight="semibold", va="top")
    fig.text(0.03, 0.835, "Input tokens of one turn that says one word. Headless Claude Code sessions on the test machine; agora bench measures yours.",
             color=t["ink2"], fontsize=11, va="top")
    save(fig, "session-cost", mode, t)


def shared_memory(mode):
    """Peers, their shared memory, and an Obsidian vault."""
    t = THEMES[mode]
    fig, ax = plt.subplots(figsize=(11.5, 6.4))
    fig.patch.set_facecolor(t["surface"])
    fig.subplots_adjust(left=0.02, right=0.98, top=0.84, bottom=0.03)
    ax.set_facecolor(t["surface"])
    ax.set_xlim(0, 100)
    ax.set_ylim(0, 60)
    ax.axis("off")

    def box(x, y, w, h, face, edge=None, radius=1.4):
        ax.add_patch(FancyBboxPatch((x, y), w, h, boxstyle=f"round,pad=0,rounding_size={radius}", linewidth=1,
                                    edgecolor=edge or t["border"], facecolor=face, zorder=2))

    def label(x, y, text, size=10.5, weight="normal", color=None, ha="center", va="center"):
        ax.text(x, y, text, ha=ha, va=va, fontsize=size, fontweight=weight, color=color or t["ink"], zorder=5, linespacing=1.4)

    def arrow(a, b, both=True):
        ax.add_patch(FancyArrowPatch(a, b, arrowstyle="<|-|>" if both else "-|>", mutation_scale=13, linewidth=1.6,
                                     color=t["ink2"], shrinkA=0, shrinkB=0, zorder=4))

    machines = [(3, "Mac"), (36.5, "Home server"), (70, "Laptop")]
    w = 27
    for x, name in machines:
        box(x, 21, w, 37, t["page"])
        label(x + 1.6, 55.3, name, size=12.5, weight="semibold", ha="left")
        label(x + w - 1.6, 55.3, "own account, own files", size=9, color=t["muted"], ha="right")
        box(x + 2, 44.5, w - 4, 7.6, t["wash3"], edge=t["s3"])
        label(x + w / 2, 48.3, "Claude Code sessions\nin rooms", size=10)
        box(x + 2, 34.2, w - 4, 6, t["surface"])
        label(x + w / 2, 37.2, "agora daemon", size=10)
        box(x + 2, 23, w - 4, 7.6, t["wash1"], edge=t["s1"])
        label(x + w / 2, 26.8, "Shared memory\nMarkdown files, one per entry", size=10)
        arrow((x + w / 2, 30.9), (x + w / 2, 33.9), both=True)
        arrow((x + w / 2, 40.5), (x + w / 2, 44.2), both=True)

    # Replication between the daemons.
    for left, right in ((3 + w, 36.5), (36.5 + w, 70)):
        arrow((left + 0.4, 37.2), (right - 0.4, 37.2))
    label(33.25, 40.6, "signed\nentries", size=9, color=t["ink2"])
    label(66.75, 40.6, "about\na second", size=9, color=t["ink2"])

    # The Obsidian vault of one machine.
    box(3, 3, w, 12.5, t["wash2"], edge=t["s2"])
    label(3 + w / 2, 11.4, "Obsidian vault", size=12, weight="semibold")
    label(3 + w / 2, 6.6, "a folder of notes you read\nand write as usual", size=9.5, color=t["ink2"])
    arrow((3 + w / 2, 15.9), (3 + w / 2, 22.6))
    label(3 + w / 2 + 1.6, 19.2, "both ways", size=9, color=t["ink2"], ha="left")

    notes = [
        ("An agent or a human saves a fact once.", "A second later it is a file on every machine."),
        ("A note written in the vault", "becomes an entry on all peers; entries appear in the vault."),
        ("Marked as important,", "sessions that are running are told its title on their next turn."),
    ]
    y = 14.2
    for head, rest in notes:
        ax.plot([36.5], [y], "o", color=t["ink2"], markersize=4, zorder=5)
        label(38.5, y, head + " " + rest, size=10, ha="left")
        y -= 4.4

    fig.text(0.03, 0.955, "One memory for every peer, and an Obsidian vault as its window", color=t["ink"], fontsize=17, fontweight="semibold", va="top")
    fig.text(0.03, 0.895, "Each machine keeps the shared memory as plain files. The daemons replicate signed entries directly between the machines.",
             color=t["ink2"], fontsize=11, va="top")
    save(fig, "shared-memory", mode, t)


def own_accounts(mode):
    """Every machine keeps its own login and its own limit."""
    t = THEMES[mode]
    fig, ax = plt.subplots(figsize=(11.5, 5.6))
    fig.patch.set_facecolor(t["surface"])
    fig.subplots_adjust(left=0.02, right=0.98, top=0.82, bottom=0.03)
    ax.set_facecolor(t["surface"])
    ax.set_xlim(0, 100)
    ax.set_ylim(0, 50)
    ax.axis("off")

    def box(x, y, w, h, face, edge=None, radius=1.4):
        ax.add_patch(FancyBboxPatch((x, y), w, h, boxstyle=f"round,pad=0,rounding_size={radius}", linewidth=1,
                                    edgecolor=edge or t["border"], facecolor=face, zorder=2))

    def label(x, y, text, size=10.5, weight="normal", color=None, ha="center", va="center"):
        ax.text(x, y, text, ha=ha, va=va, fontsize=size, fontweight=weight, color=color or t["ink"], zorder=5, linespacing=1.4)

    def arrow(a, b):
        ax.add_patch(FancyArrowPatch(a, b, arrowstyle="-|>", mutation_scale=13, linewidth=1.6, color=t["ink2"], shrinkA=0, shrinkB=0, zorder=4))

    # Three peers. The usage figures are an example, not a measurement.
    peers = [("Laptop", "own login", 0.35), ("Desktop", "own login", 0.60), ("Home server", "own login", 0.15)]
    w, h = 29, 11
    ys = [35.5, 21, 6.5]
    for (name, plan, used), y in zip(peers, ys):
        box(3, y, w, h, t["page"])
        label(4.8, y + h - 3.1, name, size=12, weight="semibold", ha="left")
        label(3 + w - 1.8, y + h - 3.1, plan, size=10, color=t["ink2"], ha="right")
        # A meter: the track is a lighter step of the fill's own hue.
        mx, mw, mh = 4.8, w - 12.5, 1.5
        ax.add_patch(FancyBboxPatch((mx, y + 2.4), mw, mh, boxstyle="round,pad=0,rounding_size=0.75", linewidth=0, facecolor=t["wash1"], zorder=3))
        ax.add_patch(FancyBboxPatch((mx, y + 2.4), mw * used, mh, boxstyle="round,pad=0,rounding_size=0.75", linewidth=0, facecolor=t["s1"], zorder=4))
        label(mx + mw + 1.2, y + 3.15, f"{used:.0%} used", size=9.5, color=t["ink2"], ha="left")
        arrow((3 + w + 0.6, y + h / 2), (41.4, 26.5 + (y + h / 2 - 26.5) * 0.28))

    box(42, 14.5, 25, 24, t["wash3"], edge=t["s3"])
    label(54.5, 34.4, "One room", size=13, weight="semibold")
    label(54.5, 25, "brief each other\ndiscuss and criticise\nagree on a plan\nsplit the work", size=10.5, color=t["ink"])
    label(54.5, 16.6, "one session per machine", size=9, color=t["ink2"])

    arrow((67.6, 26.5), (73.4, 26.5))
    box(74, 14.5, 23, 24, t["page"])
    label(85.5, 34.4, "For you", size=13, weight="semibold")
    label(85.5, 25, "a plan the agents\nagree on, or\none branch per peer\nwith its part done", size=10.5)
    label(85.5, 16.6, "and what is left to decide", size=9, color=t["ink2"])

    label(42, 9.2, "No shared login. The mesh starts the agent that is installed on each machine.", size=10, ha="left")
    label(42, 5.4, "A peer at its limit hands its package to one that has capacity left.", size=10, ha="left")

    fig.text(0.03, 0.955, "Every machine keeps its own login and its own limit", color=t["ink"], fontsize=17, fontweight="semibold", va="top")
    fig.text(0.03, 0.885, "The mesh never sees a login or a key. Usage shown is an example; agora peers shows the real figures.",
             color=t["ink2"], fontsize=11, va="top")
    save(fig, "own-accounts", mode, t)


def bar(ax, y, value, height, colour, round_px):
    """A horizontal bar: rounded at the value end, square at the baseline."""
    round_px = min(value, round_px)
    ax.add_patch(FancyBboxPatch((0, y - height / 2), value, height, boxstyle=f"round,pad=0,rounding_size={round_px}",
                                mutation_aspect=height / (round_px * 2.0) if round_px else 1, linewidth=0, facecolor=colour, zorder=3))
    ax.add_patch(Rectangle((0, y - height / 2), value / 2, height, linewidth=0, facecolor=colour, zorder=3))


def bar_axis(ax, t, top, step, unit):
    ax.set_xlim(0, top)
    ax.set_yticks([])
    ax.set_xticks(range(0, top + 1, step))
    ax.set_xticklabels([f"{v // 1000}{unit}" if v else "0" for v in range(0, top + 1, step)])
    ax.grid(True, axis="x", color=t["grid"], linewidth=1)
    ax.set_axisbelow(True)
    ax.spines["left"].set_color(t["axis"])
    ax.spines["bottom"].set_visible(False)
    ax.tick_params(colors=t["muted"], length=0, labelsize=10)


def harness_cost(mode):
    """What each kind of session reads in one short turn."""
    t = THEMES[mode]
    # docs/measurements/2026-11-claude-code-on-another-service.md and 2026-11-real-agents.md.
    # The first row is the middle of four models; its label gives the range.
    rows = [
        ("Claude Code, bare, on another model\nno tools; four models", 462, "344 to 736"),
        ("Claude Code on Claude, no tools\nagora discuss --lean", 3743, "3,743"),
        ("OpenCode\non deepseek-v4.1-flash", 11543, "11,543"),
        ("Qwen Code\non qwen3.8-flash", 15074, "15,074"),
        ("Codex\non gpt-6-luna", 17845, "17,845"),
        ("Claude Code on Claude\nwith its built-in tools", 23994, "23,994"),
    ]
    fig, ax = plt.subplots(figsize=(11.5, 6.2))
    fig.patch.set_facecolor(t["surface"])
    fig.subplots_adjust(left=0.27, right=0.93, top=0.79, bottom=0.10)
    ax.set_facecolor(t["surface"])
    top, height = 25000, 0.42
    for i, (name, value, shown) in enumerate(rows):
        y = len(rows) - 1 - i
        bar(ax, y, value, height, t["s1"], 330)
        ax.text(value + top * 0.012, y, shown, va="center", ha="left", color=t["ink"], fontsize=11.5, fontweight="semibold")
        ax.text(-top * 0.02, y, name, va="center", ha="right", color=t["ink"], fontsize=10.5, linespacing=1.45)
    ax.set_ylim(-0.6, len(rows) - 0.4)
    bar_axis(ax, t, top, 5000, "K")
    fig.text(0.03, 0.95, "What each kind of session reads before anyone says anything", color=t["ink"], fontsize=17, fontweight="semibold", va="top")
    fig.text(0.03, 0.885, "Input tokens of one turn that says one word, measured on one machine. The agents were started as they come.",
             color=t["ink2"], fontsize=11, va="top")
    save(fig, "harness-cost", mode, t)


def what_leaves(mode):
    """What Claude Code sends to another model service, started the usual way and bare."""
    t = THEMES[mode]
    groups = [("Session without tools", 17259, 2189), ("Session with tools", 57118, 5539)]
    fig, ax = plt.subplots(figsize=(11.5, 5.4))
    fig.patch.set_facecolor(t["surface"])
    fig.subplots_adjust(left=0.19, right=0.60, top=0.70, bottom=0.12)
    ax.set_facecolor(t["surface"])
    top, height = 60000, 0.30
    for i, (name, usual, bare) in enumerate(groups):
        y = (len(groups) - 1 - i) * 1.0
        bar(ax, y + 0.17, usual, height, t["s2"], 800)
        bar(ax, y - 0.17, bare, height, t["s1"], 800)
        ax.text(usual + top * 0.015, y + 0.17, f"{usual:,}", va="center", ha="left", color=t["ink"], fontsize=11, fontweight="semibold")
        ax.text(bare + top * 0.015, y - 0.17, f"{bare:,}", va="center", ha="left", color=t["ink"], fontsize=11, fontweight="semibold")
        ax.text(-top * 0.025, y, name, va="center", ha="right", color=t["ink"], fontsize=10.5)
    ax.set_ylim(-0.55, len(groups) - 0.45)
    bar_axis(ax, t, top, 20000, "K")
    handles = [Line2D([], [], marker="s", linestyle="", markersize=9, color=t["s2"], label="started the usual way"),
               Line2D([], [], marker="s", linestyle="", markersize=9, color=t["s1"], label="started bare, as agora-mesh does")]
    legend = fig.legend(handles=handles, loc="upper left", bbox_to_anchor=(0.025, 0.80), ncol=2, frameon=False, fontsize=10.5, handletextpad=0.3, columnspacing=1.6)
    for text in legend.get_texts():
        text.set_color(t["ink2"])
    # What the usual start carried with every request, and the bare one does not.
    fig.text(0.65, 0.685, "Sent along the usual way,\nnot sent bare", color=t["ink"], fontsize=11.5, fontweight="semibold", va="top", linespacing=1.4)
    items = ["a CLAUDE.md from above the project", "the path of the home directory", "some 85 permission rules", "the state of the repository", "nine of twelve tools"]
    for i, item in enumerate(items):
        fig.text(0.65, 0.535 - i * 0.075, "–  " + item, color=t["ink2"], fontsize=10.5, va="top")
    fig.text(0.03, 0.945, "What Claude Code sends to another model service", color=t["ink"], fontsize=17, fontweight="semibold", va="top")
    fig.text(0.03, 0.87, "Bytes of one request, measured with the real Claude Code against a local stand-in for a service.",
             color=t["ink2"], fontsize=11, va="top")
    save(fig, "what-leaves", mode, t)


# Real rooms, message by message. Each entry is one run from docs/measurements:
# the two machines, what each message was, and what the room ended with.
ROOMS = {
    # 2026-11-real-agents.md, "A work phase of two harnesses".
    "room-flow": dict(
        left="Claude Code, bare, on DeepSeek", right="OpenCode on DeepSeek",
        messages=[
            ("Mac", "BRIEF", "the goal, and what is known"),
            ("Server", "KNOWN_MISSING", "what it knows, what it lacks"),
            ("Mac", "ANSWER", "fills the gaps"),
            ("Server", "PROPOSAL", "a first plan"),
            ("Mac", "SPLIT_WORK", "one package per peer"),
            ("Server", "CONSENSUS", "agreed"),
            ("room", "WORK", "both work at the same time"),
            ("Mac", "DONE", "docs/USAGE.md"),
            ("Server", "DONE", "bin/greet.sh"),
        ],
        summary="main untouched  ·  9 messages  ·  422 s",
    ),
    # 2026-11-real-agents.md, "A work phase with Codex".
    "room-flow-codex": dict(
        left="Claude Code, bare, on DeepSeek", right="Codex on gpt-6-luna",
        messages=[
            ("Mac", "BRIEF", "the goal, and what is known"),
            ("Server", "KNOWN_MISSING", "what it knows, what it lacks"),
            ("Mac", "ANSWER", "fills the gaps"),
            ("Server", "PASS", "nothing to add yet"),
            ("Mac", "SPLIT_WORK", "one package per peer"),
            ("Server", "CONSENSUS", "agreed"),
            ("room", "WORK", "both work at the same time"),
            ("Mac", "DONE", "docs/USAGE.md"),
            ("Server", "DONE", "bin/greet.sh"),
        ],
        summary="main untouched  ·  9 messages  ·  190 s",
    ),
    # 2026-11-claude-code-on-another-service.md, part 3, run 4.
    "room-flow-deepseek": dict(
        left="Claude Code, bare, on DeepSeek", right="Claude Code, bare, on DeepSeek",
        messages=[
            ("Mac", "BRIEF", "the goal, and what is known"),
            ("Server", "KNOWN_MISSING", "what it knows, what it lacks"),
            ("Mac", "ANSWER", "fills the gaps"),
            ("Server", "SPLIT_WORK", "one package per peer"),
            ("Mac", "CONSENSUS", "agreed"),
            ("room", "WORK", "both work at the same time"),
            ("Mac", "DONE", "bin/greet.sh"),
            ("Server", "DONE", "docs/USAGE.md"),
        ],
        summary="main untouched  ·  8 messages  ·  351 s",
    ),
}


def room_frame(room, shown, t):
    """One picture of a room after `shown` messages; one more shows the result too."""
    messages = room["messages"]
    fig, ax = plt.subplots(figsize=(11, 6.6), dpi=100)
    fig.patch.set_facecolor(t["surface"])
    fig.subplots_adjust(left=0.02, right=0.98, top=0.88, bottom=0.02)
    ax.set_facecolor(t["surface"])
    ax.set_xlim(0, 100)
    ax.set_ylim(0, 60)
    ax.axis("off")

    def box(x, y, w, h, face, edge, radius=1.3, width=1.0):
        ax.add_patch(FancyBboxPatch((x, y), w, h, boxstyle=f"round,pad=0,rounding_size={radius}", linewidth=width, edgecolor=edge, facecolor=face, zorder=2))

    def label(x, y, text, size=10.5, weight="normal", colour=None, ha="center"):
        ax.text(x, y, text, ha=ha, va="center", fontsize=size, fontweight=weight, color=colour or t["ink"], zorder=5)

    # The two machines.
    for x, name, runs, colour in ((2, "Mac", room["left"], t["s1"]), (70, "Server", room["right"], t["s2"])):
        box(x, 49, 28, 9, t["page"], t["border"])
        ax.add_patch(Rectangle((x, 49), 0.9, 9, linewidth=0, facecolor=colour, zorder=3))
        label(x + 3, 55.2, name, size=13, weight="semibold", ha="left")
        label(x + 3, 51.6, runs, size=9.5, colour=t["ink2"], ha="left")
    label(50, 55.2, "one room", size=11, colour=t["ink2"])
    label(50, 51.6, "no master, no shared login", size=9.5, colour=t["muted"])

    # The messages, newest at the bottom.
    row = 4.55
    for i, (who, kind, what) in enumerate(messages[:shown]):
        y = 44.2 - i * row
        if who == "Mac":
            x, face, edge = 4, t["wash1"], t["s1"]
        elif who == "Server":
            x, face, edge = 50, t["wash2"], t["s2"]
        else:
            x, face, edge = 27, t["wash3"], t["s3"]
        newest = i == shown - 1 and shown <= len(messages)
        box(x, y - 1.75, 46, 3.5, face, edge, radius=1.0, width=2.0 if newest else 0.8)
        label(x + 1.6, y, f"#{i + 1}", size=9.5, colour=t["ink2"], ha="left")
        label(x + 6.2, y, kind, size=10, weight="semibold", ha="left")
        label(x + 24.5, y, what, size=9.5, colour=t["ink2"], ha="left")

    # The result: the branches the peers pushed.
    if shown > len(messages):
        y = 44.2 - len(messages) * row - 0.4
        box(4, y - 2.3, 92, 4.6, t["page"], t["border"])
        label(6, y, "pushed:", size=10, colour=t["ink2"], ha="left")
        label(15, y, "agora/<room>/mac", size=10, weight="semibold", colour=t["ink"], ha="left")
        label(40, y, "agora/<room>/server", size=10, weight="semibold", colour=t["ink"], ha="left")
        label(68, y, room["summary"], size=10, colour=t["ink2"], ha="left")

    kinds = [kind for _, kind, _ in messages]
    work = kinds.index("WORK") + 1
    split = min(i + 1 for i, kind in enumerate(kinds) if kind not in ("BRIEF", "KNOWN_MISSING", "ANSWER"))
    phases = [(0, "A room is opened"), (1, "Briefing"), (split, "Discussion"), (work, "Work phase"), (len(messages) + 1, "Result: one branch per peer")]
    phase = [name for start, name in phases if shown >= start][-1]
    fig.text(0.03, 0.965, "A room, message by message", color=t["ink"], fontsize=16, fontweight="semibold", va="top")
    fig.text(0.97, 0.958, phase, color=t["ink2"], fontsize=12, va="top", ha="right")
    return fig


def room_flows():
    """An animation per room, and the last picture of each as a still."""
    from PIL import Image

    t = THEMES["light"]
    for name, room in ROOMS.items():
        count = len(room["messages"])
        frames = []
        for shown in range(0, count + 2):
            fig = room_frame(room, shown, t)
            buffer = io.BytesIO()
            fig.savefig(buffer, format="png", dpi=100, facecolor=t["surface"])
            if shown == count + 1:
                fig.savefig(os.path.join(HERE, f"{name}-light.png"), dpi=200, facecolor=t["surface"])
            plt.close(fig)
            buffer.seek(0)
            frames.append(Image.open(buffer).convert("P", palette=Image.ADAPTIVE, colors=64))
        durations = [1200] + [1000] * count + [5000]
        frames[0].save(os.path.join(HERE, f"{name}.gif"), save_all=True, append_images=frames[1:], duration=durations, loop=0, optimize=True)


if __name__ == "__main__":
    for mode in THEMES:
        time_model(mode)
        session_cost(mode)
        shared_memory(mode)
        own_accounts(mode)
        harness_cost(mode)
        what_leaves(mode)
    room_flows()
    print("wrote", sorted(f for f in os.listdir(HERE) if f.endswith((".svg", ".png", ".gif"))))
