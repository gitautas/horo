import { describe, expect, it } from "vitest";

import { stepZoom, ZOOM_RANGE } from "./settings";

describe("stepZoom", () => {
  it("steps by a tenth and comes back to exactly 1", () => {
    let zoom = 1;
    for (let i = 0; i < 3; i++) zoom = stepZoom(zoom, 1);
    expect(zoom).toBe(1.3);
    for (let i = 0; i < 3; i++) zoom = stepZoom(zoom, -1);
    expect(zoom).toBe(1);
  });

  it("stays inside the range", () => {
    expect(stepZoom(ZOOM_RANGE.max, 1)).toBe(ZOOM_RANGE.max);
    expect(stepZoom(ZOOM_RANGE.min, -1)).toBe(ZOOM_RANGE.min);
  });
});
