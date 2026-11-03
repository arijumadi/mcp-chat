/**
 * Keyboard shortcuts are written as strings such as "Mod+Shift+P". `Mod` is
 * Ctrl on Windows and Linux and Cmd on macOS.
 */

export interface Chord {
  mod: boolean;
  shift: boolean;
  alt: boolean;
  /** `KeyboardEvent.key`, lowercased for single characters. */
  key: string;
}

const NAMED_KEYS: Record<string, string> = {
  esc: "escape",
  escape: "escape",
  enter: "enter",
  return: "enter",
  backspace: "backspace",
  tab: "tab",
  space: " ",
};

export function parseChord(text: string): Chord {
  const chord: Chord = { mod: false, shift: false, alt: false, key: "" };
  for (const part of text.split("+").map((item) => item.trim().toLowerCase())) {
    if (part === "mod" || part === "ctrl" || part === "cmd") chord.mod = true;
    else if (part === "shift") chord.shift = true;
    else if (part === "alt" || part === "option") chord.alt = true;
    else chord.key = NAMED_KEYS[part] ?? part;
  }
  if (!chord.key) throw new Error(`Shortcut has no key: ${text}`);
  return chord;
}

export function isMac(platform: string = navigator.platform ?? ""): boolean {
  return /mac|iphone|ipad/i.test(platform);
}

/**
 * Whether a keydown is exactly this chord (no extra modifiers). `Mod` is Cmd on
 * macOS and Ctrl elsewhere. On macOS Ctrl+B, Ctrl+N and Ctrl+K are cursor
 * movement in text fields, so they must not trigger Mod chords there.
 */
export function matchesChord(
  event: KeyboardEvent,
  chord: Chord,
  mac: boolean = isMac(),
): boolean {
  const mod = mac ? event.metaKey : event.ctrlKey;
  const otherMod = mac ? event.ctrlKey : event.metaKey;
  if (otherMod || mod !== chord.mod) return false;
  if (event.shiftKey !== chord.shift || event.altKey !== chord.alt) return false;
  return event.key.toLowerCase() === chord.key;
}

const MAC_SYMBOLS: Record<string, string> = {
  escape: "Esc",
  enter: "↩",
  backspace: "⌫",
  tab: "⇥",
  " ": "Space",
};

/** How a shortcut reads on this platform: "Ctrl+Shift+P" or "⌘⇧P". */
export function formatShortcut(text: string, mac: boolean = isMac()): string {
  const chord = parseChord(text);
  const key =
    chord.key.length === 1
      ? chord.key.toUpperCase()
      : (mac ? MAC_SYMBOLS[chord.key] : undefined) ??
        chord.key.charAt(0).toUpperCase() + chord.key.slice(1);
  if (mac) {
    return `${chord.alt ? "⌥" : ""}${chord.shift ? "⇧" : ""}${chord.mod ? "⌘" : ""}${key}`;
  }
  return [chord.mod && "Ctrl", chord.alt && "Alt", chord.shift && "Shift", key]
    .filter(Boolean)
    .join("+");
}

/** True for elements where typing should not trigger plain shortcuts. */
export function isEditable(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

/** Whether a shortcut may fire from this event target, given its typing rule. */
export function allowedFromTarget(
  target: EventTarget | null,
  rule: boolean | string | undefined,
): boolean {
  if (!isEditable(target)) return true;
  if (rule === true) return true;
  if (typeof rule === "string") return target instanceof Element && target.matches(rule);
  return false;
}

/** The value of `aria-keyshortcuts` for a shortcut: "Control+Shift+Enter". */
export function ariaKeyShortcuts(text: string, mac: boolean = isMac()): string {
  const chord = parseChord(text);
  const key =
    chord.key === "escape"
      ? "Escape"
      : chord.key.length === 1
        ? chord.key.toUpperCase()
        : chord.key.charAt(0).toUpperCase() + chord.key.slice(1);
  return [chord.mod && (mac ? "Meta" : "Control"), chord.alt && "Alt", chord.shift && "Shift", key]
    .filter(Boolean)
    .join("+");
}
