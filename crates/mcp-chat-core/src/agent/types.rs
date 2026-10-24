use crate::CoreError;
use crate::llm::FinishReason;

#[derive(Debug, Clone)]
pub enum StepOutcome {
    Completed {
        needs_continuation: bool,
    },
    Retry {
        error: StepError,
        decision: RetryDecision,
    },
    Continue {
        error: StepError,
    },
    OutputLimit {
        needs_continuation: bool,
    },
    Compacted,
    RecoverFull,
}

#[derive(Debug, Clone)]
pub struct StepError {
    pub message: String,
    pub retryable: bool,
    pub context_overflow: bool,
    pub auth: bool,
}

impl StepError {
    pub fn from_core(error: &CoreError) -> Self {
        let message = error.user_message();
        match error {
            CoreError::ContextOverflow => Self {
                message,
                retryable: false,
                context_overflow: true,
                auth: false,
            },
            CoreError::Cancelled => Self {
                message,
                retryable: false,
                context_overflow: false,
                auth: false,
            },
            CoreError::Unauthorized => Self {
                message,
                retryable: false,
                context_overflow: false,
                auth: true,
            },
            CoreError::Http(_) => Self {
                message,
                retryable: true,
                context_overflow: false,
                auth: false,
            },
            CoreError::Message(text) => {
                let lower = text.to_ascii_lowercase();
                Self {
                    message,
                    retryable: lower.contains("temporarily unavailable")
                        || lower.contains("rate limit")
                        || lower.contains("http 429")
                        || lower.contains("http 5")
                        || lower.contains("timed out")
                        || lower.contains("connection"),
                    context_overflow: crate::error::is_context_overflow_message(text),
                    auth: lower.contains("authentication failed") || lower.contains("api key"),
                }
            }
            _ => Self {
                message,
                retryable: false,
                context_overflow: false,
                auth: false,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RetryDecision {
    pub delay_ms: u64,
}

pub fn retry_delay(attempt: usize) -> RetryDecision {
    let base: u64 = match attempt {
        0 => 250,
        1 => 2_000,
        _ => 8_000,
    };
    let jitter = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
        % 250)
        .saturating_sub(125);
    RetryDecision {
        delay_ms: base.saturating_add(jitter),
    }
}

pub fn is_length(reason: &FinishReason) -> bool {
    matches!(reason, FinishReason::Length)
}

pub fn is_tool_finish(reason: &FinishReason) -> bool {
    matches!(reason, FinishReason::ToolCalls)
}

pub const CONTINUE_PROMPT: &str = "The previous response was interrupted. Continue from where you left off without repeating completed content.";
pub const OUTPUT_LIMIT_PROMPT: &str = "You hit the output token limit. Do not apologize or recap. Break the remaining work into smaller pieces.";
pub const MAX_STEPS_PROMPT: &str = "You have reached the maximum number of tool steps for this turn. Do not call any more tools. Finish with a concise summary of what was accomplished and what remains.";
pub const LOOP_GUARD_PROMPT: &str = "The same tool call was repeated at least three times with an identical result. Stop retrying it. Explain the blocker, change approach, or ask the user. Do not call another tool in this step.";
pub const CONTINUE_TURN_PROMPT: &str = "Continue from where you stopped. Do not repeat completed work. Use tools only when they are still necessary.";
