import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { ModelPicker } from "./ModelPicker";
import { useApp } from "@/app/store";
import type { Conversation, ProviderView } from "@/lib/types";

const provider: ProviderView = {
  id: "p1",
  name: "DeepSeek",
  kind: "openai_compat",
  baseUrl: "https://api.deepseek.com",
  model: "deepseek/deepseek-v4.1-flash",
  apiKeyRef: "p1",
  contextWindow: 128000,
  maxOutputTokens: 8192,
  createdAt: 0,
  updatedAt: 0,
  hasKey: true,
  keyHint: null,
  models: [
    "deepseek/deepseek-v4.1-flash",
    "deepseek/deepseek-v4.1-pro",
  ],
};

const conversation: Conversation = {
  id: "c1",
  title: "Test",
  createdAt: 0,
  updatedAt: 0,
  providerId: "p1",
  model: "deepseek/deepseek-v4.1-flash",
  workspaceRoot: "/tmp",
  systemPrompt: null,
  archived: false,
  pinned: false,
  maxSteps: 25,
  toolMode: "lazy",
  serversConfigured: false,
};

afterEach(() => {
  cleanup();
});

describe("ModelPicker", () => {
  it("shows provider names without kind and filters short model names", () => {
    useApp.setState({ providers: [provider] });
    render(<ModelPicker conversation={conversation} />);

    fireEvent.click(screen.getByRole("button", { name: /deepseek-v4.1-flash/ }));
    expect(screen.getByRole("option", { name: "DeepSeek" })).toBeInTheDocument();
    expect(screen.queryByText(/openai_compat/)).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Search models"), {
      target: { value: "pro" },
    });
    const options = screen.getByTestId("model-options");
    expect(
      within(options).getByRole("button", { name: /deepseek-v4.1-pro/ }),
    ).toBeInTheDocument();
    expect(
      within(options).queryByRole("button", { name: /deepseek-v4.1-flash/ }),
    ).not.toBeInTheDocument();
  });
});
