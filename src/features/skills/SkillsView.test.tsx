import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { SkillsView } from "./SkillsView";
import { useApp } from "@/app/store";
import type { Conversation, SkillView } from "@/lib/types";

const skills: SkillView[] = [
  {
    name: "excalidraw",
    description: "Create hand-drawn diagrams.",
    path: "/home/you/.config/mcp-chat/skills/excalidraw/SKILL.md",
    source: "global",
    enabled: true,
  },
  {
    name: "spreadsheet",
    description: "Read and edit Excel workbooks.",
    path: "/home/you/.config/mcp-chat/skills/spreadsheet/SKILL.md",
    source: "global",
    enabled: true,
  },
];
const saved: Array<{ name: string; enabled: boolean }> = [];

const conversation = {
  id: "c1",
  title: "Test",
  createdAt: 0,
  updatedAt: 0,
  providerId: null,
  model: null,
  workspaceRoot: "/home/you/project",
  systemPrompt: null,
  archived: false,
  pinned: false,
  maxSteps: 25,
  toolMode: "lazy",
  serversConfigured: false,
} satisfies Conversation;

beforeEach(() => {
  saved.length = 0;
  window.__MCP_CHAT_MOCK__ = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
      if (command === "list_skills") return skills as T;
      if (command === "set_skill_enabled") {
        saved.push({
          name: String(args?.name),
          enabled: Boolean(args?.enabled),
        });
        return null as T;
      }
      return null as T;
    },
    subscribe: () => () => undefined,
  };
  useApp.setState({
    activeId: "c1",
    conversations: [conversation],
    appInfo: {
      name: "Jarvis",
      version: "0.1.0-test",
      dataDir: "/home/you/.local/share/com.mcpchat.app",
      workspaceDir: "/home/you/project",
      platform: "linux",
      autoUpdate: false,
    },
    toasts: [],
  });
});

afterEach(() => {
  cleanup();
  delete window.__MCP_CHAT_MOCK__;
});

describe("SkillsView", () => {
  it("lists available skills with their source and description", async () => {
    render(<SkillsView />);
    expect(await screen.findByText("excalidraw")).toBeInTheDocument();
    expect(screen.getByText("spreadsheet")).toBeInTheDocument();
    expect(screen.getByText("Create hand-drawn diagrams.")).toBeInTheDocument();
    expect(screen.getAllByText("Global").length).toBe(2);
    expect(screen.getByText("2 of 2 enabled")).toBeInTheDocument();
  });

  it("disables a skill and persists the change", async () => {
    render(<SkillsView />);
    const toggle = await screen.findByRole("switch", { name: "Enable excalidraw" });
    expect(toggle).toHaveAttribute("aria-checked", "true");

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await waitFor(() => expect(saved).toEqual([{ name: "excalidraw", enabled: false }]));
    expect(screen.getByText("1 of 2 enabled")).toBeInTheDocument();
    expect(useApp.getState().toasts.at(-1)?.text).toMatch(/disabled for all chats/);
  });
});
