use super::{MyError, SessionDescription};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct AppState {
    whip_offer: Option<SessionDescription>,
    whep_offer: Option<SessionDescription>,
    resource_id: Option<String>,
}

impl AppState {
    fn new() -> Self {
        Self {
            whip_offer: None,
            whep_offer: None,
            resource_id: None,
        }
    }
}

#[derive(Clone)]
pub struct SharableAppState(Arc<Mutex<AppState>>);

impl SharableAppState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(AppState::new())))
    }

    pub async fn save_whip_offer(&self, offer: SessionDescription) -> Result<(), MyError> {
        let mut app_state = self.0.lock().unwrap();
        if app_state.whip_offer.is_some() {
            return Err(MyError::RepeatedWhipOffer);
        }
        app_state.whip_offer = Some(offer);

        Ok(())
    }

    pub async fn create_resource(&self) -> Result<String, MyError> {
        let mut app_state = self.0.lock().unwrap();
        let resource_id = Uuid::new_v4().to_string();
        app_state.resource_id = Some(resource_id.clone());

        Ok(resource_id)
    }

    pub async fn get_resource(&self) -> Result<String, MyError> {
        let app_state = self.0.lock().unwrap();
        if let Some(resource_id) = &app_state.resource_id {
            return Ok(resource_id.clone());
        }

        Err(MyError::ResourceNotFound)
    }

    pub async fn wait_on_whep_offer(&self) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            let mut app_state = self.0.lock().unwrap();
            if let Some(offer) = &mut app_state.whep_offer {
                offer.set_as_passive();

                return Ok(offer.clone());
            }
            drop(app_state);

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }

    pub async fn save_whep_offer(&self, offer: SessionDescription) -> Result<(), MyError> {
        let mut app_state = self.0.lock().unwrap();
        if app_state.whep_offer.is_some() {
            return Err(MyError::RepeatedWhepError);
        }
        app_state.whep_offer = Some(offer);

        Ok(())
    }

    pub async fn wait_on_whip_offer(&self) -> Result<SessionDescription, MyError> {
        // Check every second if an offer is ready
        // If the offer is ready, return it
        loop {
            let mut app_state: std::sync::MutexGuard<AppState> = self.0.lock().unwrap();
            if let Some(offer) = &mut app_state.whip_offer {
                offer.set_as_active();
                return Ok(offer.clone());
            }
            drop(app_state);

            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    }
    pub fn get_whip_offer(&self) -> Result<SessionDescription, MyError> {
        let app_state = self.0.lock().unwrap();
        if let Some(whip_offer) = &app_state.whip_offer {
            return Ok(whip_offer.clone());
        }

        Err(MyError::ResourceNotFound)
    }
}
