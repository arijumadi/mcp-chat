import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { installFakeScroll, removeFakeScroll } from "@/test/fakeScroll";
import { ThoughtRow } from "./ThoughtRow";

beforeEach(installFakeScroll);

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  removeFakeScroll();
});

const long = (chars: number) => "x".repeat(chars);
const row = () => screen.getByRole("button", { name: /Thought/ });
const body = () => screen.getByRole("region", { name: "Thought text" });

function scrollTo(element: HTMLElement, top: number) {
  element.scrollTop = top;
  fireEvent.scroll(element);
}

describe("ThoughtRow", () => {
  it("is collapsed by default and shows only the label", () => {
    render(<ThoughtRow text="secret reasoning" />);
    expect(row()).toHaveAttribute("aria-expanded", "false");
    expect(row().textContent).toBe("Thought");
    expect(screen.queryByText("secret reasoning")).not.toBeInTheDocument();
  });

  it("shows the duration only when one is given", () => {
    const { rerender } = render(<ThoughtRow text="t" duration="12s" />);
    expect(row()).toHaveTextContent("Thought12s");
    rerender(<ThoughtRow text="t" />);
    expect(row().textContent).toBe("Thought");
  });

  it("shows the text after expanding", () => {
    render(<ThoughtRow text="secret reasoning" />);
    fireEvent.click(row());
    expect(screen.getByText("secret reasoning")).toBeInTheDocument();
    fireEvent.click(row());
    expect(screen.queryByText("secret reasoning")).not.toBeInTheDocument();
  });

  it("opens a streaming thought at the bottom and follows new text", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    expect(body().scrollTop).toBe(200);

    rerender(<ThoughtRow text={long(450)} streaming />);
    expect(body().scrollTop).toBe(350);
    rerender(<ThoughtRow text={long(900)} streaming />);
    expect(body().scrollTop).toBe(800);
  });

  it("stops following when the user scrolls up, and does not pull them back", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    scrollTo(body(), 40);

    rerender(<ThoughtRow text={long(600)} streaming />);
    expect(body().scrollTop).toBe(40);
    rerender(<ThoughtRow text={long(900)} streaming />);
    expect(body().scrollTop).toBe(40);
  });

  it("does not drag the user back when text arrives before their scroll event is delivered", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    // A browser updates scrollTop at once but dispatches the scroll event a
    // frame later. A text update can land in that gap.
    body().scrollTop = 40;
    rerender(<ThoughtRow text={long(450)} streaming />);
    expect(body().scrollTop).toBe(40);
    fireEvent.scroll(body());
    rerender(<ThoughtRow text={long(600)} streaming />);
    expect(body().scrollTop).toBe(40);
  });

  it("follows again once the user scrolls back near the bottom", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    scrollTo(body(), 40);
    rerender(<ThoughtRow text={long(600)} streaming />);
    expect(body().scrollTop).toBe(40);

    // 500 is the max for 600 chars; 480 is 20px from the bottom, inside the threshold.
    scrollTo(body(), 480);
    rerender(<ThoughtRow text={long(800)} streaming />);
    expect(body().scrollTop).toBe(700);
  });

  it("does not resume following when the user stays well above the bottom", () => {
    const { rerender } = render(<ThoughtRow text={long(600)} streaming />);
    fireEvent.click(row());
    scrollTo(body(), 400); // 100px from the bottom
    rerender(<ThoughtRow text={long(800)} streaming />);
    expect(body().scrollTop).toBe(400);
  });

  it("starts following again after collapse and expand", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    scrollTo(body(), 0);
    rerender(<ThoughtRow text={long(500)} streaming />);
    expect(body().scrollTop).toBe(0);

    fireEvent.click(row());
    fireEvent.click(row());
    expect(body().scrollTop).toBe(400);
    rerender(<ThoughtRow text={long(700)} streaming />);
    expect(body().scrollTop).toBe(600);
  });

  it("opens a finished thought at the top and leaves it alone", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} />);
    fireEvent.click(row());
    expect(body().scrollTop).toBe(0);
    rerender(<ThoughtRow text={long(600)} />);
    expect(body().scrollTop).toBe(0);
  });

  it("stops following when the thought finishes", () => {
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    expect(body().scrollTop).toBe(200);
    rerender(<ThoughtRow text={long(300)} streaming={false} />);
    scrollTo(body(), 10);
    rerender(<ThoughtRow text={long(300)} streaming={false} />);
    expect(body().scrollTop).toBe(10);
  });

  it("never scrolls the page or the transcript", () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    const windowScroll = vi.spyOn(window, "scrollTo").mockImplementation(() => undefined);
    const { rerender } = render(<ThoughtRow text={long(300)} streaming />);
    fireEvent.click(row());
    rerender(<ThoughtRow text={long(600)} streaming />);
    expect(scrollIntoView).not.toHaveBeenCalled();
    expect(windowScroll).not.toHaveBeenCalled();
    delete (Element.prototype as unknown as Record<string, unknown>).scrollIntoView;
  });
});
