use super::{MyError, SessionDescription};
use event_listener::{Event, Listener};
use std::ops::{Deref, DerefMut};
use std::{
    collections::{hash_map::Entry, HashMap},
    result::Result::Ok,
    sync::Arc,
    time::Duration,
};
use timed_locks::Mutex;

// The maximum time in seconds to wait for an offer/anwser
static MAXWAITTIMES: u32 = 10;

// A struct to hold offer&answer for a webrtc connection
#[derive(Debug)]
pub struct Connection {
    offer_available: Event,
    whip_offer: Mutex<Option<SessionDescription>>,
    answer_available: Event,
    whep_answer: Mutex<Option<SessionDescription>>,
}

impl Connection {
    fn new() -> Self {
        Self {
            offer_available: Event::new(),
            whip_offer: Mutex::new(None),
            answer_available: Event::new(),
            whep_answer: Mutex::new(None),
        }
    }
}

// A struct to hold all the connections.
// The connections are stored in a hashmap with a unique id as the key
pub struct AppState {
    connections: HashMap<String, Arc<Connection>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            connections: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct SharableAppState(Arc<Mutex<AppState>>);

impl Default for SharableAppState {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for SharableAppState {
    type Target = Arc<Mutex<AppState>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SharableAppState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl SharableAppState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new_with_timeout(
            AppState::new(),
            Duration::from_secs(5),
        )))
    }

    pub async fn reset(&self) -> Result<(), MyError> {
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        connections.clear();
        Ok(())
    }

    pub async fn has_connection(&self, id: String) -> Result<bool, MyError> {
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        Ok(connections.contains_key(&id))
    }

    pub async fn remove_connection(&self, id: String) -> Result<(), MyError> {
        tracing::debug!("Remove connection {} from app state", id);
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        connections
            .remove(&id)
            .map(|_| ())
            .ok_or(MyError::ConnectionNotFound(id))
    }

    pub async fn list_connections(&self) -> Result<Vec<String>, MyError> {
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        let keys = connections.keys().cloned().collect::<Vec<_>>();
        Ok(keys)
    }

    pub async fn add_connection(&self, id: String) -> Result<(), MyError> {
        tracing::debug!("Add connection {} to app state", id);
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        match connections.entry(id.clone()) {
            Entry::Occupied(_) => Err(MyError::RepeatedConnection(id)),
            Entry::Vacant(entry) => {
                entry.insert(Arc::new(Connection::new()));
                Ok(())
            }
        }
    }

    pub async fn get_connection(&self, id: String) -> Result<Arc<Connection>, MyError> {
        let app_state = self.lock_err().await?;
        let connections = &app_state.connections;

        connections
            .get(&id)
            .cloned()
            .ok_or(MyError::ConnectionNotFound(id))
    }

    pub async fn save_whip_offer(
        &self,
        id: String,
        offer: SessionDescription,
    ) -> Result<(), MyError> {
        tracing::debug!("Save WHIP SDP offer: {:?}", offer);

        let connect = self.get_connection(id.clone()).await?;
        let mut whip_offer = connect.whip_offer.lock().await;
        *whip_offer = Some(offer);
        connect.offer_available.notify(1);
        Ok(())
    }

    pub async fn wait_on_whep_answer(&self, id: String) -> Result<SessionDescription, MyError> {
        tracing::debug!(
            "Wait on WHEP SDP answer: {} for {} seconds",
            id,
            MAXWAITTIMES
        );

        let connect = self.get_connection(id.clone()).await?;
        let answer_await = connect.answer_available.listen();
        if answer_await
            .wait_timeout(Duration::from_secs(MAXWAITTIMES as u64))
            .is_some()
        {
            Ok(connect.whep_answer.lock().await.clone().unwrap())
        } else {
            Err(MyError::AnswerMissing)
        }
    }

    pub async fn save_whep_answer(
        &self,
        id: String,
        offer: SessionDescription,
    ) -> Result<(), MyError> {
        tracing::debug!("Save WHEP SDP answer: {:?}", offer);
        let connect = self.get_connection(id.clone()).await?;
        let mut whep_answer = connect.whep_answer.lock().await;
        *whep_answer = Some(offer);
        connect.answer_available.notify(1);
        Ok(())
    }

    pub async fn wait_on_whip_offer(&self, id: String) -> Result<SessionDescription, MyError> {
        tracing::debug!(
            "Wait on WHIP SDP offer: {} for {} seconds",
            id,
            MAXWAITTIMES
        );

        let connect = self.get_connection(id.clone()).await?;
        let offer_await = connect.offer_available.listen();
        if offer_await
            .wait_timeout(Duration::from_secs(MAXWAITTIMES as u64))
            .is_some()
        {
            Ok(connect.whip_offer.lock().await.clone().unwrap())
        } else {
            Err(MyError::OfferMissing)
        }
    }
}
