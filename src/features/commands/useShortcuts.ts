import { useEffect } from "react";
import { useApp } from "@/app/store";
import type { AppStore } from "@/app/store";
import { useUi } from "@/app/ui";
import { COMMANDS, type Command } from "./registry";
import { allowedFromTarget, matchesChord, parseChord } from "./shortcuts";

/** The overlays that own the keyboard while they are open. */
export interface OverlayState {
  quickOpen: boolean;
  paletteOpen: boolean;
  compactOpen: boolean;
}

/**
 * Which command a keydown triggers, or null. Pure, so the rules are testable:
 * - nothing fires while an overlay is open (it handles its own keys)
 * - nothing fires on key repeat or during IME composition
 * - a shortcut fires from a text field only if its command allows it
 */
export function resolveShortcut(event: KeyboardEvent, overlays: OverlayState): Command | null {
  if (event.repeat || event.isComposing) return null;
  if (overlays.quickOpen || overlays.paletteOpen || overlays.compactOpen) return null;
  for (const command of COMMANDS) {
    if (!command.shortcut) continue;
    if (!matchesChord(event, parseChord(command.shortcut))) continue;
    return allowedFromTarget(event.target, command.worksWhileTyping) ? command : null;
  }
  return null;
}

/**
 * Handle one keydown. Returns true when it ran a command. A disabled command
 * does nothing and leaves the key alone, so Ctrl+Backspace still deletes a word
 * when no approval is waiting.
 */
export function handleShortcut(
  event: KeyboardEvent,
  state: AppStore,
  overlays: OverlayState,
): boolean {
  const command = resolveShortcut(event, overlays);
  if (!command) return false;
  const reason = command.disabled(state);
  if (reason) {
    if (!command.silentWhenDisabled) {
      event.preventDefault();
      state.toast(reason, "info");
    }
    return false;
  }
  event.preventDefault();
  void command.run(state);
  return true;
}

/** Global keyboard shortcuts, built from the command registry. */
export function useGlobalShortcuts() {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const ui = useUi.getState();
      handleShortcut(event, useApp.getState(), {
        quickOpen: ui.quickOpen,
        paletteOpen: ui.palette !== null,
        compactOpen: useApp.getState().compactDialog !== null,
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
