-- External ACP agents available as subagent backends, plus the markers that
-- keep an ACP child conversation from being mistaken for an internal one.
--
-- These markers are the only durable truth about a child ACP session: the
-- process itself lives in memory and dies with the app.
CREATE TABLE acp_agents (
  id                TEXT PRIMARY KEY NOT NULL,
  name              TEXT NOT NULL,
  slug              TEXT NOT NULL UNIQUE,
  command           TEXT NOT NULL,
  args_json         TEXT NOT NULL DEFAULT '[]',
  env_json          TEXT NOT NULL DEFAULT '{}',
  -- ask | auto_reads
  permission_policy TEXT NOT NULL DEFAULT 'ask',
  enabled           INTEGER NOT NULL DEFAULT 1,
  last_error        TEXT,
  last_checked_at   INTEGER,
  created_at        INTEGER NOT NULL,
  updated_at        INTEGER NOT NULL
);

CREATE INDEX idx_acp_agents_enabled ON acp_agents(enabled, name);

ALTER TABLE conversations ADD COLUMN acp_agent TEXT;
ALTER TABLE conversations ADD COLUMN acp_session_id TEXT;
