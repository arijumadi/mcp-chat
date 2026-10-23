use mcp_chat_core::CoreError;

pub type CmdResult<T> = Result<T, String>;

pub fn cmd_err(error: CoreError) -> String {
    tracing::debug!(error = ?error, "command failed");
    error.user_message()
}
