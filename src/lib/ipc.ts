import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CoreEvent } from "./types";

export interface MockBridge {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  subscribe(handler: (event: CoreEvent) => void): () => void;
}

declare global {
  interface Window {
    __MCP_CHAT_MOCK__?: MockBridge;
  }
}

export async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (window.__MCP_CHAT_MOCK__) {
    return window.__MCP_CHAT_MOCK__.invoke<T>(command, args);
  }
  return invoke<T>(command, args);
}

const CHANNELS = ["chat://delta", "chat://tool", "chat://status"] as const;

export async function subscribe(
  handler: (event: CoreEvent) => void,
): Promise<() => void> {
  if (window.__MCP_CHAT_MOCK__) {
    return window.__MCP_CHAT_MOCK__.subscribe(handler);
  }
  const unlisteners = await Promise.all(
    CHANNELS.map((channel) =>
      listen<CoreEvent>(channel, (event) => handler(event.payload)),
    ),
  );
  return () => {
    for (const unlisten of unlisteners) unlisten();
  };
}

export function downloadText(filename: string, content: string, mime: string): void {
  const blob = new Blob([content], { type: mime });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.style.display = "none";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
