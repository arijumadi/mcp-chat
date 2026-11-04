//! The `question` tool.

use super::{Rendered, ToolExecRequest, ToolExecutor};
use crate::agent::approvals::InputAnswer;
use crate::events::CoreEvent;
use crate::{CoreError, Result};
use serde_json::{Value, json};

impl ToolExecutor {
    pub(super) async fn run_question(&self, req: &ToolExecRequest) -> Result<Rendered> {
        let questions = req
            .args
            .get("questions")
            .cloned()
            .ok_or_else(|| CoreError::invalid("questions is required"))?;
        let _ = self.events.send(CoreEvent::InputRequest {
            conversation_id: req.conversation_id.clone(),
            call_id: req.call_id.clone(),
            request: json!({"questions": questions}),
        });
        let receiver = self.inputs.register(&req.call_id);
        let answer = tokio::select! {
            _ = req.cancel.cancelled() => {
                self.inputs.cancel(&req.call_id);
                return Err(CoreError::Cancelled);
            }
            answer = receiver => answer.ok(),
        };
        let Some(InputAnswer {
            responses,
            cancelled,
        }) = answer
        else {
            return Err(CoreError::Message(
                "The question was closed without an answer.".into(),
            ));
        };
        if cancelled {
            return Err(CoreError::Message(
                "The user cancelled the question.".into(),
            ));
        }
        let answers = responses.get("answers").cloned().unwrap_or(responses);
        Ok(Rendered {
            content: render_answers(&answers),
            structured: Some(answers),
            ok: true,
        })
    }
}

/// The text the model reads back: one `- header: answer` line per question,
/// followed by the question in parentheses for context.
fn render_answers(answers: &Value) -> String {
    let mut content = String::from("The user answered:\n");
    if let Some(items) = answers.as_array() {
        for item in items {
            let header = item
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or("Question");
            let question = item.get("question").and_then(Value::as_str).unwrap_or("");
            let answer = answer_text(item.get("answer"));
            content.push_str(&format!("- {header}: {answer}\n"));
            if !question.is_empty() {
                content.push_str(&format!("  ({question})\n"));
            }
        }
    } else {
        content.push_str(&serde_json::to_string_pretty(answers).unwrap_or_default());
        content.push('\n');
    }
    content
}

/// A string answer is shown as typed. Any other JSON value (an array of picked
/// options, a boolean, a number) is shown as compact JSON instead of vanishing.
/// A missing or null answer stays empty.
fn answer_text(answer: Option<&Value>) -> String {
    match answer {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_string_answer_is_shown_as_typed() {
        let answers = json!([{
            "header": "Color",
            "question": "Which color should I use?",
            "answer": "Blue"
        }]);
        assert_eq!(
            render_answers(&answers),
            "The user answered:\n- Color: Blue\n  (Which color should I use?)\n"
        );
    }

    #[test]
    fn answers_that_are_not_strings_are_not_dropped() {
        let answers = json!([
            {"header": "Formats", "question": "Which formats?", "answer": ["Markdown", "JSON"]},
            {"header": "Confirm", "question": "Delete the files?", "answer": true},
            {"header": "Count", "question": "How many?", "answer": 3},
            {"header": "Extra", "question": "Anything else?", "answer": {"note": "no"}},
        ]);
        assert_eq!(
            render_answers(&answers),
            "The user answered:\n\
             - Formats: [\"Markdown\",\"JSON\"]\n  (Which formats?)\n\
             - Confirm: true\n  (Delete the files?)\n\
             - Count: 3\n  (How many?)\n\
             - Extra: {\"note\":\"no\"}\n  (Anything else?)\n"
        );
    }

    #[test]
    fn a_missing_answer_stays_empty_and_a_missing_question_is_omitted() {
        let answers = json!([{"header": "Name"}, {"header": "Nick", "answer": null}]);
        assert_eq!(
            render_answers(&answers),
            "The user answered:\n- Name: \n- Nick: \n"
        );
    }

    #[test]
    fn an_unexpected_shape_is_printed_instead_of_lost() {
        let answers = json!({"color": "blue"});
        assert_eq!(
            render_answers(&answers),
            "The user answered:\n{\n  \"color\": \"blue\"\n}\n"
        );
    }
}
