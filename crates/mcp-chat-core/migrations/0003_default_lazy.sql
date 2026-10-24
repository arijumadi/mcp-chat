-- New conversations default to lazy MCP tool delivery. Roll the just-introduced
-- feature forward for existing conversations too, so the default is consistent.
UPDATE conversations SET tool_mode = 'lazy';
