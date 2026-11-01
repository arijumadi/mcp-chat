use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Process-local mutation locks keyed by resolved file path.
///
/// This does not protect against external editors, but it prevents two Jarvis
/// tools (including tools running in different conversations) from reading and
/// writing the same file at the same time.
#[derive(Clone, Default)]
pub struct FileLocks {
    inner: Arc<Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>>,
}

impl FileLocks {
    pub async fn acquire(&self, mut paths: Vec<PathBuf>) -> Vec<OwnedMutexGuard<()>> {
        paths.sort();
        paths.dedup();
        let mut locks = Vec::with_capacity(paths.len());
        for path in paths {
            let lock = {
                let mut inner = self.inner.lock().expect("file lock map");
                match inner.get(&path).and_then(Weak::upgrade) {
                    Some(lock) => lock,
                    None => {
                        let lock = Arc::new(AsyncMutex::new(()));
                        inner.insert(path, Arc::downgrade(&lock));
                        lock
                    }
                }
            };
            locks.push(lock);
        }
        let mut guards = Vec::with_capacity(locks.len());
        for lock in locks {
            guards.push(lock.lock_owned().await);
        }
        guards
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn serializes_the_same_path() {
        let locks = FileLocks::default();
        let first = locks.acquire(vec![PathBuf::from("/tmp/a")]).await;
        let counter = Arc::new(AtomicUsize::new(0));
        let task_counter = counter.clone();
        let task_locks = locks.clone();
        let task = tokio::spawn(async move {
            let _guard = task_locks.acquire(vec![PathBuf::from("/tmp/a")]).await;
            task_counter.fetch_add(1, Ordering::SeqCst);
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        drop(first);
        task.await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
