use crate::domain::new_subscriber::NewSubscriber;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct AppState {
    status: HashMap<String, String>,
    tokens: HashMap<Uuid, String>,
}

impl AppState {
    fn new() -> Self {
        Self {
            status: HashMap::new(),
            tokens: HashMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct SharableAppState(Arc<Mutex<AppState>>);

impl SharableAppState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(AppState::new())))
    }

    pub fn insert_subscriber(&self, subscriber: &NewSubscriber) -> Result<(), MyError> {
        // wait to hold the lock to the app_state and insert the email and name into the status HashMap
        // Block when the lock is held by another thread
        let mut app_state = self.0.lock().unwrap();
        if app_state.status.contains_key(subscriber.email.as_ref()) {
            return Err(MyError::RepeatedUserNameError);
        } else {
            app_state.status.insert(
                subscriber.email.as_ref().to_string(),
                subscriber.name.as_ref().to_string(),
            );
        }

        Ok(())
    }

    pub fn save_token(&self, subscriber_id: Uuid, subscription_token: &str) -> Result<(), MyError> {
        // Lock the app_state and insert the email and name into the status HashMap
        // Block when the lock is held by another thread
        let mut app_state = self.0.lock().unwrap();
        if app_state.tokens.contains_key(&subscriber_id) {
            return Err(MyError::RepeatedUserIdError);
        } else {
            app_state
                .tokens
                .insert(subscriber_id, subscription_token.to_string());
        }

        Ok(())
    }
}

#[derive(thiserror::Error)]
pub enum MyError {
    #[error("Poisoned lock")]
    PoisonedLockError,
    #[error("Repeated user id")]
    RepeatedUserIdError,
    #[error("Repeated user name")]
    RepeatedUserNameError,
}

// We are still using a bespoke implementation of `Debug` // to get a nice report using the error source chain
impl std::fmt::Debug for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

pub fn error_chain_fmt(
    e: &impl std::error::Error,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    writeln!(f, "{}\n", e)?;
    let mut current = e.source();
    while let Some(cause) = current {
        writeln!(f, "Caused by:\n\t{}", cause)?;
        current = cause.source();
    }
    Ok(())
}
