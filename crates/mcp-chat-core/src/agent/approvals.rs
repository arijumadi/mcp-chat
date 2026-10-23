use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Once,
    Always,
    Reject,
}

#[derive(Debug, Clone)]
pub struct ApprovalAnswer {
    pub decision: ApprovalDecision,
    pub feedback: Option<String>,
}

#[derive(Default)]
pub struct ApprovalRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalAnswer>>>,
}

impl ApprovalRegistry {
    pub fn register(&self, call_id: &str) -> oneshot::Receiver<ApprovalAnswer> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("approval registry lock")
            .insert(call_id.to_owned(), sender);
        receiver
    }

    pub fn respond(&self, call_id: &str, answer: ApprovalAnswer) -> bool {
        self.pending
            .lock()
            .expect("approval registry lock")
            .remove(call_id)
            .map(|sender| sender.send(answer).is_ok())
            .unwrap_or(false)
    }

    pub fn cancel(&self, call_id: &str) {
        self.pending
            .lock()
            .expect("approval registry lock")
            .remove(call_id);
    }
}

#[derive(Debug, Clone)]
pub struct InputAnswer {
    pub responses: serde_json::Value,
    pub cancelled: bool,
}

#[derive(Default)]
pub struct InputRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<InputAnswer>>>,
}

impl InputRegistry {
    pub fn register(&self, call_id: &str) -> oneshot::Receiver<InputAnswer> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("input registry lock")
            .insert(call_id.to_owned(), sender);
        receiver
    }

    pub fn respond(&self, call_id: &str, answer: InputAnswer) -> bool {
        self.pending
            .lock()
            .expect("input registry lock")
            .remove(call_id)
            .map(|sender| sender.send(answer).is_ok())
            .unwrap_or(false)
    }

    pub fn cancel(&self, call_id: &str) {
        self.pending
            .lock()
            .expect("input registry lock")
            .remove(call_id);
    }
}
