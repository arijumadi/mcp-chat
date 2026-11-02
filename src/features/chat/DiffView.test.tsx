import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { DiffView } from "./DiffView";

const DIFF = [
  "--- a/src/app.ts",
  "+++ b/src/app.ts",
  "@@ -1,3 +1,3 @@",
  " keep",
  "-old line",
  "+new line",
  "",
].join("\n");

afterEach(cleanup);

describe("DiffView", () => {
  it("is collapsed by default and shows a summary with counts", () => {
    render(<DiffView diff={DIFF} />);
    const toggle = screen.getByRole("button", { name: /Changes/ });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveTextContent("Changes · +1 \u22121 · 1 file");
    expect(screen.queryByText("-old line")).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Changes" })).not.toBeInTheDocument();
  });

  it("renders colored lines per kind after expanding", () => {
    const { container } = render(<DiffView diff={DIFF} />);
    fireEvent.click(screen.getByRole("button", { name: /Changes/ }));

    expect(screen.getByRole("button", { name: /Changes/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("+new line")).toHaveClass("diff-line", "diff-line-added");
    expect(screen.getByText("-old line")).toHaveClass("diff-line", "diff-line-removed");
    expect(screen.getByText("@@ -1,3 +1,3 @@")).toHaveClass("diff-line", "diff-hunk");
    expect(screen.getByText("--- a/src/app.ts")).toHaveClass("diff-line", "diff-file-header");
    expect(screen.getByText("+++ b/src/app.ts")).toHaveClass("diff-file-header");
    expect(screen.getByText("keep")).not.toHaveClass("diff-line-added", "diff-line-removed");
    // The +/- prefixes stay in the text so color is not the only cue.
    expect(container.querySelectorAll(".diff-line-added")).toHaveLength(1);
    expect(container.querySelectorAll(".diff-line-removed")).toHaveLength(1);
  });

  it("collapses again on a second click", () => {
    render(<DiffView diff={DIFF} />);
    const toggle = screen.getByRole("button", { name: /Changes/ });
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("+new line")).not.toBeInTheDocument();
  });

  it("can start open", () => {
    render(<DiffView diff={DIFF} defaultOpen />);
    expect(screen.getByRole("button", { name: /Changes/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByText("+new line")).toBeInTheDocument();
  });

  it("counts every file of a multi-file diff", () => {
    const multi = `${DIFF}--- a/b.txt\n+++ b/b.txt\n@@ -0,0 +1 @@\n+added\n`;
    render(<DiffView diff={multi} />);
    expect(screen.getByRole("button", { name: /Changes/ })).toHaveTextContent(
      "Changes · +2 \u22121 · 2 files",
    );
  });

  it("applies the extra class name and renders nothing for an empty diff", () => {
    const { container, rerender } = render(<DiffView diff={DIFF} className="extra" />);
    expect(container.firstElementChild).toHaveClass("diff-view", "extra");
    rerender(<DiffView diff="" />);
    expect(container).toBeEmptyDOMElement();
  });
});
