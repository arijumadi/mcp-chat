-- Per-conversation reasoning effort. `auto` means the provider default and
-- sends no extra parameter.
ALTER TABLE conversations ADD COLUMN reasoning_effort TEXT NOT NULL DEFAULT 'auto';
