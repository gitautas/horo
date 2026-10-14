import { Component, type ErrorInfo, type ReactNode } from "react";
import { corner, headingSize, space, textSize } from "../lib/scale";

interface State {
  error: Error | null;
  componentStack: string | null;
}

/**
 * Without this, a render error in a Tauri window is just a black rectangle —
 * there's no browser chrome to hint that anything went wrong. Showing the error
 * (and the component stack) turns a mystery into a bug report.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, componentStack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("horo crashed", error, info.componentStack);
    this.setState({ componentStack: info.componentStack ?? null });
  }

  render() {
    const { error, componentStack } = this.state;
    if (!error) return this.props.children;

    return (
      <div
        style={{
          height: "100vh",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          padding: space(40),
          background: "var(--surface-app)",
          color: "var(--text-primary)",
          fontFamily: "var(--font-body)",
        }}
      >
        <div style={{ maxWidth: 680, width: "100%" }}>
          <div
            style={{
              fontFamily: "var(--font-heading)",
              fontWeight: 800,
              fontSize: headingSize(28),
              color: "var(--accent-secondary)",
            }}
          >
            horo fell over
          </div>
          <div style={{ marginTop: 8, color: "var(--text-secondary)", fontSize: textSize(14) }}>
            something went wrong while drawing the app. the details below are the
            useful bit.
          </div>

          <pre
            className="selectable uwu-scroll"
            style={{
              marginTop: 18,
              padding: space(16),
              maxHeight: 320,
              borderRadius: corner(16),
              background: "var(--ink-900)",
              border: "1px solid var(--border-subtle)",
              fontFamily: "var(--font-mono)",
              fontSize: textSize(12),
              lineHeight: 1.55,
              color: "var(--status-danger)",
              whiteSpace: "pre-wrap",
            }}
          >
            {error.message}
            {error.stack ? `\n\n${error.stack}` : ""}
            {componentStack ? `\n\ncomponents:${componentStack}` : ""}
          </pre>

          <button
            onClick={() => window.location.reload()}
            style={{
              marginTop: 18,
              padding: space("10px 20px"),
              borderRadius: 999,
              background: "var(--accent-primary)",
              color: "var(--text-on-accent)",
              border: "2px solid var(--ink-950)",
              boxShadow: "var(--shadow-sticker-ink)",
              fontFamily: "var(--font-display)",
              fontWeight: 700,
              fontSize: textSize(14),
              cursor: "pointer",
            }}
          >
            reload
          </button>
        </div>
      </div>
    );
  }
}
