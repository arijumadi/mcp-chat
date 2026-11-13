---
name: manage-skill
description: "Create, update, validate, and organize global Jarvis skills that are available in every workspace."
version: 1.0.0
---

# Manage Jarvis skills

Use this skill when the user wants to add, change, or remove a reusable
workflow, or asks to turn instructions into a skill. A skill is a directory
containing `SKILL.md`; it can also contain `references/` and `scripts/`.

## Where skills live

The runtime context prints the exact **Global skills directory** to use. It is
normally:

- Linux/macOS: `~/.config/mcp-chat/skills`
- Windows: `%USERPROFILE%\.config\mcp-chat\skills`

Each skill is a direct child directory:

```text
<global-skills>/my-skill/SKILL.md
<global-skills>/my-skill/references/details.md
<global-skills>/my-skill/scripts/helper.py
```

`write` creates parent directories. Global paths are outside the workspace, so
the first write shows an approval prompt. That is expected: explain briefly why
Jarvis needs to write to the global skills directory. Never work around the
approval.

Workspace skills under `.mcp-chat/skills/` or `.opencode/skills/` take
precedence over global skills with the same name. The app-data `skills` folder
also takes precedence. If a newly created skill does not appear, check for a
shadowing skill with the same name.

## Create a skill

1. Pick a short lowercase name using letters, digits, `-`, or `_` only.
2. Check for a collision with the `skill` tool: call it with an empty `name` to
   list the catalog, then call it with the proposed name if it already exists.
3. Write a concise `SKILL.md`:

```markdown
---
name: my-skill
description: "One sentence describing when the model should load this skill."
---

# My skill

Short instructions, workflow, and safety notes.
```

4. Put long reference material in `references/` and reusable code in `scripts/`
   rather than making `SKILL.md` enormous.
5. Create the file with `write(filePath="<global-skills>/my-skill/SKILL.md",
   content="...")`. Add helper files the same way.
6. Verify with the `skill` tool: load `my-skill`, then load a reference file.
   The **Skills** page lists the effective catalog and its enabled state; use
   it to confirm the new entry or refresh after adding files.

The `description` is what appears in the system prompt, so make it specific:
say what the skill does and when to use it. Keep it to one line.

## Update a skill

1. Read the current `SKILL.md` with the `skill` tool before changing it.
2. Preserve its `name` and frontmatter. Keep unrelated sections and user edits.
3. For a targeted change, use `edit` or `apply_patch`.
4. For a full rewrite, use `write` with `overwrite: true`.
5. Update `references/` or `scripts/` when the instructions depend on them.
6. Load the skill again and check that the new description and content work.

If a file does not exist, `write` creates it. If it exists, `edit` or
`apply_patch` is safer than replacing the whole file.

## Rules

- Do not put API keys, tokens, passwords, or private data in a skill.
- Do not invent tool names or paths. Use tools that exist in Jarvis (`write`,
  `read`, `edit`, `apply_patch`, `bash`, `python`, `skill`).
- Skills are instructions, not a sandbox. Keep destructive or network actions
  explicit and ask before running them.
- For Python helpers, prefer the standard library and document any dependency
  in `SKILL.md`. Test the script before declaring success.
- Editing a bundled skill is allowed, but Jarvis stops auto-updating a file
  once it detects local changes. If the user wants to keep receiving bundled
  updates, copy the skill to a new name instead.
- Delete or replace a skill only after explicit confirmation. Prefer archiving
  it by moving it to a clearly named folder under the global skills directory.

## Report

After a change, tell the user:

- the skill name and absolute `SKILL.md` path;
- whether it was created, updated, or only inspected;
- any helper files added;
- the verification result from the `skill` tool.
