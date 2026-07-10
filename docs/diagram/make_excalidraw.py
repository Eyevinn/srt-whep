#!/usr/bin/env python3
"""Emit an editable .excalidraw source mirroring render.py's layout.

Excalidraw is static, so this captures the boxes / labels / arrows (not the
animated glow). Contract: fontFamily 5 (hand-drawn), unique ids, files empty.
Coordinates match render.py so a future maintainer can edit this and re-render.
"""

import json
import os

WHITE = "#e8ecf0"
DIM = "#96a2ad"
GREEN = "#3ddc84"
CYAN = "#56d4e8"
PURPLE = "#b078e8"
RED = "#ff6b5e"
ORANGE = "#f0a850"
BLUE = "#60a0f0"

_n = [0]


def _id():
    _n[0] += 1
    return f"el{_n[0]:03d}"


def base(extra):
    e = dict(
        id=_id(),
        angle=0,
        backgroundColor="transparent",
        fillStyle="solid",
        strokeWidth=2,
        strokeStyle="solid",
        roughness=1,
        opacity=100,
        seed=_n[0] * 101 + 7,
        version=1,
        versionNonce=_n[0] * 131 + 3,
        isDeleted=False,
        groupIds=[],
        frameId=None,
        boundElements=[],
        updated=1,
        link=None,
        locked=False,
    )
    e.update(extra)
    return e


def rect(x0, y0, x1, y1, color, fill="transparent"):
    return base(
        dict(
            type="rectangle",
            x=x0,
            y=y0,
            width=x1 - x0,
            height=y1 - y0,
            strokeColor=color,
            backgroundColor=fill,
            roundness={"type": 3},
        )
    )


def txt(x, y, s, color, size=28, align="left"):
    lines = s.split("\n")
    w = max((len(ln) for ln in lines), default=1) * size * 0.55
    h = len(lines) * size * 1.25
    return base(
        dict(
            type="text",
            x=x,
            y=y,
            width=w,
            height=h,
            strokeColor=color,
            strokeWidth=1,
            text=s,
            fontSize=size,
            fontFamily=5,
            textAlign=align,
            verticalAlign="top",
            containerId=None,
            originalText=s,
            lineHeight=1.25,
            autoResize=True,
        )
    )


def arrow(x0, y0, x1, y1, color, dashed=False, both=False):
    e = base(
        dict(
            type="arrow",
            x=x0,
            y=y0,
            width=x1 - x0,
            height=y1 - y0,
            strokeColor=color,
            points=[[0, 0], [x1 - x0, y1 - y0]],
            lastCommittedPoint=None,
            startBinding=None,
            endBinding=None,
            startArrowhead="arrow" if both else None,
            endArrowhead="arrow",
            roundness={"type": 2},
        )
    )
    if dashed:
        e["strokeStyle"] = "dashed"
    return e


def main():
    els = []
    # title / subtitle
    els.append(
        txt(
            76,
            66,
            "srt-whep — one SRT in, many WHEP out, run by a " "COORDINATOR ACTOR",
            WHITE,
            46,
        )
    )
    els.append(
        txt(
            78,
            158,
            "v2.0.0 signal plane · one task owns all state · "
            "messages in, replies out · lifecycle is an explicit state "
            "machine",
            DIM,
            26,
        )
    )
    els.append(txt(2100, 74, "Kun Wu · Eyevinn", DIM, 28))

    # boxes: (x0,y0,x1,y1, color, title, body)
    boxes = [
        (86, 296, 470, 494, WHITE, "WHEP VIEWERS", "browsers · N sessions"),
        (
            86,
            586,
            470,
            968,
            WHITE,
            "HTTP ROUTES (actix)",
            "POST   /channel\nPATCH  /channel/{id}\nDELETE /channel/{id}\n"
            "POST   /whip_sink/{id}\n\n[thin adapters · no state]",
        ),
        (
            840,
            474,
            1566,
            958,
            GREEN,
            "COORDINATOR · signaling actor",
            "select! {\n    cmd  = mailbox (mpsc)\n    fail = reap chan (BranchId)\n"
            "    tick = sweep · every 1s\n}\nowns HashMap<Id, ConnectionState>\n"
            "watchdog counter inside\n[single owner · zero locks]",
        ),
        (2050, 250, 2462, 396, CYAN, "SRT SOURCE", "MPEG-TS over SRT"),
        (
            1884,
            474,
            2462,
            812,
            WHITE,
            "GSTREAMER PIPELINE",
            "srtsrc > demux > tee\nper-viewer branch:\nqueue + whipclientsink\n"
            "[hot-plug branches]",
        ),
        (
            1884,
            892,
            2462,
            1200,
            PURPLE,
            "SUPERVISOR (loop)",
            "init > run > clean_up +\nsignal.reset() > backoff\non restart req:\n"
            "quit() + rerun (base delay)",
        ),
        (250, 1350, 578, 1476, GREEN, "AwaitingOffer", ""),
        (700, 1350, 1028, 1476, GREEN, "AwaitingAnswer", ""),
        (1150, 1350, 1444, 1476, GREEN, "Established", ""),
        (
            430,
            1512,
            1010,
            1672,
            RED,
            "SWEEP · 1s reaper",
            "deadline passed / abandoned:\nErr(Timeout) · remove_branch",
        ),
        (
            2044,
            1512,
            2462,
            1672,
            RED,
            "WATCHDOG",
            "N consecutive fails (window)\n-> sends restart request",
        ),
    ]
    for x0, y0, x1, y1, color, title, body in boxes:
        els.append(rect(x0, y0, x1, y1, color))
        state_box = title in ("AwaitingOffer", "AwaitingAnswer", "Established")
        if state_box:
            els.append(txt(x0 + 24, (y0 + y1) / 2 - 22, title, WHITE, 34))
        else:
            els.append(txt(x0 + 28, y0 + 22, title, color, 34))
            if body:
                els.append(txt(x0 + 28, y0 + 80, body, WHITE, 26))

    # section label
    els.append(txt(86, 1236, "CONNECTION LIFECYCLE", GREEN, 36))
    els.append(
        txt(
            700,
            1244,
            "channel (HTTP) = connection (signal) = branch "
            "(stream) · one thing, three vantage points",
            DIM,
            26,
        )
    )

    # main flow arrows
    els.append(arrow(278, 494, 278, 586, ORANGE, both=True))
    els.append(arrow(472, 700, 838, 686, WHITE))
    els.append(arrow(1568, 616, 1882, 616, WHITE))
    els.append(arrow(1884, 760, 1568, 786, RED, dashed=True))  # reap chan (C3)
    els.append(arrow(2256, 396, 2256, 474, CYAN))
    els.append(arrow(2380, 812, 2380, 892, PURPLE))  # run/quit/clean_up
    els.append(arrow(2252, 1512, 2210, 1202, RED, dashed=True))  # restart req (C6)
    # lifecycle arrows
    els.append(arrow(170, 1413, 250, 1413, GREEN))
    els.append(arrow(578, 1413, 700, 1413, GREEN))
    els.append(arrow(1028, 1413, 1150, 1413, GREEN))
    els.append(arrow(1444, 1413, 1530, 1413, GREEN))
    els.append(arrow(414, 1476, 600, 1512, RED, dashed=True))
    els.append(arrow(864, 1476, 830, 1512, RED, dashed=True))

    # edge labels
    for x, y, s, c in [
        (500, 616, "SignalHandle\ncmd -> oneshot reply", DIM),
        (1600, 528, "add_branch\nremove_branch", DIM),
        (1596, 792, "branch failed -> reap chan", DIM),
        (2278, 414, "media in", DIM),
        (2050, 846, "run / quit / clean_up", DIM),
        (2276, 1356, "restart request (mpsc)", RED),
        (
            560,
            200,
            "loopback WHIP · whipclientsink POSTs its SDP offer -> " "/whip_sink/{id}",
            DIM,
        ),
        (790, 1092, "WebRTC media -> each viewer", DIM),
        (300, 470, "signaling HTTP", DIM),
    ]:
        els.append(txt(x, y, s, c, 26))

    doc = dict(
        type="excalidraw",
        version=2,
        source="srt-whep-render",
        elements=els,
        appState=dict(gridSize=None, viewBackgroundColor="#0d1117"),
        files={},
    )
    out = os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "srt-whep-coordinator-actor.excalidraw",
    )
    with open(out, "w") as f:
        json.dump(doc, f, indent=2)
    # contract checks
    ids = [e["id"] for e in els]
    assert len(ids) == len(set(ids)), "duplicate ids"
    assert all(e.get("fontFamily") == 5 for e in els if e["type"] == "text")
    assert doc["files"] == {}
    print(
        f"wrote {out} · {len(els)} elements · unique ids · fontFamily 5 · files empty"
    )


if __name__ == "__main__":
    main()
