# Connection Lifecycle

*How one WHEP viewer goes from `POST /channel` to receiving live video — the
happy path, leg by leg. For the domain vocabulary see
[`CONTEXT.md`](../CONTEXT.md); for **why** the design is shaped this way, see
[`architecture-evolution-shared-lock-to-actor.md`](./architecture-evolution-shared-lock-to-actor.md).*

## The three-leg handshake

srt-whep uses **server-initiated** WHEP: the browser POSTs an *empty* body and
the *server* supplies the SDP offer. That offer comes out of the GStreamer
pipeline through an in-process loopback WHIP bridge (see `CONTEXT.md` →
*Loopback WHIP*). Three HTTP requests carry a viewer to live video:

```mermaid
sequenceDiagram
    participant B as Browser (WHEP client)
    participant C as Coordinator (src/signal)
    participant W as whipsink (in the pipeline)

    B->>C: ① POST /channel (empty body)
    Note over C: add_branch(id)<br/>state = AwaitingOffer (① reply parked)
    C-->>W: branch attached; whipclientsink starts
    W->>C: ② POST /whip_sink/{id} (SDP offer)
    C-->>B: ① returns 201 + SDP offer
    Note over C: state = AwaitingAnswer (② reply parked)
    B->>C: ③ PATCH /channel/{id} (SDP answer)
    C-->>W: ② returns 201 + SDP answer
    C-->>B: ③ returns 204 No Content
    Note over C: state = Established — media flows
```

## The one non-obvious idea: parked waiters

Legs ① and ② cannot be answered when they arrive. When the browser POSTs
(leg ①), the SDP offer does not exist yet — the whipsink produces it only after
its branch is attached. So the coordinator does not reply; it **parks** the
reply channel inside the connection's state and returns to its loop:

```rust
// src/signal/coordinator.rs
enum ConnectionState {
    AwaitingOffer  { whep_reply: OfferReply,  deadline: Instant }, // leg ① parked here
    AwaitingAnswer { whip_reply: AnswerReply, deadline: Instant }, // leg ② parked here
    Established    { since: Instant },
}
```

When leg ② delivers the offer, the parked leg-① reply fires — that is the moment
the browser's original POST finally returns its `201`. Leg ② then parks in
turn, waiting for the browser's answer in leg ③. Each waiting state also carries
a `deadline`, so an abandoned handshake cannot park forever (the *sweep*
expires it — see `CONTEXT.md` → *Sweep*).

**Why this matters:** "viewer stuck / no response to the POST" almost always
means a parked waiter that never received its delivery. Knowing *where* each leg
parks tells you where to look.

## HTTP leg → command → state

| HTTP (route in `src/startup.rs`) | Caller | Command | Resulting state | Replies |
|---|---|---|---|---|
| `POST /channel` (empty body) | Browser | `CreateConnection` | `AwaitingOffer` | parked → `201` + SDP offer |
| `POST /whip_sink/{id}` (offer) | whipsink (loopback WHIP) | `OfferReceived` | `AwaitingAnswer` | parked → `201` + SDP answer |
| `PATCH /channel/{id}` (answer) | Browser | `AnswerReceived` | `Established` | immediate `204` |
| `DELETE /channel/{id}` or `/whip_sink/{id}` | either side | `RemoveConnection` | (removed) | immediate |
| `GET /list` | operator | `ListConnections` | (unchanged) | immediate JSON |

## Where to go next

- **Vocabulary and module map:** [`CONTEXT.md`](../CONTEXT.md)
- **Why an actor, the failure model, and the serialization trade-off:**
  [`architecture-evolution-shared-lock-to-actor.md`](./architecture-evolution-shared-lock-to-actor.md)
- **The decisions:** [`docs/adr/`](./adr/)
- **The code (and its `#[cfg(test)]` spec):** `src/signal/coordinator.rs`
