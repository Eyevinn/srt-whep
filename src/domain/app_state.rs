use super::{MyError, SessionDescription};
use std::ops::{Deref, DerefMut};
use std::{
    collections::{hash_map::Entry, HashMap},
    result::Result::Ok,
    sync::Arc,
    time::Duration,
};
use timed_locks::Mutex;

// A struct to hold offer&answer for a webrtc connection
#[derive(Debug)]
struct Connection {
    whip_offer: Option<SessionDescription>,
    whep_offer: Option<SessionDescription>,
}

impl Connection {
    fn new() -> Self {
        Self {
            whip_offer: None,
            whep_offer: None,
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
            Duration::new(5, 0),
        )))
    }

    pub async fn remove_connection(&self, connection_id: String) -> Result<(), MyError> {
        let mut app_state = self.lock().await;
        let connections = &mut app_state.connections;

        connections
            .remove(&connection_id)
            .map(|_| ())
            .ok_or(MyError::ResourceNotFound)
    }

    pub async fn list_connections(&self) -> Result<Vec<String>, MyError> {
        let mut app_state = self.lock().await;
        let connections = &mut app_state.connections;

        let keys = connections.keys().cloned().collect::<Vec<_>>();
        Ok(keys)
    }

    pub async fn add_resource(&self, connection_id: String) -> Result<(), MyError> {
        let mut app_state = self.lock().await;
        let connections = &mut app_state.connections;

        match connections.entry(connection_id.clone()) {
            Entry::Occupied(_) => Err(MyError::RepeatedResourceIdError(connection_id)),
            Entry::Vacant(entry) => {
                entry.insert(Connection::new());
                Ok(())
            }
        }
    }

    pub async fn save_whip_offer(&self, offer: SessionDescription) -> Result<String, MyError> {
        let mut app_state = self.lock().await;
        let connections = &mut app_state.connections;

        for (id, conn) in connections.iter_mut() {
            if conn.whep_offer.is_none() {
                if conn.whip_offer.is_none() {
                    conn.whip_offer = Some(offer);

                    return Ok(id.clone());
                } else {
                    return Err(MyError::RepeatedResourceIdError(id.clone()));
                }
            }
        }

        Err(MyError::ResourceNotFound)
    }

    pub async fn wait_on_whep_offer(
        &self,
        connection_id: String,
    ) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            {
                let mut app_state = self.lock().await;
                let connections = &mut app_state.connections;

                if let Some(con) = connections.get_mut(&connection_id) {
                    if con.whep_offer.is_some() {
                        let whep_offer = con.whep_offer.as_mut().unwrap();
                        whep_offer.set_as_passive();

                        return Ok(con.whep_offer.clone().unwrap());
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    pub async fn save_whep_offer(
        &self,
        offer: SessionDescription,
        connection_id: String,
    ) -> Result<(), MyError> {
        let mut app_state = self.lock().await;
        let connections = &mut app_state.connections;

        if let Some(con) = connections.get_mut(&connection_id) {
            con.whep_offer = Some(offer);
            Ok(())
        } else {
            Err(MyError::ResourceNotFound)
        }
    }

    pub async fn wait_on_whip_offer(
        &self,
        connection_id: String,
    ) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            {
                let mut app_state = self.lock().await;
                let connections = &mut app_state.connections;

                if let Some(con) = connections.get_mut(&connection_id) {
                    if con.whip_offer.is_some() {
                        let whip_offer = con.whip_offer.as_mut().unwrap();
                        whip_offer.set_as_active();

                        return Ok(con.whip_offer.clone().unwrap());
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}
