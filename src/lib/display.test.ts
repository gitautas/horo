import { describe, expect, it } from "vitest";
import { initialsFor } from "./display";

describe("initialsFor", () => {
  it("takes the first letters of the first two words", () => {
    expect(initialsFor("tekno flyers")).toBe("tf");
    expect(initialsFor("onion_snacc")).toBe("os");
  });

  it("strips Matrix sigils and servers", () => {
    expect(initialsFor("@kii:uwu.gg")).toBe("ki");
  });

  it("ignores emoji instead of splitting them into lone surrogates", () => {
    expect(initialsFor("skitchy 🍁")).toBe("sk");
    expect(initialsFor("Faux 🏳️‍⚧️")).toBe("fa");
    expect(initialsFor("🍁skitchy")).toBe("sk");
  });

  it("keeps non-BMP letters whole", () => {
    expect(initialsFor("𝓈𝓀𝒾𝓉")).toBe("𝓈𝓀");
  });

  it("falls back to the first whole emoji for emoji-only names", () => {
    expect(initialsFor("🍁")).toBe("🍁");
  });
});
