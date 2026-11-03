import type { CoreEvent } from "@/lib/types";
import { NoticeLimiter, noticeFor, type Notice } from "./notifications";

/**
 * Sends desktop notifications. The decision of whether and what to send lives
 * in `notifications.ts`; this file only talks to the operating system.
 *
 * Inside the desktop app it uses the Tauri notification plugin. In a browser
 * (the mock used for development and tests) it uses the Web Notification API,
 * and with neither it does nothing.
 */

export type Permission = "granted" | "denied" | "default";

export interface NotifyBackend {
  /** Current permission, without asking. */
  permission: () => Promise<Permission>;
  /** Ask the user. Only call this from a user action or while the app is in front. */
  request: () => Promise<Permission>;
  send: (notice: { title: string; body: string }) => Promise<void> | void;
}

function tauriBackend(): NotifyBackend {
  // Loaded on first use so the browser build never needs the plugin.
  const load = () => import("@tauri-apps/plugin-notification");
  return {
    permission: async () => ((await (await load()).isPermissionGranted()) ? "granted" : "default"),
    request: async () => (await (await load()).requestPermission()) as Permission,
    send: async (notice) => (await load()).sendNotification(notice),
  };
}

function webBackend(): NotifyBackend {
  return {
    permission: async () => Notification.permission as Permission,
    request: async () => (await Notification.requestPermission()) as Permission,
    send: (notice) => {
      new Notification(notice.title, { body: notice.body });
    },
  };
}

function noBackend(): NotifyBackend {
  return {
    permission: async () => "denied",
    request: async () => "denied",
    send: () => undefined,
  };
}

export function detectBackend(): NotifyBackend {
  if (typeof window === "undefined") return noBackend();
  if ("__TAURI_INTERNALS__" in window) return tauriBackend();
  if (typeof Notification !== "undefined") return webBackend();
  return noBackend();
}

let backend: NotifyBackend | null = null;
let limiter = new NoticeLimiter();

function current(): NotifyBackend {
  backend ??= detectBackend();
  return backend;
}

/** For tests: use a fake backend and start with a clean limiter. */
export function setBackendForTests(next: NotifyBackend | null): void {
  backend = next;
  limiter = new NoticeLimiter();
}

/** The window is in front and visible. Read when an event arrives. */
export function isWindowActive(): boolean {
  return document.visibilityState === "visible" && document.hasFocus();
}

/** Notifications are on unless the user turned them off. */
export function notificationsEnabled(settings: Record<string, string | undefined>): boolean {
  return settings["notifications.enabled"] !== "false";
}

export interface NotifyContext {
  enabled: boolean;
  activeId: string | null;
  titleOf: (conversationId: string) => string | undefined;
}

/**
 * Tell the user about an event if they are not looking at it. Never throws and
 * never asks for permission: a prompt triggered by a background event would
 * appear where nobody sees it.
 */
export async function maybeNotify(event: CoreEvent, context: NotifyContext): Promise<void> {
  if (!context.enabled) return;
  const notice = noticeFor(event, {
    windowActive: isWindowActive(),
    activeId: context.activeId,
    titleOf: context.titleOf,
  });
  if (!notice || !limiter.allow(notice)) return;
  try {
    const target = current();
    if ((await target.permission()) !== "granted") return;
    await target.send({ title: notice.title, body: notice.body });
  } catch {
    // A notification that fails must never get in the way of the chat.
  }
}

let primed = false;

/**
 * Ask for permission once, early, while the app is in front, so the system
 * prompt is seen. Called when the user sends a message.
 */
export async function primeNotifications(enabled: boolean): Promise<void> {
  if (primed || !enabled) return;
  primed = true;
  try {
    const target = current();
    if ((await target.permission()) === "default") await target.request();
  } catch {
    // Not available here.
  }
}

export type TestResult = "sent" | "blocked" | "unavailable";

/** The "Send a test notification" button. A user action, so it may ask. */
export async function sendTestNotification(): Promise<TestResult> {
  try {
    const target = current();
    let permission = await target.permission();
    if (permission !== "granted") permission = await target.request();
    if (permission !== "granted") return "blocked";
    await target.send({ title: "Jarvis", body: "Notifications are on." });
    return "sent";
  } catch {
    return "unavailable";
  }
}

/** Test seam: forget that permission was already requested. */
export function resetPrimedForTests(): void {
  primed = false;
}

export type { Notice };
