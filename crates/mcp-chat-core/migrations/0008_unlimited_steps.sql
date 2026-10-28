-- Step limits are opt-in now. Existing conversations and new chats have no
-- hard 25-step cap unless a caller sets max_steps explicitly.
UPDATE conversations SET max_steps = 0 WHERE max_steps = 25;
