/**
 * The bits of chrome every settings pane is built from.
 *
 * Shared rather than local to `SettingsView` so a pane can live in its own file
 * without either duplicating the styling or importing the screen that hosts it.
 */

import type { ReactNode } from "react";

import { Icon, RaveLabel } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

export function Heading({ children }: { children: ReactNode }) {
  return (
    <h2
      style={{
        fontFamily: "var(--font-heading)",
        fontWeight: 800,
        fontSize: headingSize(26),
        letterSpacing: "-0.02em",
        marginBottom: 20,
      }}
    >
      {children}
    </h2>
  );
}

export function Card({
  children,
  tone,
  backdrop,
}: {
  children: ReactNode;
  tone?: "warning";
  /** Something live to fill the card behind its contents, edge to edge. */
  backdrop?: ReactNode;
}) {
  return (
    <div
      style={{
        // Only a card with a backdrop clips: anything else may hold a menu or
        // popover that has to hang over the edge.
        ...(backdrop != null && { position: "relative", overflow: "hidden" }),
        background: "var(--surface-card)",
        border: `1px solid ${tone === "warning" ? "color-mix(in srgb, var(--amber-500) 35%, transparent)" : "var(--border-subtle)"}`,
        borderRadius: corner(20),
        padding: space(18),
        marginBottom: 14,
      }}
    >
      {backdrop == null ? (
        children
      ) : (
        <>
          {backdrop}
          <div style={{ position: "relative" }}>{children}</div>
        </>
      )}
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div style={{ marginBottom: 16 }}>
      <RaveLabel style={{ marginBottom: 7 }}>{label}</RaveLabel>
      {children}
      {hint && (
        <div
          style={{
            marginTop: 6,
            fontSize: textSize(12),
            color: "var(--text-tertiary)",
            lineHeight: 1.5,
          }}
        >
          {hint}
        </div>
      )}
    </div>
  );
}

export const inputStyle = {
  width: "100%",
  background: "var(--surface-inset)",
  border: "1px solid var(--border-subtle)",
  borderRadius: corner(14),
  padding: space("10px 14px"),
  color: "var(--text-primary)",
  fontSize: textSize(13.5),
  outline: "none",
} as const;

export function Row({
  icon,
  title,
  subtitle,
  children,
}: {
  icon?: string;
  title: ReactNode;
  subtitle?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(12),
        padding: space("10px 0"),
        borderBottom: "1px solid var(--border-subtle)",
      }}
    >
      {icon && <Icon name={icon} size={18} color="var(--text-tertiary)" />}
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontFamily: "var(--font-heading)", fontWeight: 700, fontSize: headingSize(14) }}>
          {title}
        </div>
        {subtitle && (
          <div
            className="selectable"
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: textSize(11),
              color: "var(--text-tertiary)",
              marginTop: 2,
            }}
          >
            {subtitle}
          </div>
        )}
      </div>
      {children}
    </div>
  );
}
