use super::{MyError, SessionDescription};
use crate::pipeline::Args;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

struct Connection {
    whip_offer: SessionDescription,
    whep_offer: Option<SessionDescription>,
}

pub struct ReturnValues {
    pub sdp: SessionDescription,
    pub resource_id: String,
}

struct AppState {
    connections: HashMap<String, Connection>,
    args: Option<Args>,
}

impl AppState {
    fn new(_args: Args) -> Self {
        Self {
            connections: HashMap::new(),
            args: Some(_args),
        }
    }
}

#[derive(Clone)]
pub struct SharableAppState(Arc<Mutex<AppState>>);

impl SharableAppState {
    pub fn new(_args: Args) -> Self {
        Self(Arc::new(Mutex::new(AppState::new(_args))))
    }

    pub async fn get_args(&self) -> Result<Args, MyError> {
        let app_state = self.0.lock().unwrap();
        if let Some(resource_id) = &app_state.args {
            return Ok(resource_id.clone());
        }

        Err(MyError::ResourceNotFound)
    }

    pub async fn save_whip_offer(
        &self,
        offer: SessionDescription,
        resource_id: Option<String>,
    ) -> Result<(), MyError> {
        println!("svae whip");
        let mut app_state = self.0.lock().unwrap();
        let connections = &mut app_state.connections;

        let rid = resource_id.unwrap();

        if let Some(_) = connections.get_mut(&rid) {
            return Err(MyError::RepeatedWhipOffer);
        }

        let temp = Connection {
            whip_offer: offer.to_owned(),
            whep_offer: None,
        };
        connections.insert(rid, temp);

        Ok(())
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
                con.whip_offer.set_as_active();

                if con.whep_offer.is_some() {
                    return Ok(con.whep_offer.clone().unwrap());
                }
            }

            drop(app_state);

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    pub async fn save_whep_offer(
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

    pub async fn wait_on_whip_offer(&self) -> Result<ReturnValues, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            let mut app_state = self.0.lock().unwrap();
            let connections = &mut app_state.connections;

            let mut key_value: String = "".to_string();

            for (key, value) in &mut *connections {
                if !Option::is_some(&value.whep_offer) {
                    key_value = key.to_string();
                }
            }

            if !key_value.is_empty() {
                if let Some(con) = connections.get_mut(&key_value) {
                    con.whip_offer.set_as_active();

                    return Ok(ReturnValues {
                        sdp: con.whip_offer.clone(),
                        resource_id: key_value,
                    });
                }
            }

            drop(app_state);

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
}
