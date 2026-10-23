import { Component, type ErrorInfo, type ReactNode } from "react";

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Keep the console clean in production; the UI shows a recoverable error.
    if (import.meta.env.DEV) console.error(error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div
        className="flex h-dvh items-center justify-center p-6"
        style={{ background: "var(--bg)", color: "var(--fg)" }}
      >
        <div
          className="w-full max-w-md rounded-2xl border p-5"
          style={{ borderColor: "var(--border)", background: "var(--panel)" }}
        >
          <div className="text-[15px] font-semibold">The interface hit an error</div>
          <p className="mt-1 text-[12.5px]" style={{ color: "var(--muted)" }}>
            Your conversations are safe in SQLite. Reload the window to continue.
          </p>
          <pre
            className="mt-3 max-h-32 overflow-auto rounded-lg border p-2.5 font-mono text-[10.5px] whitespace-pre-wrap"
            style={{ borderColor: "var(--border)", background: "var(--code)", color: "var(--muted)" }}
          >
            {this.state.error.message}
          </pre>
          <button
            onClick={() => window.location.reload()}
            className="focus-ring mt-3 rounded-lg px-3 py-1.5 text-[12.5px] font-medium"
            style={{ background: "var(--accent)", color: "var(--accent-fg)" }}
          >
            Reload
          </button>
        </div>
      </div>
    );
  }
}
