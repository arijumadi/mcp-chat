import { afterEach, describe, expect, expectTypeOf, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { InputRequestModal } from "./InputRequestModal";
import { useApp } from "@/app/store";
import type { InputRequestEvent, QuestionOption, QuestionPrompt } from "@/lib/types";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function open(questions: QuestionPrompt[]) {
  const respondInput = vi.fn().mockResolvedValue(undefined);
  const request: InputRequestEvent = {
    type: "input_request",
    conversationId: "c1",
    callId: "call-question",
    request: { questions },
  };
  useApp.setState({ activeId: "c1", inputRequests: [request], respondInput });
  render(<InputRequestModal />);
  return respondInput;
}

const color: QuestionPrompt = {
  header: "Color",
  question: "Which color should I use?",
  options: [
    { label: "Blue", description: "Cool and calm" },
    { label: "Green", description: "Natural" },
  ],
};

const database: QuestionPrompt = {
  header: "Database",
  question: "Which database should I target?",
};

const formats: QuestionPrompt = {
  header: "Formats",
  question: "Which output formats do you want?",
  multiple: true,
  options: [{ label: "Markdown" }, { label: "JSON" }, { label: "CSV" }],
};

describe("InputRequestModal", () => {
  it("submits a selected option for a built-in question", () => {
    const respondInput = open([color]);

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

  it("keeps the options and the custom answer field for a question with options", () => {
    open([color]);

    expect(screen.getAllByRole("radio")).toHaveLength(2);
    expect(screen.getByPlaceholderText("Or type a custom answer…")).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("Type your answer…")).not.toBeInTheDocument();
  });

  it("shows only a text field for a question without options", () => {
    open([database]);

    expect(screen.getByText("Which database should I target?")).toBeInTheDocument();
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText("Type your answer…")).toBeInTheDocument();
    expect(screen.queryByPlaceholderText("Or type a custom answer…")).not.toBeInTheDocument();
    // No empty box where the options would have been: the card holds the
    // header, the question and the answer field, nothing else.
    const card = screen.getByText("Which database should I target?").parentElement;
    expect(card?.children).toHaveLength(3);
  });

  it("treats an empty options list like a missing one", () => {
    open([{ ...database, options: [] }]);

    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText("Type your answer…")).toBeInTheDocument();
  });

  it("sends the typed text as the answer to a free-form question", () => {
    const respondInput = open([database]);

    fireEvent.change(screen.getByRole("textbox", { name: "Answer for Database" }), {
      target: { value: "  Postgres 16  " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(respondInput).toHaveBeenCalledWith(
      "call-question",
      {
        answers: [
          {
            header: "Database",
            question: "Which database should I target?",
            answer: "Postgres 16",
          },
        ],
      },
      false,
    );
  });

  it("puts the cursor in the answer field when the first question is free-form", async () => {
    open([database]);

    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Answer for Database" })).toHaveFocus(),
    );
  });

  it("does not move focus into the first field when the first question has options", async () => {
    open([color, database]);

    await waitFor(() => expect(screen.getByRole("dialog")).toHaveFocus());
    expect(screen.getByRole("textbox", { name: "Answer for Database" })).not.toHaveFocus();
  });

  it("sends the answers with Enter from a text field", () => {
    const respondInput = open([database]);

    const field = screen.getByRole("textbox", { name: "Answer for Database" });
    fireEvent.change(field, { target: { value: "SQLite" } });
    fireEvent.keyDown(field, { key: "Enter" });

    expect(respondInput).toHaveBeenCalledTimes(1);
    expect(respondInput.mock.calls[0]?.[1]).toEqual({
      answers: [
        { header: "Database", question: "Which database should I target?", answer: "SQLite" },
      ],
    });
  });

  it("does not send on the Enter that confirms an IME composition", () => {
    const respondInput = open([database]);

    const field = screen.getByRole("textbox", { name: "Answer for Database" });
    fireEvent.change(field, { target: { value: "にほん" } });
    fireEvent.keyDown(field, { key: "Enter", isComposing: true });

    expect(respondInput).not.toHaveBeenCalled();
  });

  it("lets a custom answer win over a chosen option", () => {
    const respondInput = open([color]);

    fireEvent.click(screen.getByRole("radio", { name: /Blue/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Custom answer for Color" }), {
      target: { value: "Teal" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(respondInput.mock.calls[0]?.[1]).toEqual({
      answers: [{ header: "Color", question: "Which color should I use?", answer: "Teal" }],
    });
  });

  it("falls back to the chosen option when the custom answer is only spaces", () => {
    const respondInput = open([color]);

    fireEvent.click(screen.getByRole("radio", { name: /Green/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "Custom answer for Color" }), {
      target: { value: "   " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(respondInput.mock.calls[0]?.[1]).toEqual({
      answers: [{ header: "Color", question: "Which color should I use?", answer: "Green" }],
    });
  });

  it("joins several chosen options with a comma and a space", () => {
    const respondInput = open([formats]);

    expect(screen.getAllByRole("checkbox")).toHaveLength(3);
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("checkbox", { name: "Markdown" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "CSV" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "JSON" }));
    // Unchecking removes only that option.
    fireEvent.click(screen.getByRole("checkbox", { name: "CSV" }));
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(respondInput.mock.calls[0]?.[1]).toEqual({
      answers: [
        {
          header: "Formats",
          question: "Which output formats do you want?",
          answer: "Markdown, JSON",
        },
      ],
    });
  });

  it("refuses an empty answer and names the question that is missing", () => {
    const respondInput = open([color, database]);

    fireEvent.click(screen.getByRole("radio", { name: /Blue/ }));
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));
    expect(screen.getByText('Answer "Database" to continue.')).toBeInTheDocument();
    expect(respondInput).not.toHaveBeenCalled();

    // Spaces are not an answer either.
    fireEvent.change(screen.getByRole("textbox", { name: "Answer for Database" }), {
      target: { value: "   " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));
    expect(respondInput).not.toHaveBeenCalled();

    fireEvent.change(screen.getByRole("textbox", { name: "Answer for Database" }), {
      target: { value: "MySQL" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));
    expect(respondInput.mock.calls[0]?.[1]).toEqual({
      answers: [
        { header: "Color", question: "Which color should I use?", answer: "Blue" },
        { header: "Database", question: "Which database should I target?", answer: "MySQL" },
      ],
    });
  });

  it("names the first unanswered question when a free-form one is empty", () => {
    const respondInput = open([database]);

    fireEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(screen.getByText('Answer "Database" to continue.')).toBeInTheDocument();
    expect(respondInput).not.toHaveBeenCalled();
  });

  it("still declines with Decline", () => {
    const respondInput = open([database]);

    fireEvent.click(screen.getByRole("button", { name: "Decline" }));

    expect(respondInput).toHaveBeenCalledWith("call-question", {}, true);
  });

  it("still answers an MCP elicitation form", () => {
    const respondInput = vi.fn().mockResolvedValue(undefined);
    const request: InputRequestEvent = {
      type: "input_request",
      conversationId: "c1",
      callId: "call-elicit",
      request: {
        inputRequests: {
          ask: {
            method: "elicitation/create",
            params: {
              mode: "form",
              message: "What is your name?",
              requestedSchema: {
                type: "object",
                properties: { name: { type: "string", title: "Name" } },
                required: ["name"],
              },
            },
          },
        },
      },
    };
    useApp.setState({ activeId: "c1", inputRequests: [request], respondInput });
    render(<InputRequestModal />);

    expect(screen.getByText("The server needs input")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Send response" }));
    expect(screen.getByText('"name" is required.')).toBeInTheDocument();
    expect(respondInput).not.toHaveBeenCalled();

    fireEvent.change(screen.getByRole("textbox"), { target: { value: "Ari" } });
    fireEvent.click(screen.getByRole("button", { name: "Send response" }));
    expect(respondInput).toHaveBeenCalledWith(
      "call-elicit",
      { ask: { action: "accept", content: { name: "Ari" } } },
      false,
    );
  });
});

describe("QuestionPrompt", () => {
  it("makes options optional", () => {
    expectTypeOf<QuestionPrompt["options"]>().toEqualTypeOf<QuestionOption[] | undefined>();

    // A free-form question compiles without `options`.
    const freeForm: QuestionPrompt = { header: "Database", question: "Which one?" };
    expect(freeForm.options ?? []).toEqual([]);
  });
});
