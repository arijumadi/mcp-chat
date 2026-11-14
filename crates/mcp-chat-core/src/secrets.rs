use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::{CoreError, KEYCHAIN_SERVICE, Result};

#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn set(&self, account: &str, secret: &str) -> Result<()>;
    async fn get(&self, account: &str) -> Result<Option<String>>;
    async fn delete(&self, account: &str) -> Result<()>;
}

#[derive(Default)]
pub struct KeyringStore;

#[async_trait]
impl SecretStore for KeyringStore {
    async fn set(&self, account: &str, secret: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|err| CoreError::message(format!("OS keychain unavailable: {err}")))?;
        entry
            .set_password(secret)
            .map_err(|err| CoreError::message(format!("Could not save to OS keychain: {err}")))?;
        Ok(())
    }

    async fn get(&self, account: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|err| CoreError::message(format!("OS keychain unavailable: {err}")))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(CoreError::message(format!(
                "Could not read from OS keychain: {err}"
            ))),
        }
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|err| CoreError::message(format!("OS keychain unavailable: {err}")))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(CoreError::message(format!(
                "Could not remove from OS keychain: {err}"
            ))),
        }
    }
}

#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl SecretStore for MemoryStore {
    async fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.values
            .lock()
            .expect("secret store lock")
            .insert(account.to_owned(), secret.to_owned());
        Ok(())
    }

    async fn get(&self, account: &str) -> Result<Option<String>> {
        Ok(self
            .values
            .lock()
            .expect("secret store lock")
            .get(account)
            .cloned())
    }

    async fn delete(&self, account: &str) -> Result<()> {
        self.values
            .lock()
            .expect("secret store lock")
            .remove(account);
        Ok(())
    }
}

pub fn provider_account(provider_id: &str) -> String {
    provider_id.to_owned()
}

pub fn server_bearer_account(server_id: &str) -> String {
    format!("server:{server_id}:bearer")
}

pub fn server_api_key_account(server_id: &str) -> String {
    format!("server:{server_id}:api_key")
}

pub fn oauth_account(server_id: &str, issuer: &str, suffix: &str) -> String {
    format!("oauth:{issuer}:{server_id}:{suffix}")
}

/// The OAuth session JSON for one server. It contains the access token,
/// optional refresh token and client registration details.
pub fn oauth_token_account(server_id: &str) -> String {
    format!("oauth:{server_id}:session")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_roundtrip() {
        let store = MemoryStore::default();
        store.set("provider-1", "sk-secret").await.unwrap();
        assert_eq!(
            store.get("provider-1").await.unwrap().as_deref(),
            Some("sk-secret")
        );
        store.delete("provider-1").await.unwrap();
        assert!(store.get("provider-1").await.unwrap().is_none());
    }
}
