# Jarvis PDF skill

Jarvis installs this skill automatically into the global skills directory on startup. The manual steps below are for older builds or a custom installation. The installer does not overwrite a file you have edited.

Requires Python 3.10+ with the core PDF libraries:

```bash
python -m pip install pypdf reportlab pdfplumber
```

Optional:

- `pypdfium2` or poppler's `pdftoppm` for page rasterization and form overlays.
- `pymupdf` and `pymupdf4llm` for the lightweight extraction path.
- `marker-pdf` for OCR and complex layouts (large download).
- LibreOffice (`soffice`) for format conversion.

Global install path:

```text
~/.config/mcp-chat/skills/pdf
```

On Windows:

```text
%USERPROFILE%\.config\mcp-chat\skills\pdf
```

Workspace-only:

```text
<workspace>/.mcp-chat/skills/pdf
```

Jarvis reads only the skill name and description in the prompt. The full `SKILL.md` and scripts load on demand through the `skill` tool.
