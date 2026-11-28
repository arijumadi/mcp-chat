import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { useSmoothText } from "./useSmoothText";

afterEach(cleanup);

function Probe({ text }: { text: string }) {
  const shown = useSmoothText(text);
  return <span data-testid="out">{shown}</span>;
}

describe("useSmoothText", () => {
  it("shows text that is already there when the component mounts", () => {
    // A transcript loaded from disk must not type itself out again.
    render(<Probe text="already here" />);
    expect(screen.getByTestId("out")).toHaveTextContent("already here");
  });

  it("paces text that grows after mount, then catches up", async () => {
    const { rerender } = render(<Probe text="hello " />);
    const full = "hello world, this keeps arriving";
    rerender(<Probe text={full} />);

    // The point of the hook: the new text is not painted in one lump.
    const immediate = screen.getByTestId("out").textContent ?? "";
    expect(immediate.length).toBeGreaterThan(0);
    expect(immediate.length).toBeLessThan(full.length);

    await waitFor(
      () => expect(screen.getByTestId("out")).toHaveTextContent(full),
      { timeout: 3000 },
    );
  });

  it("shows the whole string at once when motion is reduced", () => {
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      matches: query.includes("prefers-reduced-motion"),
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;

    try {
      const { rerender } = render(<Probe text="a" />);
      rerender(<Probe text="a much longer string" />);
      expect(screen.getByTestId("out")).toHaveTextContent("a much longer string");
    } finally {
      window.matchMedia = original;
    }
  });
});
