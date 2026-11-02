/**
 * Reading comfort settings: density, chat font size and transcript width.
 *
 * They are stored as plain strings in the generic settings table (`ui.*`) and
 * applied as a `data-density` attribute and CSS variables on the document
 * root. The defaults reproduce the original look exactly.
 */

export type Density = "comfortable" | "compact";
export type ChatWidth = "narrow" | "default" | "wide" | "full";

export interface Appearance {
  density: Density;
  /** Base size of the chat text, in px. Other chat sizes scale from it. */
  fontSize: number;
  chatWidth: ChatWidth;
}

export const FONT_SIZE = { min: 13, max: 19, step: 1, default: 15 } as const;

export const DEFAULT_APPEARANCE: Appearance = {
  density: "comfortable",
  fontSize: FONT_SIZE.default,
  chatWidth: "default",
};

export const DENSITIES: ReadonlyArray<{ value: Density; label: string; hint: string }> = [
  { value: "comfortable", label: "Comfortable", hint: "Roomy spacing" },
  { value: "compact", label: "Compact", hint: "Fits more on screen" },
];

/** `px` is the transcript's maximum width; null means no limit. */
export const WIDTHS: ReadonlyArray<{ value: ChatWidth; label: string; px: number | null }> = [
  { value: "narrow", label: "Narrow", px: 640 },
  { value: "default", label: "Default", px: 760 },
  { value: "wide", label: "Wide", px: 960 },
  { value: "full", label: "Full", px: null },
];

export const SETTING_KEYS = {
  density: "ui.density",
  fontSize: "ui.fontSize",
  chatWidth: "ui.chatWidth",
} as const;

function clampFontSize(value: number): number {
  const rounded = Math.round(value / FONT_SIZE.step) * FONT_SIZE.step;
  return Math.min(FONT_SIZE.max, Math.max(FONT_SIZE.min, rounded));
}

/** Read the appearance from settings, falling back to defaults for anything invalid. */
export function readAppearance(settings: Record<string, string | undefined>): Appearance {
  const density = DENSITIES.find((item) => item.value === settings[SETTING_KEYS.density]);
  const width = WIDTHS.find((item) => item.value === settings[SETTING_KEYS.chatWidth]);
  const raw = settings[SETTING_KEYS.fontSize];
  const parsed = raw === undefined || raw.trim() === "" ? Number.NaN : Number(raw);
  return {
    density: density?.value ?? DEFAULT_APPEARANCE.density,
    fontSize: Number.isFinite(parsed) ? clampFontSize(parsed) : DEFAULT_APPEARANCE.fontSize,
    chatWidth: width?.value ?? DEFAULT_APPEARANCE.chatWidth,
  };
}

/** Settings entries to store for an appearance. */
export function appearanceSettings(appearance: Appearance): Record<string, string> {
  return {
    [SETTING_KEYS.density]: appearance.density,
    [SETTING_KEYS.fontSize]: String(appearance.fontSize),
    [SETTING_KEYS.chatWidth]: appearance.chatWidth,
  };
}

export function widthPx(width: ChatWidth): number | null {
  return WIDTHS.find((item) => item.value === width)?.px ?? null;
}

/** Apply an appearance to the document. */
export function applyAppearance(
  appearance: Appearance,
  root: HTMLElement = document.documentElement,
): void {
  root.dataset.density = appearance.density;
  root.style.setProperty("--chat-fs", `${appearance.fontSize}px`);
  const px = widthPx(appearance.chatWidth);
  root.style.setProperty("--chat-width", px === null ? "100%" : `${px}px`);
}
