import { useEffect, useMemo, useRef, useState } from "react";
import { MessageSquareQuote } from "lucide-react";
import { useApp } from "@/app/store";
import { Button } from "@/components/Button";
import { Modal } from "@/components/Modal";
import { inputClass } from "@/components/ui";
import { call } from "@/lib/ipc";
import type { InputRequestEvent, QuestionPrompt } from "@/lib/types";

export function InputRequestModal() {
  const requests = useApp((state) => state.inputRequests);
  const activeId = useApp((state) => state.activeId);
  const respond = useApp((state) => state.respondInput);
  const toast = useApp((state) => state.toast);
  const request = requests[0];
  const subagent = request !== undefined && request.conversationId !== activeId;
  const [values, setValues] = useState<Record<string, Record<string, unknown>>>({});
  const [error, setError] = useState<string | null>(null);

  const entries = useMemo(() => Object.entries(request?.request.inputRequests ?? {}), [request]);
  const questions = useMemo<QuestionPrompt[]>(
    () =>
      Array.isArray(request?.request.questions)
        ? (request.request.questions as QuestionPrompt[])
        : [],
    [request],
  );

  useEffect(() => {
    setValues({});
    setError(null);
  }, [request?.callId]);

  if (!request) return null;

  if (questions.length > 0) {
    return <QuestionRequest request={request} questions={questions} subagent={subagent} />;
  }

  const unsupported = entries.filter(
    ([, item]) => item.method !== "elicitation/create",
  );
  const forms = entries.filter(
    ([, item]) => item.method === "elicitation/create" && item.params.mode !== "url",
  );
  const urls = entries.filter(
    ([, item]) => item.method === "elicitation/create" && item.params.mode === "url",
  );

  const accept = () => {
    const responses: Record<string, unknown> = {};
    for (const [key, item] of forms) {
      const schema = item.params.requestedSchema;
      const content = values[key] ?? {};
      for (const required of schema?.required ?? []) {
        const current = content[required];
        if (current === undefined || current === "") {
          setError(`"${required}" is required.`);
          return;
        }
      }
      responses[key] = { action: "accept", content };
    }
    for (const [key] of urls) {
      responses[key] = { action: "accept" };
    }
    if (unsupported.length > 0) {
      setError("This app cannot answer sampling or roots requests. Decline to continue.");
      return;
    }
    void respond(request.callId, responses, false);
  };

  return (
    <Modal
      open
      onClose={() => undefined}
      width="md"
      title={
        <span className="flex items-center gap-2">
          <MessageSquareQuote size={16} style={{ color: "var(--accent)" }} />
          {subagent ? "A subagent needs input" : "The server needs input"}
        </span>
      }
      subtitle={
        subagent
          ? "A subagent is paused until you answer."
          : "The tool call is paused until you answer."
      }
      footer={
        <>
          <Button size="sm" variant="ghost" onClick={() => void respond(request.callId, {}, true)}>
            Decline
          </Button>
          <Button
            size="sm"
            variant="primary"
            disabled={unsupported.length > 0}
            onClick={accept}
          >
            Send response
          </Button>
        </>
      }
    >
      {request.request.requestState ? (
        <p className="mb-3 text-[11.5px]" style={{ color: "var(--faint)" }}>
          The server sent a continuation state. It will be echoed back verbatim.
        </p>
      ) : null}

      {entries.length === 0 ? (
        <p className="text-[13.5px]" style={{ color: "var(--muted)" }}>
          The server requested another round trip without a form. You can retry immediately.
        </p>
      ) : null}

      {forms.map(([key, item]) => (
        <div key={key} className="mb-4">
          {item.params.message ? (
            <p className="mb-2 text-[13.5px]">{item.params.message}</p>
          ) : null}
          <div className="grid gap-3">
            {Object.entries(item.params.requestedSchema?.properties ?? {}).map(
              ([field, property]) => {
                const required = item.params.requestedSchema?.required?.includes(field);
                if (property.enum) {
                  return (
                    <label key={field} className="block">
                      <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                        {property.title ?? field}
                        {required ? " *" : ""}
                      </span>
                      <select
                        value={String(values[key]?.[field] ?? "")}
                        onChange={(event) =>
                          setValues((current) => ({
                            ...current,
                            [key]: { ...current[key], [field]: event.target.value },
                          }))
                        }
                        className={inputClass}
                        style={{ borderColor: "var(--border)", background: "var(--panel)" }}
                      >
                        <option value="">Choose…</option>
                        {property.enum.map((option) => (
                          <option key={String(option)} value={String(option)}>
                            {String(option)}
                          </option>
                        ))}
                      </select>
                    </label>
                  );
                }
                if (property.type === "boolean") {
                  return (
                    <label key={field} className="flex items-center gap-2 text-[12.5px]">
                      <input
                        type="checkbox"
                        checked={Boolean(values[key]?.[field] ?? false)}
                        onChange={(event) =>
                          setValues((current) => ({
                            ...current,
                            [key]: { ...current[key], [field]: event.target.checked },
                          }))
                        }
                      />
                      {property.title ?? field}
                      {required ? " *" : ""}
                    </label>
                  );
                }
                const numeric = property.type === "number" || property.type === "integer";
                return (
                  <label key={field} className="block">
                    <span className="mb-1 block text-[11.5px]" style={{ color: "var(--muted)" }}>
                      {property.title ?? field}
                      {required ? " *" : ""}
                    </span>
                    <input
                      type={numeric ? "number" : "text"}
                      value={String(values[key]?.[field] ?? "")}
                      onChange={(event) =>
                        setValues((current) => ({
                          ...current,
                          [key]: {
                            ...current[key],
                            [field]: numeric ? Number(event.target.value) : event.target.value,
                          },
                        }))
                      }
                      className={inputClass}
                      style={{ borderColor: "var(--border)" }}
                    />
                    {property.description ? (
                      <span className="mt-0.5 block text-[10.5px]" style={{ color: "var(--faint)" }}>
                        {property.description}
                      </span>
                    ) : null}
                  </label>
                );
              },
            )}
          </div>
        </div>
      ))}

      {urls.map(([key, item]) => (
        <div key={key} className="mb-4">
          <p className="mb-2 text-[13.5px]">{item.params.message}</p>
          {item.params.url ? (
            <Button
              size="sm"
              variant="secondary"
              onClick={() => void call("open_external", { url: item.params.url }).catch(() => toast("Could not open URL", "error"))}
            >
              Open link in browser
            </Button>
          ) : null}
        </div>
      ))}

      {unsupported.length > 0 ? (
        <div
          className="rounded-lg border px-3 py-2.5 text-[12.5px]"
          style={{
            borderColor: "var(--border)",
            background: "var(--panel-2)",
            color: "var(--muted)",
          }}
        >
          This server asked for{" "}
          {unsupported.map(([, item]) => item.method).join(", ")}. Jarvis does not support
          sampling or roots requests in v1. Decline to let the tool settle with an error.
        </div>
      ) : null}

      {error ? (
        <p className="mt-3 text-[12.5px]" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      ) : null}
    </Modal>
  );
}


/** A question without `options` is free-form: the user just types an answer. */
function optionsOf(question: QuestionPrompt) {
  return question.options ?? [];
}

function QuestionRequest({
  request,
  questions,
  subagent = false,
}: {
  request: InputRequestEvent;
  questions: QuestionPrompt[];
  subagent?: boolean;
}) {
  const respond = useApp((state) => state.respondInput);
  const [values, setValues] = useState<
    Record<number, { selected: string[]; custom: string }>
  >({});
  const [error, setError] = useState<string | null>(null);
  const firstAnswer = useRef<HTMLInputElement>(null);
  const opensOnFreeForm = questions[0] ? optionsOf(questions[0]).length === 0 : false;

  useEffect(() => {
    setValues({});
    setError(null);
  }, [request.callId]);

  useEffect(() => {
    if (!opensOnFreeForm) return;
    // The dialog moves focus to itself on the next frame. Asking one frame
    // later, after it has done so, leaves the cursor in the answer field.
    const frame = requestAnimationFrame(() => firstAnswer.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [request.callId, opensOnFreeForm]);

  const update = (index: number, patch: Partial<{ selected: string[]; custom: string }>) => {
    setValues((current) => {
      const previous = current[index] ?? { selected: [], custom: "" };
      return { ...current, [index]: { ...previous, ...patch } };
    });
  };

  const submit = () => {
    const answers = questions.map((question, index) => {
      const value = values[index];
      const selected = value?.selected ?? [];
      const custom = value?.custom.trim() ?? "";
      return {
        header: question.header,
        question: question.question,
        answer: custom || selected.join(", "),
      };
    });
    const missing = answers.find((answer) => !answer.answer);
    if (missing) {
      setError(`Answer "${missing.header}" to continue.`);
      return;
    }
    void respond(request.callId, { answers }, false);
  };

  return (
    <Modal
      open
      onClose={() => undefined}
      width="md"
      title={
        <span className="flex items-center gap-2">
          <MessageSquareQuote size={16} style={{ color: "var(--accent)" }} />
          {subagent ? "A subagent needs input" : "The model needs input"}
        </span>
      }
      subtitle={
        subagent
          ? "A subagent is paused until you answer."
          : "Answer the question to continue the tool call."
      }
      footer={
        <>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void respond(request.callId, {}, true)}
          >
            Decline
          </Button>
          <Button size="sm" variant="primary" onClick={submit}>
            Send answer
          </Button>
        </>
      }
    >
      <div className="max-h-[60vh] space-y-4 overflow-y-auto pr-1">
        {questions.map((question, index) => {
          const value = values[index] ?? { selected: [], custom: "" };
          const options = optionsOf(question);
          const freeForm = options.length === 0;
          return (
            <div
              key={`${question.header}-${index}`}
              className="rounded-xl border p-3.5"
              style={{ borderColor: "var(--border)", background: "var(--panel)" }}
            >
              <div
                className="text-[10.5px] tracking-[0.06em] uppercase"
                style={{ color: "var(--faint)" }}
              >
                {question.header}
              </div>
              <div className="mt-1 text-[13.5px] font-medium">{question.question}</div>
              {freeForm ? null : (
                <div className="mt-2.5 grid gap-1.5">
                  {options.map((option) => {
                    const checked = value.selected.includes(option.label);
                    return (
                      <label
                        key={option.label}
                        className="flex cursor-pointer items-start gap-2 rounded-lg px-2 py-1.5 text-[12.5px] transition hover:bg-[var(--panel-2)]"
                      >
                        <input
                          type={question.multiple ? "checkbox" : "radio"}
                          name={`question-${index}`}
                          checked={checked}
                          onChange={() => {
                            update(index, {
                              selected: question.multiple
                                ? checked
                                  ? value.selected.filter((item) => item !== option.label)
                                  : [...value.selected, option.label]
                                : [option.label],
                            });
                          }}
                          className="mt-0.5"
                        />
                        <span className="min-w-0">
                          <span className="block">{option.label}</span>
                          {option.description ? (
                            <span
                              className="mt-0.5 block text-[10.5px] leading-snug"
                              style={{ color: "var(--faint)" }}
                            >
                              {option.description}
                            </span>
                          ) : null}
                        </span>
                      </label>
                    );
                  })}
                </div>
              )}
              <input
                ref={index === 0 ? firstAnswer : undefined}
                value={value.custom}
                onChange={(event) => update(index, { custom: event.target.value })}
                onKeyDown={(event) => {
                  // Enter sends the answers; it must not fire while an IME is
                  // still composing a character.
                  if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                    event.preventDefault();
                    submit();
                  }
                }}
                placeholder={freeForm ? "Type your answer…" : "Or type a custom answer…"}
                className={`${inputClass} ${freeForm ? "mt-2.5" : "mt-2"}`}
                style={{ borderColor: "var(--border)" }}
                aria-label={
                  freeForm ? `Answer for ${question.header}` : `Custom answer for ${question.header}`
                }
              />
            </div>
          );
        })}
      </div>
      {error ? (
        <p className="mt-3 text-[12.5px]" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      ) : null}
    </Modal>
  );
}
