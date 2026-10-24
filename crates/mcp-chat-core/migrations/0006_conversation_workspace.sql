-- Each conversation can pin its own workspace. Existing chats inherit the
-- previous global setting when one was saved; otherwise they fall back to the
-- process working directory at runtime.
ALTER TABLE conversations ADD COLUMN workspace_root TEXT;

UPDATE conversations
SET workspace_root = (
  SELECT value FROM settings
  WHERE key = 'workspace.root' AND value <> ''
)
WHERE workspace_root IS NULL
  AND EXISTS (
    SELECT 1 FROM settings
    WHERE key = 'workspace.root' AND value <> ''
  );
