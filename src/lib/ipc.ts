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

/**
 * True inside the desktop shell.
 *
 * This check comes before the HTTP fallback on purpose: the desktop must keep
 * using in-process IPC, never a localhost server. `__TAURI_INTERNALS__` is
 * injected by Tauri before any script runs, so it is reliable at call time
 * (tests swap it in and out, which is why it is not cached at module load).
 */
function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (window.__MCP_CHAT_MOCK__) {
    return window.__MCP_CHAT_MOCK__.invoke<T>(command, args);
  }
  if (!isTauri()) return httpCall<T>(command, args);
  return invoke<T>(command, args);
}

/**
 * The browser is the UI in server mode, so it opens links itself. Doing it here
 * means it happens instantly and works even when the server is unreachable.
 */
function openLocally(command: string, args?: Record<string, unknown>): boolean {
  if (command !== "open_external") return false;
  const url = typeof args?.url === "string" ? args.url : "";
  if (url) window.open(url, "_blank", "noopener,noreferrer");
  return true;
}

async function httpCall<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (openLocally(command, args)) return undefined as T;
  const response = await fetch("/api/invoke", {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify({ command, args: args ?? {} }),
  });
  const text = await response.text();
  if (!response.ok) {
    // Tauri rejects with the error string itself and the UI renders
    // `String(error)`. Throw the same shape so both transports read alike.
    throw text || `Request failed (${response.status})`;
  }
  return (text ? JSON.parse(text) : null) as T;
}

const CHANNELS = ["chat://delta", "chat://tool", "chat://status"] as const;

export async function subscribe(
  handler: (event: CoreEvent) => void,
): Promise<() => void> {
  if (window.__MCP_CHAT_MOCK__) {
    return window.__MCP_CHAT_MOCK__.subscribe(handler);
  }
  if (!isTauri()) return subscribeHttp(handler);
  const unlisteners = await Promise.all(
    CHANNELS.map((channel) =>
      listen<CoreEvent>(channel, (event) => handler(event.payload)),
    ),
  );
  return () => {
    for (const unlisten of unlisteners) unlisten();
  };
}

/**
 * One SSE stream carries every event. The desktop splits them across three
 * channels for throughput; the order here is strictly the order the engine
 * emitted them, which is easier to reason about.
 *
 * `EventSource` reconnects on its own. The server answers a reconnect with a
 * `resync`, so deltas missed while the socket was down are replaced by a
 * reload rather than silently lost.
 */
function subscribeHttp(handler: (event: CoreEvent) => void): () => void {
  const source = new EventSource("/api/events");
  source.onmessage = (event) => {
    try {
      handler(JSON.parse(event.data) as CoreEvent);
    } catch {
      // A malformed frame must not take the stream down.
    }
  };
  return () => source.close();
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
