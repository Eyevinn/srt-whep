# Supported Video Codecs & Profiles

srt-whep does **not** transcode — it forwards whatever the SRT source encodes, so
the H.264 **profile the browser negotiates against is chosen by your encoder**, not
by srt-whep. The tables below record which profiles play in each browser, alongside
the `profile-level-id` srt-whep actually advertised in its WebRTC SDP offer.

> ⚠️ **`profile-level-id` values are encoder-dependent.** Browsers accept or reject
> based on the *profile* (Constrained Baseline / Main / High / …); the exact hex
> varies by encoder (constraint-flag bits) and by the **level**, which depends on
> resolution/framerate/bitrate. The IDs below were observed at **1280×720, Level
> 3.1**. For example, x264's "baseline" and "constrained-baseline" both emit
> `42c01f`, and only Apple VideoToolbox emits Constrained High `640c1f`.

## macOS — verified 2026-07-08

Stack: GStreamer framework **1.28.4** (rswebrtc 0.15.2), current Chrome & Safari.
Sources generated with x264 (`x264enc`) except Constrained High, which only Apple
VideoToolbox (`h264_videotoolbox`) emits.

| Profile | Offered `profile-level-id` | Chrome | Safari | Notes |
|---------|----------------------------|:------:|:------:|-------|
| Constrained Baseline | `42c01f` | ✅ | ✅ | The only profile Safari accepts over WHEP. |
| Baseline | `42c01f` | ✅ | ✅ | x264's `baseline` *is* constrained baseline (it sets `constraint_set1`); a true non-constrained Baseline (`42001f`) is not produced by common encoders, so it is byte-identical to the row above. |
| Main | `4d401f` | ✅ | ❌ | Safari refuses at SDP negotiation (`m=video` port 0). |
| High | `64001f` | ✅ | ❌ | Safari refuses at SDP negotiation (`m=video` port 0). |
| High 4:4:4 Predictive | — | ❌ | ❌ | **Not deliverable:** the WebRTC sink refuses 4:4:4 H.264, so srt-whep never produces an SDP offer (`POST /channel` → HTTP 503). No browser can connect. |
| Constrained High | `640c1f` | ❌ | ❔ | Chrome refuses at SDP negotiation (`m=video` port 0); Safari could not be confirmed. Treat as **unsupported**. |

## Ubuntu — not re-verified (historical)

These rows are from an earlier test pass and were **not** re-checked on 2026-07-08.
The `profile-level-id` values are the canonical Level-3.1 forms rather than what a
specific encoder emits (see the note above).

| Profile | `profile-level-id` | Chrome |
|---------|--------------------|:------:|
| Constrained Baseline | `42e01f` | ✅ |
| Baseline | `42001f` | ✅ |
| Main | `4d001f` | ✅ |
| High | `64001f` | ❔ |
| High 4:4:4 Predictive | `f4001f` | ✅ |
| Constrained High | `640c1f` | ❔ |

## Takeaways

- **Safari (macOS) only plays Constrained Baseline** over WebRTC/WHEP. For a
  Safari-compatible stream, encode H.264 constrained-baseline — e.g.
  `x264enc … ! video/x-h264,profile=constrained-baseline` or ffmpeg
  `-profile:v baseline`.
- **Chrome (macOS) plays Constrained Baseline, Main, and High**, but **not**
  Constrained High (`640c1f`) nor High 4:4:4.
- **High 4:4:4 does not work at all** on the current GStreamer stack — srt-whep
  cannot produce an offer for it (this differs from an earlier pass that marked
  Chrome ✓).

## Troubleshooting: "connects but no media" in Safari

If a stream plays in Chrome but Safari shows no media **even for Constrained
Baseline**, the cause is usually the **player**, not the codec:

1. **Mixed content.** The hosted WHEP player is served over **HTTPS**; a request
   from it to `http://localhost:8000/channel` is mixed content, which Safari
   blocks more aggressively than Chrome.
2. **Wrong WHEP mode.** Some players attempt **client-initiated** WHEP (they POST
   their own SDP offer). srt-whep is **server-initiated only** and rejects a
   non-empty POST body with *"Empty body expected. Client initialization not
   supported."*

Use a player served over plain **`http://`** on the same host that speaks
server-initiated WHEP: empty `POST /channel` → read the server's offer and
`Location` → `PATCH` your answer to that `Location`.

See also the codec-test harness under
[`.claude/skills/run-srt-whep-codec-test`](../.claude/skills/run-srt-whep-codec-test)
to reproduce this table (per-profile sources + a local diagnostic player).
