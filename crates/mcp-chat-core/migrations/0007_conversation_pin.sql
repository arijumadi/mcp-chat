-- Pinned conversations stay at the top of the sidebar regardless of activity.
ALTER TABLE conversations ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
