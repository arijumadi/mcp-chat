-- Tool delivery is lazy-only in the UI. Normalize every existing chat so a
-- previously selected direct-schema mode cannot linger after the upgrade.
UPDATE conversations SET tool_mode = 'lazy';
