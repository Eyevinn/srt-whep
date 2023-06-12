use super::{MyError, SessionDescription};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
#[derive(Debug, Clone)]
struct Connection {
    whip_offer: Option<SessionDescription>,
    whep_offer: Option<SessionDescription>,
}

struct AppState {
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

impl SharableAppState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(AppState::new())))
    }

    pub async fn remove_connection(
        &self,
        resource_id: String,
    ) -> Result<(), MyError> {
        let mut app_state = self.0.lock().unwrap();
        let connections = &mut app_state.connections;

        if let Some(_) = connections.remove_entry(&resource_id) {
           return Ok(());
        }

        return Err(MyError::ResourceNotFound)
    }

    pub fn list_connections(
        &self,
    ) -> Result<Vec<String>, MyError> {
        let mut app_state = self.0.lock().unwrap();
        let connections = &mut app_state.connections;

        let keys = connections.clone().into_keys().collect();

        return Ok(keys);
    }

    pub fn add_resource(&self, resource_id: String) -> Result<(), MyError>  {

        let mut app_state = self.0.lock().unwrap();
        let connections = &mut app_state.connections;

        if let Some(_) = connections.get_mut(&resource_id) {
            return Err(MyError::RepeatedResourceIdError);
        }

        let temp = Connection {
            whip_offer: None,
            whep_offer: None,
        };

        connections.insert(resource_id, temp);

        return Ok(());
    } 

    pub fn save_whip_offer(
        &self,
        offer: SessionDescription,
    ) -> Result<String, MyError> {
        let mut app_state = self.0.lock().unwrap();
        let connections = &mut app_state.connections;

        let mut key_value = "".to_string();

        for (key, value) in &mut *connections {
            if !Option::is_some(&value.whep_offer) {
                key_value = key.to_string();
            }
        }

        if let Some(con) = connections.get_mut(&key_value) {
            if con.whip_offer.is_some() {
            return Err(MyError::RepeatedWhipOffer);
            }
            con.whip_offer = Some(offer);
        }

        Ok(key_value)
    }

    pub async fn wait_on_whep_offer(
        &self,
        resource_id: String,
    ) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            let mut app_state = self.0.lock().unwrap();
            let connections = &mut app_state.connections;

            if let Some(con) = connections.get_mut(&resource_id) {
                let whip_offer = con.whip_offer.as_mut().unwrap();
                whip_offer.set_as_active();

                if con.whep_offer.is_some() {
                    return Ok(con.whep_offer.clone().unwrap());
                }
            }

            drop(app_state);

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    pub fn save_whep_offer(
        &self,
        offer: SessionDescription,
        resource_id: Option<String>,
    ) -> Result<(), MyError> {
        if resource_id.is_none() {
            return Err(MyError::ResourceNotFound);
        }

        let mut app_state = self.0.lock().unwrap();

        let connections = &mut app_state.connections;
        let rid = resource_id.unwrap();

        if let Some(con) = connections.get_mut(&rid) {
            if con.whep_offer.is_some() {
                return Err(MyError::RepeatedWhepError);
            }
            con.whep_offer = Some(offer);
        }

        Ok(())
    }

    pub async fn wait_on_whip_offer(&self, resource_id: String) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            let mut app_state = self.0.lock().unwrap();
            let connections = &mut app_state.connections;

            if let Some(con) = connections.get_mut(&resource_id) {
                if con.whip_offer.is_some() {
                    let whip_offer = con.whip_offer.as_mut().unwrap();
                    whip_offer.set_as_active();

                    return Ok(con.whip_offer.clone().unwrap());
                }
            }

            drop(app_state);

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}
