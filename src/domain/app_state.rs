use super::{MyError, SessionDescription};
use std::ops::{Deref, DerefMut};
use std::{
    collections::{hash_map::Entry, HashMap},
    result::Result::Ok,
    sync::Arc,
    time::Duration,
};
use timed_locks::Mutex;

// The maximum times to wait for an offer
static MAXWAITTIMES: u32 = 10;

// A struct to hold offer&answer for a webrtc connection
#[derive(Debug)]
struct Connection {
    whip_offer: Option<SessionDescription>,
    whep_answer: Option<SessionDescription>,
}

impl Connection {
    fn new() -> Self {
        Self {
            whip_offer: None,
            whep_answer: None,
        }
    }
}

// A struct to hold all the connections.
// The connections are stored in a hashmap with a unique id as the key
pub struct AppState {
    connections: HashMap<String, Connection>,
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

    pub async fn has_connection(&self, id: String) -> Result<bool, MyError> {
        // Try to hold the lock and check if the connection exists
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        Ok(connections.contains_key(&id))
    }

    pub async fn remove_connection(&self, id: String) -> Result<(), MyError> {
        tracing::debug!("Remove connection {} from app state", id);

        // Try to hold the lock and remove the connection
        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        connections
            .remove(&id)
            .map(|_| ())
            .ok_or(MyError::ResourceNotFound)
    }

    pub async fn list_connections(&self) -> Result<Vec<String>, MyError> {
        // Try to hold the lock and return a list of the connections
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
            Entry::Occupied(_) => Err(MyError::RepeatedResourceIdError(id)),
            Entry::Vacant(entry) => {
                entry.insert(Connection::new());
                Ok(())
            }
        }
    }

    pub async fn save_whip_offer(
        &self,
        conn_id: String,
        offer: SessionDescription,
    ) -> Result<String, MyError> {
        tracing::debug!("Save WHIP SDP offer: {:?}", offer);

        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        for (id, conn) in connections.iter_mut() {
            if conn.whep_answer.is_none() {
                if conn.whip_offer.is_none() && &conn_id == id {
                    conn.whip_offer = Some(offer);

                    return Ok(conn_id.clone());
                } else {
                    return Err(MyError::RepeatedResourceIdError(conn_id.clone()));
                }
            }
        }

        Err(MyError::ResourceNotFound)
    }

    pub async fn wait_on_whep_answer(&self, id: String) -> Result<SessionDescription, MyError> {
        // Check every second if an answer is ready
        // If the answer is ready, return it
        // If not, wait for a second and check again
        // If the answer is not ready after several tries, return an error
        for i in 0..MAXWAITTIMES {
            tracing::debug!(
                "Wait on WHEP SDP answer: {} for {}/{} times",
                id,
                i,
                MAXWAITTIMES
            );

            let mut app_state = self.lock_err().await?;
            let connections = &mut app_state.connections;

            if let Some(con) = connections.get_mut(&id) {
                if con.whep_answer.is_some() {
                    let whep_answer = con.whep_answer.as_mut().unwrap();
                    whep_answer.set_as_passive();

                    return Ok(con.whep_answer.clone().unwrap());
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Err(MyError::AnswerMissing)
    }

    pub async fn save_whep_answer(
        &self,
        offer: SessionDescription,
        id: String,
    ) -> Result<(), MyError> {
        tracing::debug!("Save WHEP SDP answer: {:?}", offer);

        let mut app_state = self.lock_err().await?;
        let connections = &mut app_state.connections;

        if let Some(con) = connections.get_mut(&id) {
            con.whep_answer = Some(offer);
            Ok(())
        } else {
            Err(MyError::ResourceNotFound)
        }
    }

    pub async fn wait_on_whip_offer(&self, id: String) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        // If not, wait for a second and check again
        // If the offer is not ready after several tries, return an error
        for i in 0..MAXWAITTIMES {
            tracing::debug!(
                "Wait on WHIP SDP offer: {} for {}/{} times",
                id,
                i,
                MAXWAITTIMES
            );

            let mut app_state = self.lock_err().await?;
            let connections = &mut app_state.connections;

            if let Some(con) = connections.get_mut(&id) {
                if con.whip_offer.is_some() {
                    let whip_offer = con.whip_offer.as_mut().unwrap();
                    whip_offer.set_as_active();

                    return Ok(con.whip_offer.clone().unwrap());
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Err(MyError::OfferMissing)
    }
}
