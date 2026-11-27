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

const splitOptions: AgentConfigOption[] = [
  {
    id: "model",
    name: "Model",
    category: "model",
    currentValue: "gemini-4-hydrogen",
    kind: "select",
    choices: [{ value: "gemini-4-hydrogen", name: "Gemini 4 Hydrogen" }],
  },
  {
    // The id differs between agents; only the category is stable.
    id: "effort",
    name: "Effort",
    category: "thought_level",
    currentValue: "high",
    kind: "select",
    choices: [
      { value: "low", name: "Low" },
      { value: "high", name: "High" },
    ],
  },
  {
    id: "mode",
    name: "Session Mode",
    category: "mode",
    currentValue: "build",
    kind: "select",
    choices: [{ value: "build", name: "Build" }],
  },
];

describe("AgentConfigPicker category split", () => {
  it("keeps the model and thinking level when asked by category", () => {
    useApp.setState({ agentConfig: splitOptions, setAgentConfig: vi.fn() });
    render(
      <AgentConfigPicker
        conversation={conversation}
        categories={["model", "thought_level"]}
      />,
    );
    expect(screen.getByText("Gemini 4 Hydrogen")).toBeInTheDocument();
    expect(screen.getByText("High")).toBeInTheDocument();
    expect(screen.queryByText("Build")).not.toBeInTheDocument();
  });

  it("shows only what is left when those categories are excluded", () => {
    useApp.setState({ agentConfig: splitOptions, setAgentConfig: vi.fn() });
    render(
      <AgentConfigPicker
        conversation={conversation}
        withoutCategories={["model", "thought_level"]}
      />,
    );
    expect(screen.getByText("Build")).toBeInTheDocument();
    expect(screen.queryByText("Gemini 4 Hydrogen")).not.toBeInTheDocument();
    expect(screen.queryByText("High")).not.toBeInTheDocument();
  });

  it("falls back to the id when an agent sends no category", () => {
    useApp.setState({ agentConfig: options, setAgentConfig: vi.fn() });
    render(<AgentConfigPicker conversation={conversation} categories={["model"]} />);
    expect(screen.getByText("Gemini 4 Hydrogen")).toBeInTheDocument();
  });
});
