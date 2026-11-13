-- Child conversations created by the subagent tool. They are hidden from the
-- main conversation list and cascade when their parent is deleted.
ALTER TABLE conversations ADD COLUMN parent_id TEXT REFERENCES conversations(id) ON DELETE CASCADE;
CREATE INDEX idx_conversations_parent ON conversations(parent_id);
