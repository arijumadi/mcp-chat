import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { AgentCommandMenu } from "./AgentCommandMenu";
import type { AgentCommand } from "@/lib/types";

afterEach(cleanup);

const commands: AgentCommand[] = [
  { name: "review", description: "Review the changes" },
  { name: "search", description: "Search the codebase", inputHint: "<query>" },
];

describe("AgentCommandMenu", () => {
  it("stays hidden until the user types a slash", () => {
    render(<AgentCommandMenu draft="hello" commands={commands} onPick={vi.fn()} />);
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("filters by the text after the slash", () => {
    render(<AgentCommandMenu draft="/rev" commands={commands} onPick={vi.fn()} />);
    expect(screen.getByText("/review")).toBeInTheDocument();
    expect(screen.queryByText("/search")).not.toBeInTheDocument();
  });

  it("hides once the user is typing arguments", () => {
    render(<AgentCommandMenu draft="/search hello" commands={commands} onPick={vi.fn()} />);
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });

  it("reports the picked command", () => {
    const onPick = vi.fn();
    render(<AgentCommandMenu draft="/sea" commands={commands} onPick={onPick} />);
    fireEvent.mouseDown(screen.getByText("/search"));
    expect(onPick).toHaveBeenCalledWith(commands[1]);
  });

  it("scrolls inside a bounded box instead of growing with the composer", () => {
    const many = Array.from({ length: 25 }, (_, index) => ({
      name: `command-${index}`,
      description: "x",
    }));
    render(<AgentCommandMenu draft="/" commands={many} onPick={vi.fn()} />);
    const listbox = screen.getByRole("listbox");
    // A static list used to push the textarea down and fill the screen.
    expect(listbox.className).toContain("absolute");
    expect(listbox.className).toContain("overflow-y-auto");
    expect(listbox.className).toContain("max-h-");
    expect(screen.getByText("25 commands · run by the agent itself")).toBeInTheDocument();
  });

  it("shows the argument hint so the shape is obvious", () => {
    render(<AgentCommandMenu draft="/" commands={commands} onPick={vi.fn()} />);
    expect(screen.getByText("<query>")).toBeInTheDocument();
  });
});
