#!/usr/bin/env python3
"""Lanshu-style animated architecture diagram renderer for srt-whep.

Hand-built (the lanshu skill ships only SKILL.md). Produces:
  - a static PNG preview
  - an animated GIF (moving glow particles along the flow edges)
  - an .excalidraw source (editable next time)

Style: dark canvas, thin rounded outer frame, hand-drawn (wobbly) boxes with
coloured borders, one green title capsule, top-right signature, moving colour
only in the GIF overlay. Matches docs/srt-whep-coordinator-actor.gif at
2520x1725, 12.5 fps, 48 frames.

NOTE: Comic Sans MS has no arrow/chevron glyphs; use ASCII "->" and ">".
"""

import json
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont, ImageFilter

# Author the layout in a fixed base space (BW x BH) and supersample the raster
# by SCALE for a higher-resolution GIF. All build_base coords/fonts are in base
# space; ScaledDraw applies SCALE at draw time.
SCALE = 1.5
BW, BH = 2520, 1725
W, H = round(BW * SCALE), round(BH * SCALE)
FPS = 12.5
NFRAMES = 48

BG = (13, 17, 23)
FRAME = (58, 64, 72)
WHITE = (232, 236, 240)
DIM = (150, 162, 173)
GREEN = (61, 220, 132)
CYAN = (86, 212, 232)
PURPLE = (176, 120, 232)
RED = (255, 107, 94)
ORANGE = (240, 168, 80)
BLUE = (96, 160, 240)
TAGBG = (24, 30, 38)

FONT_DIR = "/System/Library/Fonts/Supplemental"
CS = os.path.join(FONT_DIR, "Comic Sans MS.ttf")
CSB = os.path.join(FONT_DIR, "Comic Sans MS Bold.ttf")


def font(size, bold=False):
    return ImageFont.truetype(CSB if bold else CS, max(1, round(size * SCALE)))


def _sc(v):
    """Scale a coordinate structure by SCALE. Handles a scalar, a flat coord/
    bbox tuple, or a list of point tuples. Colours are kwargs, never touched."""
    if isinstance(v, (int, float)):
        return v * SCALE
    if isinstance(v, (list, tuple)):
        if v and isinstance(v[0], (int, float)):
            return type(v)(x * SCALE for x in v)
        return [_sc(e) for e in v]
    return v


class ScaledDraw:
    """Wrap ImageDraw so build_base authors in base (BW x BH) coords while the
    raster is SCALE larger. Positional coords + width/radius/spacing kwargs are
    scaled; fill/outline colours pass through. Fonts are pre-scaled by font()."""

    def __init__(self, d):
        self._d = d

    def _kw(self, kw):
        if "width" in kw:
            kw["width"] = max(1, round(kw["width"] * SCALE))
        if "radius" in kw:
            kw["radius"] = kw["radius"] * SCALE
        if "spacing" in kw:
            kw["spacing"] = kw["spacing"] * SCALE
        return kw

    def line(self, xy, **kw):
        self._d.line(_sc(xy), **self._kw(kw))

    def polygon(self, xy, **kw):
        self._d.polygon(_sc(xy), **self._kw(kw))

    def ellipse(self, xy, **kw):
        self._d.ellipse(_sc(xy), **self._kw(kw))

    def rounded_rectangle(self, xy, **kw):
        self._d.rounded_rectangle(_sc(xy), **self._kw(kw))

    def multiline_text(self, xy, text, **kw):
        self._d.multiline_text(_sc(xy), text, **self._kw(kw))

    def textlength(self, text, **kw):
        return self._d.textlength(text, **kw) / SCALE


# ----- hand-drawn helpers -------------------------------------------------


def smooth_noise(t, seed):
    return (
        math.sin(t * 1.7 + seed * 2.3)
        + 0.5 * math.sin(t * 3.9 + seed * 5.1)
        + 0.3 * math.sin(t * 7.3 + seed * 1.1)
    ) / 1.8


def rounded_rect_pts(x0, y0, x1, y1, r, n_side=24, n_arc=8):
    pts = []

    def arc(cx, cy, a0, a1):
        for i in range(n_arc + 1):
            a = a0 + (a1 - a0) * i / n_arc
            pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))

    def edge(p, q):
        for i in range(1, n_side + 1):
            pts.append(
                (p[0] + (q[0] - p[0]) * i / n_side, p[1] + (q[1] - p[1]) * i / n_side)
            )

    pts.append((x0 + r, y0))
    edge((x0 + r, y0), (x1 - r, y0))
    arc(x1 - r, y0 + r, -math.pi / 2, 0)
    edge((x1, y0 + r), (x1, y1 - r))
    arc(x1 - r, y1 - r, 0, math.pi / 2)
    edge((x1 - r, y1), (x0 + r, y1))
    arc(x0 + r, y1 - r, math.pi / 2, math.pi)
    edge((x0, y1 - r), (x0, y0 + r))
    arc(x0 + r, y0 + r, math.pi, math.pi * 1.5)
    return pts


def wobble(pts, amp, seed):
    out = []
    total = 0.0
    for i in range(len(pts)):
        p = pts[i]
        q = pts[(i + 1) % len(pts)]
        dx, dy = q[0] - p[0], q[1] - p[1]
        L = math.hypot(dx, dy) or 1.0
        total += L
        nx, ny = -dy / L, dx / L
        off = amp * smooth_noise(total * 0.02, seed)
        out.append((p[0] + nx * off, p[1] + ny * off))
    return out


def draw_hand_rect(d, box, color, lw=5, r=34, amp=3.0, seed=1, fill=None):
    x0, y0, x1, y1 = box
    pts = rounded_rect_pts(x0, y0, x1, y1, r)
    if fill is not None:
        d.polygon(pts, fill=fill)
    w = wobble(pts, amp, seed)
    d.line(w + [w[0]], fill=color, width=lw, joint="curve")
    w2 = wobble(pts, amp * 1.6, seed + 7)
    d.line(w2 + [w2[0]], fill=color, width=max(1, lw - 3), joint="curve")


def poly_from(pts, amp, seed):
    out = []
    total = 0.0
    for i in range(len(pts)):
        p = pts[i]
        q = pts[i + 1] if i + 1 < len(pts) else pts[i - 1]
        dx, dy = q[0] - p[0], q[1] - p[1]
        L = math.hypot(dx, dy) or 1.0
        total += L
        nx, ny = -dy / L, dx / L
        off = amp * smooth_noise(total * 0.02, seed)
        out.append((p[0] + nx * off, p[1] + ny * off))
    return out


def bezier(p0, p1, p2, n=60):
    pts = []
    for i in range(n + 1):
        t = i / n
        x = (1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0]
        y = (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1]
        pts.append((x, y))
    return pts


def dashed_line(d, pts, color, lw, dash=26, gap=20):
    draw_on = True
    seg_left = dash
    cur = pts[0]
    i = 1
    while i < len(pts):
        nxt = pts[i]
        dx, dy = nxt[0] - cur[0], nxt[1] - cur[1]
        L = math.hypot(dx, dy)
        if L < 1e-6:
            cur = nxt
            i += 1
            continue
        ux, uy = dx / L, dy / L
        step = min(seg_left, L)
        p2 = (cur[0] + ux * step, cur[1] + uy * step)
        if draw_on:
            d.line([cur, p2], fill=color, width=lw)
        cur = p2
        seg_left -= step
        if seg_left <= 1e-6:
            draw_on = not draw_on
            seg_left = dash if draw_on else gap
        if math.hypot(nxt[0] - cur[0], nxt[1] - cur[1]) < 1e-6:
            cur = nxt
            i += 1


def arrowhead(d, tip, frm, color, size=26):
    ang = math.atan2(tip[1] - frm[1], tip[0] - frm[0])
    a1 = ang + math.radians(158)
    a2 = ang - math.radians(158)
    p1 = (tip[0] + size * math.cos(a1), tip[1] + size * math.sin(a1))
    p2 = (tip[0] + size * math.cos(a2), tip[1] + size * math.sin(a2))
    d.polygon([tip, p1, p2], fill=color)


def draw_edge(d, pts, color, lw=5, seed=3, dashed=False, arrow="end", amp=2.2):
    w = poly_from(pts, amp, seed)
    if dashed:
        dashed_line(d, w, color, lw)
    else:
        d.line(w, fill=color, width=lw, joint="curve")
    if arrow in ("end", "both"):
        arrowhead(d, w[-1], w[-2], color)
    if arrow in ("start", "both"):
        arrowhead(d, w[0], w[1], color)


def text(d, xy, s, f, fill, anchor="la", spacing=10):
    d.multiline_text(xy, s, font=f, fill=fill, anchor=anchor, spacing=spacing)


def capsule(d, box, fill, r=None):
    x0, y0, x1, y1 = box
    if r is None:
        r = (y1 - y0) / 2
    d.rounded_rectangle(box, radius=r, fill=fill)


# ----- diagram spec -------------------------------------------------------


def build_base():
    img = Image.new("RGB", (W, H), BG)
    d = ScaledDraw(ImageDraw.Draw(img))

    draw_hand_rect(d, (34, 34, BW - 34, BH - 34), FRAME, lw=4, r=46, amp=2.4, seed=99)

    # --- title + capsule + signature ---
    tfont = font(50, bold=True)
    title = "srt-whep  —  one SRT in, many WHEP out, run by a"
    text(d, (76, 70), title, tfont, WHITE)
    tw = d.textlength(title, font=tfont)
    cfont = font(40, bold=True)
    cap_label = "COORDINATOR ACTOR"
    cw = d.textlength(cap_label, font=cfont)
    cap_x0 = 76 + tw + 34
    cap = (cap_x0, 60, cap_x0 + cw + 70, 132)
    capsule(d, cap, GREEN)
    text(
        d,
        ((cap[0] + cap[2]) / 2, (cap[1] + cap[3]) / 2 - 2),
        cap_label,
        cfont,
        (10, 20, 14),
        anchor="mm",
    )

    sfont = font(32, bold=True)
    sig = "Kun Wu · Eyevinn"
    sw = d.textlength(sig, font=sfont)
    sig_box = (BW - 70 - sw - 56, 62, BW - 70, 128)
    d.rounded_rectangle(sig_box, radius=33, outline=DIM, width=3)
    text(
        d,
        ((sig_box[0] + sig_box[2]) / 2, (sig_box[1] + sig_box[3]) / 2 - 2),
        sig,
        sfont,
        DIM,
        anchor="mm",
    )

    text(
        d,
        (78, 162),
        "v2.0.0 signal plane  ·  one task owns all state  ·  messages in, "
        "replies out  ·  lifecycle is an explicit state machine",
        font(31),
        DIM,
    )

    lf = font(40, bold=True)
    mf = font(31)
    tgf = font(27)
    ll = font(29)

    def box_title(box, s, col):
        text(d, (box[0] + 34, box[1] + 24), s, lf, col)

    def body(box, lines, dy, fnt=mf, col=WHITE, sp=15, dx=34):
        text(d, (box[0] + dx, box[1] + dy), "\n".join(lines), fnt, col, spacing=sp)

    def tag(box, s, align="left"):
        w = d.textlength(s, font=tgf)
        if align == "right":
            pill = (box[2] - 30 - w - 44, box[3] - 62, box[2] - 30, box[3] - 20)
        else:
            pill = (box[0] + 30, box[3] - 62, box[0] + 30 + w + 44, box[3] - 20)
        d.rounded_rectangle(pill, radius=21, fill=TAGBG, outline=(70, 80, 90), width=2)
        text(
            d,
            ((pill[0] + pill[2]) / 2, (pill[1] + pill[3]) / 2 - 2),
            s,
            tgf,
            DIM,
            anchor="mm",
        )

    # --- boxes ---
    VIEW = (86, 296, 470, 494)
    draw_hand_rect(d, VIEW, WHITE, seed=11)
    box_title(VIEW, "WHEP VIEWERS", WHITE)
    body(VIEW, ["browsers · N sessions"], dy=94, col=DIM)

    ROUTES = (86, 586, 470, 968)
    draw_hand_rect(d, ROUTES, WHITE, seed=12)
    text(
        d,
        (ROUTES[0] + 34, ROUTES[1] + 26),
        "HTTP ROUTES (actix)",
        font(34, bold=True),
        WHITE,
    )
    body(
        ROUTES,
        [
            "POST   /channel",
            "PATCH  /channel/{id}",
            "DELETE /channel/{id}",
            "POST   /whip_sink/{id}",
        ],
        dy=96,
        sp=18,
    )
    tag(ROUTES, "thin adapters · no state")

    COORD = (840, 474, 1566, 958)
    draw_hand_rect(d, COORD, GREEN, seed=13, lw=6)
    box_title(COORD, "COORDINATOR · signaling actor", GREEN)
    body(
        COORD,
        [
            "select! {",
            "    cmd  = mailbox (mpsc)",
            "    fail = reap chan (BranchId)",
            "    tick = sweep · every 1s",
            "}",
            "owns HashMap<Id, ConnectionState>",
            "watchdog counter inside",
        ],
        dy=78,
        sp=13,
    )
    tag(COORD, "single owner · zero locks")

    SRT = (2050, 250, 2462, 396)
    draw_hand_rect(d, SRT, CYAN, seed=14)
    box_title(SRT, "SRT SOURCE", CYAN)
    body(SRT, ["MPEG-TS over SRT"], dy=90, col=DIM)

    GST = (1884, 474, 2462, 812)
    draw_hand_rect(d, GST, WHITE, seed=15)
    box_title(GST, "GSTREAMER PIPELINE", WHITE)
    body(
        GST,
        ["srtsrc > demux > tee", "per-viewer branch:", "queue + whipclientsink"],
        dy=92,
        sp=16,
    )
    tag(GST, "hot-plug branches", align="right")

    SUP = (1884, 892, 2462, 1200)
    draw_hand_rect(d, SUP, PURPLE, seed=16)
    box_title(SUP, "SUPERVISOR (loop)", PURPLE)
    body(
        SUP,
        [
            "init > run > clean_up +",
            "signal.reset() > backoff",
            "on restart req:",
            "quit() + rerun (base delay)",
        ],
        dy=78,
        sp=13,
    )

    # --- connection lifecycle ---
    text(d, (86, 1236), "CONNECTION LIFECYCLE", font(40, bold=True), GREEN)
    upts = [(92 + i * 12, 1288 + 5 * math.sin(i * 0.6)) for i in range(50)]
    d.line(upts, fill=GREEN, width=4, joint="curve")
    text(
        d,
        (720, 1248),
        "channel (HTTP) = connection (signal) = branch (stream)  ·  one thing, "
        "three vantage points",
        font(29),
        DIM,
    )

    sy0, sy1 = 1350, 1476
    scy = (sy0 + sy1) / 2
    AO = (250, sy0, 578, sy1)
    AA = (700, sy0, 1028, sy1)
    ES = (1150, sy0, 1444, sy1)
    for b, lbl in ((AO, "AwaitingOffer"), (AA, "AwaitingAnswer"), (ES, "Established")):
        draw_hand_rect(
            d, b, GREEN, seed=(hash(lbl) % 50), lw=5, r=44, fill=(18, 26, 32)
        )
        text(
            d,
            ((b[0] + b[2]) / 2, (b[1] + b[3]) / 2 - 2),
            lbl,
            font(37, bold=True),
            WHITE,
            anchor="mm",
        )
    d.ellipse((150 - 19, scy - 19, 150 + 19, scy + 19), fill=WHITE)
    ex = 1556
    d.ellipse((ex - 25, scy - 25, ex + 25, scy + 25), outline=DIM, width=4)
    d.line((ex - 14, scy - 14, ex + 14, scy + 14), fill=DIM, width=4)
    d.line((ex - 14, scy + 14, ex + 14, scy - 14), fill=DIM, width=4)

    draw_edge(d, [(171, scy), (248, scy)], GREEN, lw=4, seed=21)
    draw_edge(d, [(580, scy), (698, scy)], GREEN, lw=4, seed=22)
    draw_edge(d, [(1030, scy), (1148, scy)], GREEN, lw=4, seed=23)
    draw_edge(d, [(1446, scy), (ex - 28, scy)], GREEN, lw=4, seed=24)
    lc = font(27)
    text(
        d, (210, 1286), "POST /channel\nadd_branch ok", lc, DIM, anchor="ma", spacing=6
    )
    text(
        d,
        (639, 1286),
        "offer arrives\n(loopback WHIP)",
        lc,
        DIM,
        anchor="ma",
        spacing=6,
    )
    text(d, (1089, 1300), "PATCH answer", lc, DIM, anchor="ma")
    text(d, (1500, 1286), "DELETE /\npeer gone", lc, DIM, anchor="ma", spacing=6)

    # SWEEP box + dashed reaper arrows
    SWEEP = (430, 1512, 1010, 1672)
    draw_hand_rect(d, SWEEP, RED, seed=31)
    box_title(SWEEP, "SWEEP · 1s reaper", RED)
    body(
        SWEEP,
        ["deadline passed / abandoned:", "Err(Timeout) · remove_branch"],
        dy=76,
        sp=12,
    )
    p = poly_from([(414, sy1), (600, 1512)], 2, 41)
    dashed_line(d, p, RED, 4)
    arrowhead(d, p[-1], p[-2], RED)
    p = poly_from([(864, sy1), (830, 1512)], 2, 42)
    dashed_line(d, p, RED, 4)
    arrowhead(d, p[-1], p[-2], RED)
    text(d, (300, 1488), "timeout / abandoned", font(27), DIM, anchor="ma")

    # WATCHDOG box + dashed escalation to supervisor (C6)
    WD = (2044, 1512, 2462, 1672)
    draw_hand_rect(d, WD, RED, seed=32)
    box_title(WD, "WATCHDOG", RED)
    body(WD, ["N consecutive fails (window)", "-> sends restart request"], dy=76, sp=12)
    wd_path = [(2252, 1512), (2210, 1202)]
    dashed_line(d, poly_from(wd_path, 2, 43), RED, 5)
    arrowhead(d, (2210, 1202), (2244, 1300), RED)
    text(
        d,
        (2276, 1356),
        "restart request\n(mpsc)",
        font(28),
        RED,
        anchor="la",
        spacing=6,
    )

    # --- main flow edges + labels ---
    draw_edge(d, [(278, 496), (278, 584)], ORANGE, lw=5, seed=51, arrow="both")
    text(d, (298, 522), "signaling HTTP", ll, DIM)

    draw_edge(d, [(472, 700), (838, 686)], WHITE, lw=5, seed=52)
    text(
        d,
        (712, 610),
        "SignalHandle\ncmd -> oneshot reply",
        ll,
        DIM,
        anchor="ma",
        spacing=6,
    )

    draw_edge(d, [(1568, 616), (1882, 616)], WHITE, lw=5, seed=53)
    text(d, (1600, 528), "add_branch\nremove_branch", ll, DIM, spacing=6)

    reap_path = [(1884, 760), (1568, 786)]
    reap = poly_from(reap_path, 2, 54)
    dashed_line(d, reap, RED, 4)
    arrowhead(d, reap[-1], reap[-2], RED)
    text(d, (1725, 798), "branch failed -> reap chan", ll, DIM, anchor="ma")

    draw_edge(d, [(2256, 398), (2256, 470)], CYAN, lw=5, seed=55)
    text(d, (2278, 414), "media in", ll, DIM)

    draw_edge(d, [(2380, 890), (2380, 814)], PURPLE, lw=5, seed=56, arrow="end")
    text(d, (2050, 846), "run / quit / clean_up", ll, DIM, anchor="la")

    # Both cross-canvas flows originate at the GStreamer pipeline (right) and
    # land on the box they actually reach. Media (blue) arcs over the top into
    # the WHEP VIEWERS box; the loopback-WHIP offer (green) dips under the
    # coordinator into the HTTP ROUTES box (the /whip_sink route). Paths run
    # right->left so the arrowhead and the moving particles both read
    # GStreamer -> destination.
    media = bezier((1900, 508), (1200, 210), (472, 400))
    draw_edge(d, media, BLUE, lw=4, seed=61, arrow="end", amp=1.6)
    text(d, (1200, 258), "WebRTC media -> each viewer", ll, DIM, anchor="ma")
    whip = bezier((1900, 792), (1200, 1288), (472, 850))
    draw_edge(d, whip, GREEN, lw=4, seed=62, arrow="end", amp=1.6)
    text(
        d,
        (1200, 1120),
        "loopback WHIP  ·  whipclientsink POSTs its SDP offer -> /whip_sink/{id}",
        ll,
        DIM,
        anchor="ma",
    )

    particles = [
        {"path": [(278, 496), (278, 584)], "color": ORANGE, "loops": 1, "phase": 0.0},
        {"path": [(472, 700), (838, 686)], "color": ORANGE, "loops": 1, "phase": 0.3},
        {"path": [(1568, 616), (1882, 616)], "color": ORANGE, "loops": 1, "phase": 0.1},
        {"path": reap_path, "color": RED, "loops": 1, "phase": 0.5},
        {"path": [(2256, 398), (2256, 470)], "color": CYAN, "loops": 1, "phase": 0.6},
        {"path": [(2380, 890), (2380, 814)], "color": PURPLE, "loops": 1, "phase": 0.2},
        {"path": media, "color": BLUE, "loops": 1, "phase": 0.0},
        {"path": media, "color": BLUE, "loops": 1, "phase": 0.5},
        {"path": whip, "color": GREEN, "loops": 1, "phase": 0.25},
        {"path": whip, "color": GREEN, "loops": 1, "phase": 0.75},
        {"path": [(171, scy), (248, scy)], "color": GREEN, "loops": 1, "phase": 0.0},
        {"path": [(580, scy), (698, scy)], "color": GREEN, "loops": 1, "phase": 0.15},
        {"path": [(1030, scy), (1148, scy)], "color": GREEN, "loops": 1, "phase": 0.3},
        {
            "path": [(1446, scy), (ex - 28, scy)],
            "color": GREEN,
            "loops": 1,
            "phase": 0.45,
        },
        {"path": wd_path, "color": RED, "loops": 1, "phase": 0.0},
    ]
    return img, particles


# ----- animation ----------------------------------------------------------


def arclen(path):
    cum = [0.0]
    for i in range(1, len(path)):
        cum.append(
            cum[-1]
            + math.hypot(path[i][0] - path[i - 1][0], path[i][1] - path[i - 1][1])
        )
    return cum


def point_at(path, cum, u):
    total = cum[-1]
    target = (u % 1.0) * total
    for i in range(1, len(path)):
        if cum[i] >= target:
            seg = cum[i] - cum[i - 1] or 1.0
            f = (target - cum[i - 1]) / seg
            return (
                path[i - 1][0] + (path[i][0] - path[i - 1][0]) * f,
                path[i - 1][1] + (path[i][1] - path[i - 1][1]) * f,
            )
    return path[-1]


def render_frame(base, particles, t):
    # particle paths are in base space; scale positions/sizes to the raster.
    halo = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    hd = ImageDraw.Draw(halo)
    cores = []
    R = 34 * SCALE
    for p in particles:
        u = t * p["loops"] + p["phase"]
        bx, by = point_at(p["path"], p["_cum"], u)
        x, y = bx * SCALE, by * SCALE
        col = p["color"]
        hd.ellipse((x - R, y - R, x + R, y + R), fill=col + (120,))
        cores.append((x, y, col))
    halo = halo.filter(ImageFilter.GaussianBlur(16 * SCALE))
    frame = base.convert("RGBA")
    frame.alpha_composite(halo)
    fd = ImageDraw.Draw(frame)
    r, rc = 11 * SCALE, 5 * SCALE
    for x, y, col in cores:
        fd.ellipse((x - r, y - r, x + r, y + r), fill=col + (255,))
        fd.ellipse((x - rc, y - rc, x + rc, y + rc), fill=(255, 255, 255, 255))
    return frame.convert("RGB")


def main():
    outdir = os.path.dirname(os.path.abspath(__file__))
    base, particles = build_base()
    for p in particles:
        p["_cum"] = arclen(p["path"])

    if "--png-only" in sys.argv:
        base.save(os.path.join(outdir, "preview.png"))
        print("wrote preview.png")
        return

    # Dump PNG frames; the GIF is assembled by ffmpeg (palettegen/paletteuse)
    # for a small, high-quality file. Pillow's own GIF encoder bloats badly.
    fdir = os.path.join(outdir, "frames")
    os.makedirs(fdir, exist_ok=True)
    for i in range(NFRAMES):
        frame = render_frame(base, particles, i / NFRAMES)
        if i == 0:
            frame.save(os.path.join(outdir, "preview.png"))
        frame.save(os.path.join(fdir, f"f{i:03d}.png"))
    print(f"wrote {NFRAMES} PNG frames to {fdir}")


if __name__ == "__main__":
    main()
