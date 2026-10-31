import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { InputRequestModal } from "./InputRequestModal";
import { useApp } from "@/app/store";
import type { InputRequestEvent } from "@/lib/types";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("InputRequestModal", () => {
  it("submits a selected option for a built-in question", () => {
    const respondInput = vi.fn().mockResolvedValue(undefined);
    const request: InputRequestEvent = {
      type: "input_request",
      conversationId: "c1",
      callId: "call-question",
      request: {
        questions: [
          {
            header: "Color",
            question: "Which color should I use?",
            options: [
              { label: "Blue", description: "Cool and calm" },
              { label: "Green", description: "Natural" },
            ],
          },
        ],
      },
    };
    useApp.setState({ activeId: "c1", inputRequests: [request], respondInput });
    render(<InputRequestModal />);

    expect(screen.getByText("The model needs input")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: /Blue/ }));
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(respondInput).toHaveBeenCalledWith(
      "call-question",
      {
        answers: [
          {
            header: "Color",
            question: "Which color should I use?",
            answer: "Blue",
          },
        ],
      },
      false,
    );
  });
});
