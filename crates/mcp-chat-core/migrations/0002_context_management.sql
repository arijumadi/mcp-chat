-- Per-conversation MCP selection and tool delivery mode.
ALTER TABLE conversations ADD COLUMN tool_mode TEXT NOT NULL DEFAULT 'direct';
ALTER TABLE conversations ADD COLUMN servers_configured INTEGER NOT NULL DEFAULT 0;

CREATE TABLE conversation_servers (
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  server_id       TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  PRIMARY KEY (conversation_id, server_id)
);

CREATE INDEX idx_conversation_servers_server ON conversation_servers(server_id);
