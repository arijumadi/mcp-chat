//! On-disk secret storage for a headless host.
//!
//! The desktop build uses the OS keychain, which needs a Secret Service daemon
//! and a session D-Bus. A server has neither, so the same `SecretStore` trait is
//! implemented against a file with owner-only permissions instead.
//!
//! This is weaker than a keychain: anyone who can read the data directory can
//! read the keys. That is the honest trade for running without a desktop
//! session, and it is why the file is written 0600 and the directory is the
//! app's own.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::Mutex;

use mcp_chat_core::secrets::SecretStore;
use mcp_chat_core::{CoreError, Result};

pub struct FileSecretStore {
    path: PathBuf,
    /// Serializes read-modify-write cycles. The server is single-user, so one
    /// lock for the whole file is simpler than per-account locking and cannot
    /// deadlock.
    lock: Mutex<()>,
}

impl FileSecretStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    async fn read_all(&self) -> Result<std::collections::BTreeMap<String, String>> {
        match tokio::fs::read(&self.path).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                CoreError::message(format!("Secrets file is not valid JSON: {error}"))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(std::collections::BTreeMap::new())
            }
            Err(error) => Err(CoreError::message(format!(
                "Could not read {}: {error}",
                self.path.display()
            ))),
        }
    }

    async fn write_all(&self, values: &std::collections::BTreeMap<String, String>) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(values)
            .map_err(|error| CoreError::message(format!("Could not encode secrets: {error}")))?;
        // Write beside the target and rename, so a crash never leaves a
        // half-written secrets file behind.
        let temp = self.path.with_extension("json.tmp");
        tokio::fs::write(&temp, &bytes).await.map_err(|error| {
            CoreError::message(format!("Could not write {}: {error}", temp.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o600);
            tokio::fs::set_permissions(&temp, permissions)
                .await
                .map_err(|error| {
                    CoreError::message(format!("Could not restrict {}: {error}", temp.display()))
                })?;
        }
        tokio::fs::rename(&temp, &self.path).await.map_err(|error| {
            CoreError::message(format!(
                "Could not replace {}: {error}",
                self.path.display()
            ))
        })
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn set(&self, account: &str, secret: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut values = self.read_all().await?;
        values.insert(account.to_owned(), secret.to_owned());
        self.write_all(&values).await
    }

    async fn get(&self, account: &str) -> Result<Option<String>> {
        let _guard = self.lock.lock().await;
        let values = self.read_all().await?;
        Ok(values.get(account).cloned())
    }

    async fn delete(&self, account: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let mut values = self.read_all().await?;
        if values.remove(account).is_none() {
            return Ok(());
        }
        self.write_all(&values).await
    }
}

/// Make sure the containing directory exists before the store is used.
pub fn prepare(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
