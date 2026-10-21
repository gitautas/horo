import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { Icon } from "./ui";
import { corner, space, textSize } from "../lib/scale";

export type MenuEntry =
  | {
      kind?: "item";
      label: string;
      /** A second line under the label — the current value, say. */
      hint?: string;
      icon?: string;
      /** Drawn in red: the item takes something away. */
      danger?: boolean;
      disabled?: boolean;
      onSelect: () => void;
    }
  | { kind: "check"; label: string; checked: boolean; onToggle: (next: boolean) => void }
  | { kind: "radio"; label: string; hint?: string; checked: boolean; onSelect: () => void }
  | { kind: "submenu"; label: string; hint?: string; items: MenuEntry[] }
  | { kind: "separator" };

const MENU_WIDTH = 220;
const EDGE = 8;

/**
 * A small menu at the pointer, for actions that shouldn't sit one stray click
 * away — modelled on Discord's server menu: sections split by hairlines,
 * checkboxes that stay open when ticked, and submenus that fly out to the side.
 *
 * Closes on a click anywhere else, Escape, a resize, or picking an item.
 */
export function ContextMenu({
  at,
  items,
  onClose,
}: {
  at: { x: number; y: number };
  items: MenuEntry[];
  onClose: () => void;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  return createPortal(
    <div
      // Its own backdrop, so the click that dismisses the menu doesn't also
      // land on whatever is underneath it.
      onClick={(e) => {
        e.stopPropagation();
        onClose();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onClose();
      }}
      style={{ position: "fixed", inset: 0, zIndex: 190 }}
    >
      <Panel at={at} items={items} onClose={onClose} />
    </div>,
    document.body,
  );
}

/**
 * One column of entries. Measures itself once mounted and moves back on
 * screen if it would spill off the right or bottom edge; a submenu that can't
 * fit on the right of its parent opens on the left instead.
 */
function Panel({
  at,
  items,
  onClose,
  flipX,
}: {
  at: { x: number; y: number };
  items: MenuEntry[];
  onClose: () => void;
  /** For a submenu: where to go instead if `at` doesn't fit — the parent's left edge. */
  flipX?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(at);
  const [open, setOpen] = useState<{ index: number; at: { x: number; y: number }; flipX: number } | null>(
    null,
  );

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    let x = at.x;
    if (x + width > window.innerWidth - EDGE) {
      x = flipX !== undefined ? flipX - width : window.innerWidth - width - EDGE;
    }
    const y = Math.min(at.y, window.innerHeight - height - EDGE);
    setPos({ x: Math.max(EDGE, x), y: Math.max(EDGE, y) });
  }, [at.x, at.y, flipX]);

  function openSubmenu(index: number, row: HTMLElement) {
    const r = row.getBoundingClientRect();
    setOpen({ index, at: { x: r.right + 4, y: r.top - 5 }, flipX: r.left - 4 });
  }

  const sub = open && items[open.index];

  return (
    <>
      <div
        ref={ref}
        role="menu"
        onClick={(e) => e.stopPropagation()}
        style={{
          position: "fixed",
          left: pos.x,
          top: pos.y,
          width: MENU_WIDTH,
          maxHeight: `calc(100vh - ${EDGE * 2}px)`,
          overflowY: "auto",
          padding: space(6),
          borderRadius: corner(14),
          background: "var(--surface-card-raised)",
          border: "1px solid var(--border-default)",
          boxShadow: "var(--shadow-pop)",
        }}
      >
        {items.map((entry, index) => {
          if (entry.kind === "separator") {
            return (
              <div
                key={`sep-${index}`}
                role="separator"
                style={{
                  height: 1,
                  margin: space("5px 8px"),
                  background: "var(--border-subtle)",
                }}
              />
            );
          }

          if (entry.kind === "submenu") {
            return (
              <Row
                key={entry.label}
                label={entry.label}
                hint={entry.hint}
                active={open?.index === index}
                trailing={<Icon name="caret-right" size={12} color="var(--text-tertiary)" />}
                ariaHasPopup
                onHover={(row) => openSubmenu(index, row)}
                onClick={(row) =>
                  open?.index === index ? setOpen(null) : openSubmenu(index, row)
                }
              />
            );
          }

          // Anything else closes a submenu that the pointer has moved away from.
          const leave = () => setOpen(null);

          if (entry.kind === "check") {
            return (
              <Row
                key={entry.label}
                label={entry.label}
                role="menuitemcheckbox"
                checked={entry.checked}
                trailing={<Checkbox on={entry.checked} />}
                onHover={leave}
                // Stays open: ticking a box is a setting, not a departure.
                onClick={() => entry.onToggle(!entry.checked)}
              />
            );
          }

          if (entry.kind === "radio") {
            return (
              <Row
                key={entry.label}
                label={entry.label}
                hint={entry.hint}
                role="menuitemradio"
                checked={entry.checked}
                trailing={<Radio on={entry.checked} />}
                onHover={leave}
                onClick={() => {
                  onClose();
                  entry.onSelect();
                }}
              />
            );
          }

          return (
            <Row
              key={entry.label}
              label={entry.label}
              hint={entry.hint}
              danger={entry.danger}
              disabled={entry.disabled}
              leading={
                entry.icon && (
                  <Icon
                    name={entry.icon}
                    size={15}
                    color={entry.danger ? "var(--status-danger)" : "var(--text-secondary)"}
                  />
                )
              }
              onHover={leave}
              onClick={() => {
                onClose();
                entry.onSelect();
              }}
            />
          );
        })}
      </div>

      {sub && sub.kind === "submenu" && (
        <Panel at={open.at} flipX={open.flipX} items={sub.items} onClose={onClose} />
      )}
    </>
  );
}

function Row({
  label,
  hint,
  leading,
  trailing,
  danger,
  disabled,
  active,
  role = "menuitem",
  checked,
  ariaHasPopup,
  onHover,
  onClick,
}: {
  label: string;
  hint?: string;
  leading?: React.ReactNode;
  trailing?: React.ReactNode;
  danger?: boolean;
  disabled?: boolean;
  active?: boolean;
  role?: string;
  checked?: boolean;
  ariaHasPopup?: boolean;
  onHover: (row: HTMLElement) => void;
  onClick: (row: HTMLElement) => void;
}) {
  const [hover, setHover] = useState(false);
  const lit = (hover || active) && !disabled;
  const colour = danger ? "var(--status-danger)" : "var(--text-primary)";

  return (
    <button
      role={role}
      aria-checked={checked}
      aria-haspopup={ariaHasPopup || undefined}
      aria-disabled={disabled || undefined}
      onMouseEnter={(e) => {
        setHover(true);
        onHover(e.currentTarget);
      }}
      onMouseLeave={() => setHover(false)}
      onClick={(e) => {
        if (!disabled) onClick(e.currentTarget);
      }}
      style={{
        width: "100%",
        display: "flex",
        alignItems: "center",
        gap: space(9),
        padding: space("8px 10px"),
        borderRadius: corner(9),
        cursor: disabled ? "default" : "pointer",
        border: "none",
        outline: "none",
        textAlign: "left",
        opacity: disabled ? 0.45 : 1,
        background: lit
          ? danger
            ? "color-mix(in srgb, var(--status-danger) 14%, transparent)"
            : "rgba(255,255,255,.06)"
          : "transparent",
      }}
    >
      {leading}
      <span style={{ flex: 1, minWidth: 0 }}>
        <span
          style={{
            display: "block",
            fontSize: textSize(13),
            fontWeight: 700,
            color: colour,
          }}
        >
          {label}
        </span>
        {hint && (
          <span
            style={{
              display: "block",
              marginTop: 1,
              fontSize: textSize(11),
              color: "var(--text-tertiary)",
            }}
          >
            {hint}
          </span>
        )}
      </span>
      {trailing}
    </button>
  );
}

function Checkbox({ on }: { on: boolean }) {
  return (
    <span
      aria-hidden
      style={{
        width: 18,
        height: 18,
        flex: "none",
        borderRadius: corner(5),
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        border: `1.5px solid ${on ? "var(--accent-primary)" : "var(--border-default)"}`,
        background: on ? "var(--accent-primary)" : "transparent",
      }}
    >
      {on && <Icon name="check" size={11} color="var(--text-on-accent)" />}
    </span>
  );
}

function Radio({ on }: { on: boolean }) {
  return (
    <span
      aria-hidden
      style={{
        width: 18,
        height: 18,
        flex: "none",
        borderRadius: 999,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        border: `1.5px solid ${on ? "var(--accent-primary)" : "var(--border-default)"}`,
      }}
    >
      {on && (
        <span
          style={{ width: 8, height: 8, borderRadius: 999, background: "var(--accent-primary)" }}
        />
      )}
    </span>
  );
}
