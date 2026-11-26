//! Agent-neutral ACP events.
//!
//! The SDK's message types are not stable across protocol versions, so the
//! manager translates them into these values before the engine sees anything.
//! The engine then maps them onto `part`/`delta` rows in the child
//! conversation.

/// A slash command the agent says it understands.
///
/// The agent executes these itself; the client only forwards the text. Keeping
/// the list lets the composer suggest them instead of making the user memorise
/// what each external agent supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// Without a leading slash, as the protocol sends it.
    pub name: String,
    pub description: String,
    /// Hint shown for a command that takes arguments, when the agent gave one.
    pub input_hint: Option<String>,
}

/// One value of a session configuration option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

/// A model, thinking level, or mode the agent lets the user pick.
///
/// Agents differ in where they put this: Antigravity returns it in the
/// `session/new` response, OpenCode announces changes later. Both are
/// normalised into this shape so the UI has one thing to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub current_value: String,
    /// Empty for a boolean option; `kind` says which this is.
    pub choices: Vec<ConfigChoice>,
    /// `select` or `boolean`.
    pub kind: String,
}

/// One update streamed by an ACP agent.
#[derive(Debug, Clone, PartialEq)]
pub enum AcpEvent {
    /// Assistant-visible text.
    Text(String),
    /// Internal reasoning.
    Thought(String),
    /// A new tool call, or an update to one already announced.
    ToolCall {
        call_id: String,
        title: String,
        kind: Option<String>,
        /// `pending` | `in_progress` | `completed` | `failed`
        status: String,
        /// Bounded, already truncated by the manager.
        detail: Option<String>,
    },
    /// The agent replaced its plan.
    Plan(String),
    /// The set of slash commands the agent supports, replaced wholesale.
    Commands(Vec<AgentCommand>),
    /// The agent's configurable options, replaced wholesale.
    ConfigOptions(Vec<ConfigOption>),
    /// A permission request that needs an answer.
    PermissionRequested {
        request_id: String,
        title: String,
        kind: Option<String>,
        options: Vec<String>,
    },
    /// The turn finished. `stop_reason` is the raw ACP value.
    TurnEnded { stop_reason: String },
    /// The turn failed or the process died.
    TurnFailed { message: String },
}

/// Terminal state of one prompt round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Refused(String),
    Failed(String),
}

impl TurnOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Accumulates the assistant text of one turn using the segment rules from the
/// plan:
///
/// - a tool call ends the current segment,
/// - the report is the text after the last tool call,
/// - otherwise the last non-empty segment.
#[derive(Debug, Default, Clone)]
pub struct TurnTranscript {
    segments: Vec<String>,
    current: String,
}

impl TurnTranscript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_text(&mut self, text: &str) {
        self.current.push_str(text);
    }

    pub fn end_segment(&mut self) {
        if !self.current.trim().is_empty() {
            self.segments.push(std::mem::take(&mut self.current));
        } else {
            self.current.clear();
        }
    }

    /// Final text after the last tool call, or the last non-empty segment.
    pub fn report(&self) -> Option<String> {
        if !self.current.trim().is_empty() {
            return Some(self.current.trim().to_owned());
        }
        self.segments
            .iter()
            .rev()
            .find(|segment| !segment.trim().is_empty())
            .map(|segment| segment.trim().to_owned())
    }

    pub fn all_text(&self) -> String {
        let mut parts = self.segments.clone();
        if !self.current.trim().is_empty() {
            parts.push(self.current.clone());
        }
        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_the_text_after_the_last_tool_call() {
        let mut transcript = TurnTranscript::new();
        transcript.push_text("Let me check the file.");
        transcript.end_segment();
        transcript.push_text("Done: the config is correct.");
        assert_eq!(
            transcript.report().as_deref(),
            Some("Done: the config is correct.")
        );
    }

    #[test]
    fn report_falls_back_to_the_last_non_empty_segment() {
        let mut transcript = TurnTranscript::new();
        transcript.push_text("First finding.");
        transcript.end_segment();
        transcript.end_segment();
        assert_eq!(transcript.report().as_deref(), Some("First finding."));
    }

    #[test]
    fn empty_transcript_has_no_report() {
        let mut transcript = TurnTranscript::new();
        transcript.push_text("   ");
        transcript.end_segment();
        assert_eq!(transcript.report(), None);
    }

    #[test]
    fn narration_is_kept_out_of_the_report_but_available() {
        let mut transcript = TurnTranscript::new();
        transcript.push_text("Let me look.");
        transcript.end_segment();
        transcript.push_text("The answer is 4.");
        let report = transcript.report().unwrap();
        assert!(!report.contains("Let me look"));
        assert!(transcript.all_text().contains("Let me look"));
    }

    #[test]
    fn outcome_helpers() {
        assert!(TurnOutcome::Completed.is_ok());
        assert!(!TurnOutcome::Cancelled.is_ok());
        assert!(!TurnOutcome::Failed("x".into()).is_ok());
    }
}
