-- Per-conversation reasoning effort. `default` means the provider default and
-- sends no extra parameter.
ALTER TABLE conversations ADD COLUMN reasoning_effort TEXT NOT NULL DEFAULT 'default';
