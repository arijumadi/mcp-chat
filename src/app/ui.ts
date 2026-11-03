import { create } from "zustand";

/** Which page of the command palette is showing. */
export type PalettePage = "root" | "models" | "workspaces";

interface UiState {
  /** The conversation search (Ctrl+K). */
  quickOpen: boolean;
  /** The command palette; null when closed. */
  palette: { page: PalettePage; query: string } | null;
  openQuick: () => void;
  closeQuick: () => void;
  openPalette: (page?: PalettePage, query?: string) => void;
  closePalette: () => void;
}

/**
 * Transient overlays. They live apart from the main store because several
 * unrelated places (shortcuts, the sidebar, the palette itself) open them.
 */
export const useUi = create<UiState>((set) => ({
  quickOpen: false,
  palette: null,
  // Only one overlay at a time.
  openQuick: () => set({ quickOpen: true, palette: null }),
  closeQuick: () => set({ quickOpen: false }),
  openPalette: (page = "root", query = "") =>
    set({ palette: { page, query }, quickOpen: false }),
  closePalette: () => set({ palette: null }),
}));
