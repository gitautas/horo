/**
 * These are security tests, not formatting tests.
 *
 * `formatted_body` is attacker-controlled: anyone in a room can send one, and
 * the homeserver can rewrite it. Inside a Tauri WebView, script execution means
 * access to the IPC bridge and therefore the session and message keys. Each
 * case below is a way that could go wrong.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { renderFormattedBody } from "./richText";

function html(source: string): HTMLElement {
  const { container } = render(<div>{renderFormattedBody(source)}</div>);
  return container;
}

describe("renderFormattedBody — untrusted markup", () => {
  it("never emits a script element, and keeps no script text", () => {
    const container = html('<script>window.__pwned = true</script>hello');
    expect(container.querySelector("script")).toBeNull();
    expect(window).not.toHaveProperty("__pwned");
    expect(container.textContent).toContain("hello");
  });

  it("drops event-handler attributes", () => {
    const container = html('<img src="x" onerror="window.__pwned = true">');
    const img = container.querySelector("img");
    // Either no image at all (non-mxc src is rejected) or one with no handler.
    expect(img?.getAttribute("onerror") ?? null).toBeNull();
    expect(window).not.toHaveProperty("__pwned");
  });

  it("refuses javascript: and data: links but keeps their text", () => {
    for (const href of [
      "javascript:alert(1)",
      "JavaScript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "blob:https://example.org/x",
      "file:///etc/passwd",
    ]) {
      const container = html(`<a href="${href}">click me</a>`);
      expect(container.querySelector("a")).toBeNull();
      expect(container.textContent).toBe("click me");
    }
  });

  it("keeps ordinary links, and opens them without handing over the opener", () => {
    const container = html('<a href="https://example.org/x">a link</a>');
    const anchor = container.querySelector("a");
    expect(anchor?.getAttribute("href")).toBe("https://example.org/x");
    expect(anchor?.getAttribute("rel")).toContain("noopener");
  });

  it("does not load remote images, which would leak the reader's IP", () => {
    const container = html('<img src="https://tracker.example/pixel.gif" alt="pic">');
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe("pic");
  });

  it("renders mxc images through our own media protocol", () => {
    const container = html('<img src="mxc://uwu.gg/abc" alt="pic">');
    expect(container.querySelector("img")?.getAttribute("src")).toContain("horo://media/");
  });

  it("strips iframes and objects while keeping surrounding text", () => {
    const container = html('before<iframe src="https://evil.example"></iframe>after');
    expect(container.querySelector("iframe")).toBeNull();
    expect(container.textContent).toBe("beforeafter");
  });

  it("survives absurd nesting without blowing the stack", () => {
    const deep = "<div>".repeat(500) + "deep" + "</div>".repeat(500);
    expect(() => html(deep)).not.toThrow();
  });

  it("treats unclosed and malformed markup as text, not an error", () => {
    expect(() => html("<strong>unclosed <em>tags")).not.toThrow();
    expect(() => html("<<>><<")).not.toThrow();
  });
});

describe("renderFormattedBody — formatting", () => {
  it("renders emphasis", () => {
    const container = html("<strong>bold</strong> and <em>italic</em>");
    expect(container.textContent).toBe("bold and italic");
  });

  it("renders lists with markers", () => {
    const container = html("<ul><li>one</li><li>two</li></ul>");
    expect(container.textContent).toContain("one");
    expect(container.textContent).toContain("two");
  });

  it("renders ordered lists numbered from one", () => {
    const container = html("<ol><li>first</li><li>second</li></ol>");
    expect(container.textContent).toContain("1.");
    expect(container.textContent).toContain("2.");
  });

  it("drops the reply fallback, which we render as our own chip", () => {
    const container = html(
      "<mx-reply><blockquote>quoted</blockquote></mx-reply>the actual reply",
    );
    expect(container.textContent).toBe("the actual reply");
  });

  it("returns null for empty markup so the caller falls back to the plain body", () => {
    expect(renderFormattedBody("   ")).toBeNull();
  });

  it("renders code blocks", () => {
    render(<div>{renderFormattedBody("<pre><code>cargo test</code></pre>")}</div>);
    expect(screen.getByText("cargo test")).toBeTruthy();
  });
});

describe("renderFormattedBody — custom emotes (MSC2545)", () => {
  const EMOTE = '<img data-mx-emoticon height="32" src="mxc://veil.gg/blob" alt=":blobcat:">';

  it("draws an inline emote at text height, not at image size", () => {
    const img = html(`nice ${EMOTE}`).querySelector("img");
    // The sender's own height attribute is ignored; ours follows the text.
    expect(img?.style.height).toBe("1.45em");
  });

  it("goes jumbo when the message is nothing but emotes", () => {
    const img = html(`${EMOTE} ${EMOTE}`).querySelector("img");
    expect(img?.style.height).toBe("48px");
  });

  it("stays inline when a reply fallback is the only other content", () => {
    const container = html(
      `<mx-reply><blockquote>quoted</blockquote></mx-reply>${EMOTE}`,
    );
    expect(container.querySelector("img")?.style.height).toBe("48px");
  });

  it("falls back to the shortcode when the source isn't fetchable", () => {
    const container = html(
      '<img data-mx-emoticon src="https://evil.example/track.gif" alt=":blobcat:">',
    );
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe(":blobcat:");
  });

  it("leaves ordinary inline images at image size", () => {
    const img = html('<img src="mxc://veil.gg/pic" alt="a picture">').querySelector("img");
    expect(img?.style.height).toBe("");
    // 240 is the intended size — the `min()` caps it to the container as well,
    // so a narrow screen gets a smaller picture rather than a sideways scroll.
    expect(img?.style.maxWidth).toBe("min(240px, 100%)");
  });
});
