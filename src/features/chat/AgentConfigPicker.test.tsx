import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AgentConfigPicker } from "./AgentConfigPicker";
import { useApp } from "@/app/store";
import type { AgentConfigOption, Conversation } from "@/lib/types";

afterEach(cleanup);

const conversation = {
  id: "c1",
  acpAgent: "antigravity",
} as unknown as Conversation;

const options: AgentConfigOption[] = [
  {
    id: "model",
    name: "Model",
    currentValue: "gemini-4-hydrogen",
    kind: "select",
    choices: [
      { value: "gemini-4-hydrogen", name: "Gemini 4 Hydrogen" },
      { value: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
    ],
  },
];

describe("AgentConfigPicker", () => {
  it("shows the agent's current model", () => {
    useApp.setState({ agentConfig: options, setAgentConfig: vi.fn() });
    render(<AgentConfigPicker conversation={conversation} />);
    expect(screen.getByText("Gemini 4 Hydrogen")).toBeInTheDocument();
  });

  it("sends the chosen value to the agent", () => {
    const setAgentConfig = vi.fn().mockResolvedValue(undefined);
    useApp.setState({ agentConfig: options, setAgentConfig });
    render(<AgentConfigPicker conversation={conversation} />);
    fireEvent.click(screen.getByRole("button", { name: /Model/ }));
    fireEvent.click(screen.getByText("Claude Sonnet 4.6"));
    expect(setAgentConfig).toHaveBeenCalledWith("c1", "model", "claude-sonnet-4-6");
  });

  it("renders nothing when the agent exposes no options", () => {
    useApp.setState({ agentConfig: [], setAgentConfig: vi.fn() });
    const { container } = render(<AgentConfigPicker conversation={conversation} />);
    expect(container).toBeEmptyDOMElement();
  });
});
