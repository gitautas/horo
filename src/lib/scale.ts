import type { CSSProperties } from "react";

/**
 * Theme-scaled sizes.
 *
 * Components write the design's own px, and these route them through the
 * theme's scale tokens (`--radius-scale`, `--density`, `--text-scale` in
 * `tokens/spacing.css`), so a theme can tighten the whole UI from CSS alone.
 * Under the classic theme every scale is 1 and the result is the same px it
 * always was.
 */

/** A corner radius. */
export const corner = (px: number) => `calc(${px}px * var(--radius-scale))`;

/**
 * Padding or a gap: a number of px, or a shorthand string whose every px value
 * is scaled ("6px 0 6px 52px").
 */
export function space(value: number | string): string {
  return typeof value === "number"
    ? `calc(${value}px * var(--density))`
    : value.replace(/(\d*\.?\d+)px/g, "calc($1px * var(--density))");
}

/** A UI font size. Message text has its own setting and doesn't go through this. */
export const textSize = (px: number) => `calc(${px}px * var(--text-scale))`;

/** The size of a title or heading, in `--font-heading`. */
export const headingSize = (px: number) =>
  `calc(${px}px * var(--text-scale) * var(--heading-scale))`;

/**
 * A small section label ("rooms", "accent"): classic's uppercase rave tag, or
 * whatever the theme makes of it. `tracking` is the classic letter spacing, in em.
 */
export function labelText(px: number, tracking = 0.14): CSSProperties {
  return {
    fontFamily: "var(--font-label)",
    fontSize: `calc(${px}px * var(--text-scale) * var(--label-scale))`,
    fontWeight: "var(--label-weight)" as CSSProperties["fontWeight"],
    letterSpacing: `calc(${tracking}em * var(--label-tracking))`,
    textTransform: "var(--label-case)" as CSSProperties["textTransform"],
  };
}
