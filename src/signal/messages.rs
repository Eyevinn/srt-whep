use super::errors::SignalError;
use crate::domain::{SdpAnswer, SdpOffer};
use serde::Serialize;
use tokio::sync::oneshot;

pub type ConnectionId = String;
pub type OfferReply = oneshot::Sender<Result<SdpOffer, SignalError>>;
pub type AnswerReply = oneshot::Sender<Result<SdpAnswer, SignalError>>;
pub type UnitReply = oneshot::Sender<Result<(), SignalError>>;

/// One entry in GET /list output.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub state: String,
}

#[derive(Debug)]
pub enum Command {
    /// WHEP POST: create the connection; reply carries the SDP offer once the
    /// whipsink delivers it (or an error on timeout/failure).
    CreateConnection { id: ConnectionId, reply: OfferReply },
    /// Loopback WHIP POST: the whipsink's offer; reply carries the SDP answer
    /// once the browser PATCHes it (or an error on timeout/failure).
    OfferReceived {
        id: ConnectionId,
        sdp: SdpOffer,
        reply: AnswerReply,
    },
    /// WHEP PATCH: the browser's answer; replied to immediately.
    AnswerReceived {
        id: ConnectionId,
        sdp: SdpAnswer,
        reply: UnitReply,
    },
    /// WHEP DELETE (or internal cleanup).
    RemoveConnection { id: ConnectionId, reply: UnitReply },
    ListConnections {
        reply: oneshot::Sender<Vec<ConnectionInfo>>,
    },
    /// Supervisor: the pipeline restarted; fail all waiters, clear the map.
    Reset { reply: oneshot::Sender<()> },
}
