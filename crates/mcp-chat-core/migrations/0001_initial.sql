-- 0001_initial.sql
PRAGMA foreign_keys = ON;

CREATE TABLE conversations (
  id            TEXT PRIMARY KEY NOT NULL,
  title         TEXT NOT NULL,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  provider_id   TEXT,
  model         TEXT,
  system_prompt TEXT,
  archived      INTEGER NOT NULL DEFAULT 0,
  max_steps     INTEGER NOT NULL DEFAULT 25
);

CREATE TABLE messages (
  id              TEXT PRIMARY KEY NOT NULL,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  role            TEXT NOT NULL,
  created_at      INTEGER NOT NULL,
  status          TEXT NOT NULL DEFAULT 'completed',
  error           TEXT
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id, created_at, id);

CREATE TABLE parts (
  id           TEXT PRIMARY KEY NOT NULL,
  message_id   TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  idx          INTEGER NOT NULL,
  kind         TEXT NOT NULL,
  payload_json TEXT NOT NULL
);
CREATE INDEX idx_parts_message ON parts(message_id, idx);
CREATE UNIQUE INDEX idx_parts_message_idx ON parts(message_id, idx);

CREATE TABLE providers (
  id               TEXT PRIMARY KEY NOT NULL,
  name             TEXT NOT NULL,
  kind             TEXT NOT NULL,
  base_url         TEXT NOT NULL,
  model            TEXT NOT NULL,
  api_key_ref      TEXT,
  context_window   INTEGER NOT NULL DEFAULT 128000,
  max_output_tokens INTEGER NOT NULL DEFAULT 8192,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);

CREATE TABLE servers (
  id                 TEXT PRIMARY KEY NOT NULL,
  name               TEXT NOT NULL,
  slug               TEXT NOT NULL UNIQUE,
  url                TEXT NOT NULL,
  transport          TEXT NOT NULL DEFAULT 'streamable_http',
  auth_kind          TEXT NOT NULL DEFAULT 'none',
  headers_json       TEXT NOT NULL DEFAULT '{}',
  enabled            INTEGER NOT NULL DEFAULT 1,
  policy             TEXT NOT NULL DEFAULT 'ask',
  protocol_version   TEXT,
  created_at         INTEGER NOT NULL,
  last_status        TEXT,
  last_error         TEXT,
  last_checked_at    INTEGER,
  server_info_json   TEXT,
  capabilities_json  TEXT,
  tools_json         TEXT,
  tools_cached_at    INTEGER,
  oauth_issuer       TEXT,
  oauth_client_id    TEXT,
  negotiated_at      INTEGER
);

CREATE TABLE settings (
  key   TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL
);

CREATE TABLE permission_rules (
  id         TEXT PRIMARY KEY NOT NULL,
  server_id  TEXT REFERENCES servers(id) ON DELETE CASCADE,
  action     TEXT NOT NULL,
  resource   TEXT NOT NULL,
  effect     TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_permission_rules_server ON permission_rules(server_id);

CREATE VIRTUAL TABLE messages_fts USING fts5(
  content,
  part_id UNINDEXED,
  conversation_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER parts_ai AFTER INSERT ON parts
WHEN NEW.kind = 'text'
BEGIN
  INSERT INTO messages_fts(rowid, content, part_id, conversation_id)
  VALUES (
    NEW.rowid,
    json_extract(NEW.payload_json, '$.text'),
    NEW.id,
    (SELECT conversation_id FROM messages WHERE id = NEW.message_id)
  );
END;

CREATE TRIGGER parts_ad AFTER DELETE ON parts
WHEN OLD.kind = 'text'
BEGIN
  DELETE FROM messages_fts WHERE rowid = OLD.rowid;
END;

CREATE TRIGGER parts_au AFTER UPDATE ON parts
BEGIN
  DELETE FROM messages_fts WHERE rowid = OLD.rowid;
  INSERT INTO messages_fts(rowid, content, part_id, conversation_id)
  SELECT NEW.rowid, json_extract(NEW.payload_json, '$.text'), NEW.id,
         (SELECT conversation_id FROM messages WHERE id = NEW.message_id)
  WHERE NEW.kind = 'text';
END;
