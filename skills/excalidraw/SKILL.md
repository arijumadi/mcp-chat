---
name: excalidraw
description: "Create hand-drawn Excalidraw JSON diagrams for architecture, flowcharts, sequence diagrams, and concept maps."
version: 1.0.1
author: Hermes Agent
license: MIT
tags: [Excalidraw, Diagrams, Flowcharts, Architecture, Visualization, JSON]
---

# Excalidraw Diagram Skill

Create diagrams by writing standard Excalidraw element JSON and saving it as a
`.excalidraw` file. The file can be opened at [excalidraw.com](https://excalidraw.com)
or in any Excalidraw-compatible editor. No account, API key, or rendering
library is needed -- just JSON.

## When to use

Generate `.excalidraw` files for architecture diagrams, flowcharts, sequence
diagrams, concept maps, and similar visuals. Use this for a file the user can
edit later; use a rendered image only when the user explicitly wants a static
picture.

## Workflow

1. **Load this skill** (you already did).
2. **Design the diagram** -- choose the layout, colors, and element sizes first.
3. **Write the elements JSON** -- an array of Excalidraw element objects.
4. **Save the file** with the `write` tool. Put it in the workspace, or in
   `.mcp-chat/artifacts/` when it is a generated deliverable.
5. **Optionally upload** for a shareable link with `scripts/upload.py`, but only
   when the user asks for a link.

### Saving a diagram

Wrap the elements array in the standard `.excalidraw` envelope:

```json
{
  "type": "excalidraw",
  "version": 2,
  "source": "jarvis",
  "elements": [ ...your elements array here... ],
  "appState": {
    "viewBackgroundColor": "#ffffff"
  }
}
```

Use `write` with `filePath` and the complete envelope as `content`. `write`
creates parent directories. It rejects an existing file by default; read the
existing diagram first, then use `write` with `overwrite: true` (or
`apply_patch` for a targeted edit) when replacing it.

Save to a path such as `diagrams/architecture.excalidraw`.

### Uploading for a shareable link

The upload script is inside this skill's `scripts/` directory. It uploads the
diagram encrypted to excalidraw.com and prints a URL whose fragment contains
the decryption key. Treat the link as a secret: anyone with it can read the
diagram.

Use the `python` tool to locate the skill and run the script:

```python
import os, subprocess, sys
from pathlib import Path

roots = [
    Path(".mcp-chat/skills/excalidraw"),
    Path(os.environ.get("USERPROFILE", "")) / ".config/mcp-chat/skills/excalidraw",
    Path.home() / ".config/mcp-chat/skills/excalidraw",
]
skill = next((p for p in roots if (p / "scripts/upload.py").is_file()), None)
assert skill, "excalidraw skill scripts not found"
diagram = Path("diagrams/architecture.excalidraw").resolve()
subprocess.run([sys.executable, str(skill / "scripts/upload.py"), str(diagram)], check=True)
```

This requires the `cryptography` package. Install it with
`python -m pip install cryptography` only after the user agrees. Do not upload
a diagram automatically; ask first because it sends the encrypted file to a
third-party service.

---

## Element Format Reference

### Required Fields (all elements)
`type`, `id` (unique string), `x`, `y`, `width`, `height`

### Defaults (skip these -- they're applied automatically)
- `strokeColor`: `"#1e1e1e"`
- `backgroundColor`: `"transparent"`
- `fillStyle`: `"solid"`
- `strokeWidth`: `2`
- `roughness`: `1` (hand-drawn look)
- `opacity`: `100`

Canvas background is white.

### Element Types

**Rectangle**:
```json
{ "type": "rectangle", "id": "r1", "x": 100, "y": 100, "width": 200, "height": 100 }
```
- `roundness: { "type": 3 }` for rounded corners
- `backgroundColor: "#a5d8ff"`, `fillStyle: "solid"` for filled

**Ellipse**:
```json
{ "type": "ellipse", "id": "e1", "x": 100, "y": 100, "width": 150, "height": 150 }
```

**Diamond**:
```json
{ "type": "diamond", "id": "d1", "x": 100, "y": 100, "width": 150, "height": 150 }
```

**Labeled shape (container binding)** -- create a text element bound to the shape:

> **WARNING:** Do NOT use `"label": { "text": "..." }` on shapes. This is NOT a valid
> Excalidraw property and will be silently ignored, producing blank shapes. You MUST
> use the container binding approach below.

The shape needs `boundElements` listing the text, and the text needs `containerId` pointing back:
```json
{ "type": "rectangle", "id": "r1", "x": 100, "y": 100, "width": 200, "height": 80,
  "roundness": { "type": 3 }, "backgroundColor": "#a5d8ff", "fillStyle": "solid",
  "boundElements": [{ "id": "t_r1", "type": "text" }] },
{ "type": "text", "id": "t_r1", "x": 105, "y": 110, "width": 190, "height": 25,
  "text": "Hello", "fontSize": 20, "fontFamily": 1, "strokeColor": "#1e1e1e",
  "textAlign": "center", "verticalAlign": "middle",
  "containerId": "r1", "originalText": "Hello", "autoResize": true }
```
- Works on rectangle, ellipse, diamond
- Text is auto-centered by Excalidraw when `containerId` is set
- The text `x`/`y`/`width`/`height` are approximate -- Excalidraw recalculates them on load
- `originalText` should match `text`
- Always include `fontFamily: 1` (Virgil/hand-drawn font)

**Labeled arrow** -- same container binding approach:
```json
{ "type": "arrow", "id": "a1", "x": 300, "y": 150, "width": 200, "height": 0,
  "points": [[0,0],[200,0]], "endArrowhead": "arrow",
  "boundElements": [{ "id": "t_a1", "type": "text" }] },
{ "type": "text", "id": "t_a1", "x": 370, "y": 130, "width": 60, "height": 20,
  "text": "connects", "fontSize": 16, "fontFamily": 1, "strokeColor": "#1e1e1e",
  "textAlign": "center", "verticalAlign": "middle",
  "containerId": "a1", "originalText": "connects", "autoResize": true }
```

**Standalone text** (titles and annotations only -- no container):
```json
{ "type": "text", "id": "t1", "x": 150, "y": 138, "text": "Hello", "fontSize": 20,
  "fontFamily": 1, "strokeColor": "#1e1e1e", "originalText": "Hello", "autoResize": true }
```
- `x` is the LEFT edge. To center at position `cx`: `x = cx - (text.length * fontSize * 0.5) / 2`
- Do NOT rely on `textAlign` or `width` for positioning

**Arrow**:
```json
{ "type": "arrow", "id": "a1", "x": 300, "y": 150, "width": 200, "height": 0,
  "points": [[0,0],[200,0]], "endArrowhead": "arrow" }
```
- `points`: `[dx, dy]` offsets from element `x`, `y`
- `endArrowhead`: `null` | `"arrow"` | `"bar"` | `"dot"` | `"triangle"`
- `strokeStyle`: `"solid"` (default) | `"dashed"` | `"dotted"`

### Arrow Bindings (connect arrows to shapes)

```json
{
  "type": "arrow", "id": "a1", "x": 300, "y": 150, "width": 150, "height": 0,
  "points": [[0,0],[150,0]], "endArrowhead": "arrow",
  "startBinding": { "elementId": "r1", "fixedPoint": [1, 0.5] },
  "endBinding": { "elementId": "r2", "fixedPoint": [0, 0.5] }
}
```

`fixedPoint` coordinates: `top=[0.5,0]`, `bottom=[0.5,1]`, `left=[0,0.5]`, `right=[1,0.5]`

### Drawing Order (z-order)
- Array order = z-order (first = back, last = front)
- Emit progressively: background zones → shape → its bound text → its arrows → next shape
- BAD: all rectangles, then all texts, then all arrows
- GOOD: bg_zone → shape1 → text_for_shape1 → arrow1 → arrow_label_text → shape2 → text_for_shape2 → ...
- Always place the bound text element immediately after its container shape

### Sizing Guidelines

**Font sizes:**
- Minimum `fontSize`: **16** for body text, labels, descriptions
- Minimum `fontSize`: **20** for titles and headings
- Minimum `fontSize`: **14** for secondary annotations only (sparingly)
- NEVER use `fontSize` below 14

**Element sizes:**
- Minimum shape size: 120x60 for labeled rectangles/ellipses
- Leave 20-30px gaps between elements minimum
- Prefer fewer, larger elements over many tiny ones

### Color Palette

See `references/colors.md` for full color tables. Quick reference:

| Use | Fill Color | Hex |
|-----|-----------|-----|
| Primary / Input | Light Blue | `#a5d8ff` |
| Success / Output | Light Green | `#b2f2bb` |
| Warning / External | Light Orange | `#ffd8a8` |
| Processing / Special | Light Purple | `#d0bfff` |
| Error / Critical | Light Red | `#ffc9c9` |
| Notes / Decisions | Light Yellow | `#fff3bf` |
| Storage / Data | Light Teal | `#c3fae8` |

### Tips
- Use the color palette consistently across the diagram
- **Text contrast is CRITICAL** -- never use light gray on white backgrounds. Minimum text color on white: `#757575`
- Do NOT use emoji in text -- they don't render in Excalidraw's font
- For dark mode diagrams, see `references/dark-mode.md`
- For larger examples, see `references/examples.md`
