import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import * as ipc from "../lib/ipc";
import { emoteLookup, emoteRefs, matchEmotes, typingShortcode } from "../lib/packs";
import type { EventItem, PackImage } from "../lib/types";
import { uploadFiles, uploadPaths } from "../lib/upload";
import { useIsMobile } from "../lib/viewport";
import { selectDraft, useStore } from "../store";
import { AttachSheet } from "./AttachSheet";
import { EmojiPicker } from "./EmojiPicker";
import { Icon } from "./ui";
import { corner, space, textSize } from "../lib/scale";

/** How long we let a typing notice stand before refreshing it. */
const TYPING_REFRESH_MS = 4000;

export function Composer({
  roomId,
  roomName,
  threadRoot,
  encrypted,
}: {
  roomId: string;
  roomName: string;
  threadRoot?: string;
  encrypted: boolean;
}) {
  const key = threadRoot ? `${roomId}|${threadRoot}` : roomId;
  const isMobile = useIsMobile();
  /** Hidden inputs, opened only from the attach sheet — see `attach`. */
  const fileInput = useRef<HTMLInputElement>(null);
  const cameraInput = useRef<HTMLInputElement>(null);
  const [attachOpen, setAttachOpen] = useState(false);

  const { draft, setDraft, clearDraft, showBanner, packs } = useStore(
    useShallow((s) => ({
      draft: selectDraft(s, key),
      setDraft: s.setDraft,
      clearDraft: s.clearDraft,
      showBanner: s.showBanner,
      packs: s.packs,
    })),
  );

  const input = useRef<HTMLTextAreaElement>(null);
  const typingSentAt = useRef(0);
  const emojiButton = useRef<HTMLButtonElement>(null);
  const [pickerAnchor, setPickerAnchor] = useState<DOMRect | null>(null);

  const emotes = useMemo(() => emoteLookup(packs), [packs]);
  /** The `:shortcode` being typed at the caret, and what it matches. */
  const [completing, setCompleting] = useState<{
    start: number;
    matches: PackImage[];
    active: number;
  } | null>(null);

  // Focus the composer when the room changes — you almost always want to type.
  //
  // Except on a phone, where focusing raises the keyboard, and the keyboard
  // covers half of what you opened the room to read. Opening a room there is
  // usually "catch up", not "reply"; tapping the composer says otherwise.
  useEffect(() => {
    if (isMobile) return;
    input.current?.focus();
  }, [key, isMobile]);

  // Stop advertising "typing" when leaving the room, otherwise the indicator
  // sticks for everyone else until the server times it out.
  useEffect(() => {
    return () => {
      if (typingSentAt.current) {
        void ipc.setTyping(roomId, false).catch(() => {});
        typingSentAt.current = 0;
      }
    };
  }, [roomId]);

  function noteTyping() {
    const now = Date.now();
    if (now - typingSentAt.current < TYPING_REFRESH_MS) return;
    typingSentAt.current = now;
    void ipc.setTyping(roomId, true).catch(() => {});
  }

  async function submit() {
    const body = draft.body.trim();
    if (!body) return;

    // Clear straight away: the local echo from the backend is what the user
    // should see next, not their own text sitting in the box.
    clearDraft(key);
    if (typingSentAt.current) {
      void ipc.setTyping(roomId, false).catch(() => {});
      typingSentAt.current = 0;
    }

    try {
      if (draft.editing) {
        await ipc.editMessage(roomId, draft.editing, body, true, threadRoot);
      } else {
        await ipc.sendMessage(roomId, {
          body,
          // Always on: ruma only adds a formatted body when the text actually
          // contains markup, so plain messages stay plain.
          markdown: true,
          replyTo: draft.replyTo,
          threadRoot: threadRoot ?? null,
          // Only when there's something that could be a shortcode — otherwise
          // every message would carry the whole emote set across the bridge.
          emotes: /:[^\s:]+:/.test(body) ? emoteRefs(packs) : [],
        });
      }
    } catch (e) {
      // Put the text back so nothing is lost.
      setDraft(key, { body, replyTo: draft.replyTo, editing: draft.editing });
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  /**
   * The newest message in this timeline matching `wanted`.
   *
   * The timeline is read at keypress rather than subscribed to: the composer
   * has no other reason to re-render every time a message arrives, and this is
   * only ever wanted at the moment the key goes down.
   */
  function findLast(wanted: (event: EventItem) => boolean): EventItem | null {
    const items = useStore.getState().timelines[key] ?? [];

    for (let i = items.length - 1; i >= 0; i--) {
      const event = items[i].event;
      if (event?.eventId && wanted(event)) return event;
    }

    return null;
  }

  /**
   * Put the last message you can still edit back in the box.
   *
   * `isEditable` is the same flag the pencil on a message uses, so this can
   * only ever pick something the server would actually accept an edit for.
   */
  function editLastOwnMessage(): boolean {
    const event = findLast((e) => e.isEditable);
    if (!event || !("body" in event.content)) return false;

    setDraft(key, { editing: event.eventId, body: event.content.body, replyTo: null });

    // The value is about to change under a caret sitting at 0, which would drop
    // the cursor before the text rather than after it. Same growth the
    // `onChange` handler does, for a message taller than one line.
    requestAnimationFrame(() => {
      const field = input.current;
      if (!field) return;
      field.focus();
      field.selectionStart = field.selectionEnd = field.value.length;
      field.style.height = "auto";
      field.style.height = `${Math.min(field.scrollHeight, 160)}px`;
    });

    return true;
  }

  /**
   * Start a reply to the last thing somebody else said.
   *
   * `canReply` is the SDK's own predicate and the one the reply button on a
   * message uses. Nothing goes in the box — a reply is an empty composer
   * pointed at an event, which is what the bar above it then announces.
   */
  function replyToLastOtherMessage(): boolean {
    const event = findLast((e) => !e.isOwn && e.canReply);
    if (!event) return false;

    setDraft(key, { replyTo: event.eventId, editing: null });
    return true;
  }

  /** Replace whatever is being typed at the caret with `text`. */
  function insertAtCaret(text: string, replaceFrom?: number) {
    const field = input.current;
    const caret = field?.selectionStart ?? draft.body.length;
    const from = replaceFrom ?? caret;

    const body = `${draft.body.slice(0, from)}${text}${draft.body.slice(caret)}`;
    setDraft(key, { body });
    setCompleting(null);

    // The caret belongs after what was just inserted, which React won't do on
    // its own once the value is controlled.
    requestAnimationFrame(() => {
      const at = from + text.length;
      field?.focus();
      field?.setSelectionRange(at, at);
    });
  }

  /** Work out whether a shortcode is being typed, and what it matches. */
  function updateCompletion(body: string, caret: number) {
    const typing = typingShortcode(body, caret);
    if (!typing) {
      setCompleting(null);
      return;
    }

    const matches = matchEmotes(emotes, typing.query);
    setCompleting(matches.length > 0 ? { start: typing.start, matches, active: 0 } : null);
  }

  async function sendSticker(image: PackImage) {
    setPickerAnchor(null);
    try {
      await ipc.sendSticker(
        roomId,
        {
          body: image.body,
          url: image.url,
          width: image.width,
          height: image.height,
          size: image.size,
          mimetype: image.mimetype,
        },
        threadRoot,
      );
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  /**
   * Attach something.
   *
   * On desktop this is the OS file dialog, which is what you want when the
   * thing you're sending is a file you can point at.
   *
   * On a phone it isn't: what you're sending is nearly always a photo you just
   * took, and a document browser is the long way round to it. A plain
   * `<input type="file">` in a web view *is* the native iOS sheet — Photo
   * Library, Take Photo or Video, Choose File — so the platform already has the
   * flow we want, and we only have to stop overriding it.
   *
   * `accept` is deliberately unset: iOS offers the photo and camera options
   * regardless, and filtering to `image/*,video/*` would take away the ability
   * to send any other kind of file from a phone at all.
   */
  async function attach() {
    // On a phone the sheet comes first: it shows the recent library inline, and
    // offers the camera and the file picker under it. Desktop has no library to
    // read and a perfectly good file dialog, so it goes straight there.
    if (isMobile) {
      setAttachOpen(true);
      return;
    }
    try {
      const selected = await open({ multiple: true });
      const paths = typeof selected === "string" ? [selected] : (selected ?? []);
      await uploadPaths(paths, roomId, threadRoot);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  /**
   * The picked files arrive as bytes rather than paths — the web view holds
   * them, not the filesystem — which is the same shape a paste arrives in, so
   * it takes the same route.
   */
  function onPicked(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? []);
    // Clear it, or picking the same photo twice in a row fires no change event
    // the second time and looks like the app ignored you.
    e.target.value = "";
    if (files.length) void uploadFiles(files, roomId, threadRoot);
  }

  // Paste a screenshot, an image, any file — it goes to this room.
  //
  // The listener is on the window rather than the textarea: pasting is a thing
  // you do *at the room*, and having it depend on whether the composer happens
  // to be focused would be a small mystery every time it didn't work. A paste
  // carrying no files isn't ours, so ordinary text paste is untouched.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const files = Array.from(e.clipboardData?.files ?? []);
      if (files.length === 0) return;

      e.preventDefault();
      void uploadFiles(files, roomId, threadRoot);
    };

    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [roomId, threadRoot]);

  return (
    // The safe-area inset is zero on desktop, so this is a no-op there and
    // keeps the composer clear of the home indicator on a phone.
    <div
      style={{
        position: "relative",
        padding: space(isMobile
          ? "10px 14px calc(var(--safe-bottom) + 8px)"
          : "12px 22px 18px"),
      }}
    >
      {completing && (
        <ShortcodeMenu
          matches={completing.matches}
          active={completing.active}
          onPick={(image) => insertAtCaret(`:${image.shortcode}: `, completing.start)}
        />
      )}

      {pickerAnchor && (
        <EmojiPicker
          anchor={pickerAnchor}
          packs={packs}
          stickers
          onPick={(picked) => {
            if (picked.kind === "sticker") {
              void sendSticker(picked.image);
              return;
            }
            setPickerAnchor(null);
            insertAtCaret(
              picked.kind === "unicode" ? picked.emoji : `:${picked.image.shortcode}: `,
            );
          }}
          onClose={() => setPickerAnchor(null)}
        />
      )}

      {(draft.replyTo || draft.editing) && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(10),
            padding: space("7px 14px"),
            marginBottom: 8,
            borderRadius: corner(14),
            background: "var(--surface-card)",
            border: "1px solid var(--border-subtle)",
            borderLeft: `2px solid ${draft.editing ? "var(--accent-primary)" : "var(--accent-tertiary)"}`,
          }}
        >
          <Icon
            name={draft.editing ? "pencil-simple" : "arrow-bend-up-left"}
            size={13}
            color={draft.editing ? "var(--accent-primary)" : "var(--accent-tertiary)"}
          />
          <span style={{ fontSize: textSize(12.5), color: "var(--text-secondary)" }}>
            {draft.editing ? "editing your message" : "replying"}
          </span>
          <button
            onClick={() => setDraft(key, { replyTo: null, editing: null, body: "" })}
            style={{ marginLeft: "auto", cursor: "pointer", display: "flex" }}
          >
            <Icon name="x" size={13} color="var(--text-tertiary)" />
          </button>
        </div>
      )}

      <div
        style={{
          display: "flex",
          alignItems: "flex-end",
          gap: space(10),
          background: "var(--surface-card-raised)",
          border: "1px solid var(--border-default)",
          borderRadius: corner(24),
          padding: space("9px 12px"),
          boxShadow: "var(--shadow-card)",
        }}
      >
        {/* Opened only by the button beside it. Rendered on every platform but
            used on none except touch, where it is the whole point — see
            `attach`. */}
        <input
          ref={fileInput}
          type="file"
          multiple
          onChange={onPicked}
          style={{ display: "none" }}
          aria-hidden
          tabIndex={-1}
        />
        {/* `capture` is what turns the same element into the camera rather than
            a picker, so "take a photo" opens the camera and nothing else. */}
        <input
          ref={cameraInput}
          type="file"
          accept="image/*,video/*"
          capture="environment"
          onChange={onPicked}
          style={{ display: "none" }}
          aria-hidden
          tabIndex={-1}
        />

        {attachOpen && (
          <AttachSheet
            roomId={roomId}
            threadRoot={threadRoot}
            onClose={() => setAttachOpen(false)}
            onCamera={() => {
              setAttachOpen(false);
              cameraInput.current?.click();
            }}
            onBrowse={() => {
              setAttachOpen(false);
              fileInput.current?.click();
            }}
          />
        )}

        {/* Same 38px box as the send button, so both line up on the composer's
            baseline however tall the textarea has grown. */}
        <button
          onClick={() => void attach()}
          title="attach a file"
          style={{
            width: 38,
            height: 38,
            flex: "none",
            borderRadius: 999,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            cursor: "pointer",
          }}
        >
          <Icon name="plus-circle" size={22} color="var(--text-tertiary)" />
        </button>

        <textarea
          ref={input}
          className="selectable"
          value={draft.body}
          rows={1}
          onChange={(e) => {
            setDraft(key, { body: e.target.value });
            updateCompletion(e.target.value, e.target.selectionStart);
            noteTyping();
            // Grow with the content, up to a point.
            e.target.style.height = "auto";
            e.target.style.height = `${Math.min(e.target.scrollHeight, 160)}px`;
          }}
          onKeyDown={(e) => {
            // While the shortcode menu is up it owns the keys that move through
            // it — otherwise Enter would send `:blobc` as a message.
            if (completing) {
              const { matches, active } = completing;

              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const next =
                  (active + (e.key === "ArrowDown" ? 1 : matches.length - 1)) % matches.length;
                setCompleting({ ...completing, active: next });
                return;
              }
              if (e.key === "Enter" || e.key === "Tab") {
                e.preventDefault();
                insertAtCaret(`:${matches[active].shortcode}: `, completing.start);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                setCompleting(null);
                return;
              }
            }

            // On an empty composer the arrows reach into the timeline: Up puts
            // your last message back in the box to edit, Down starts a reply to
            // the last thing somebody else said. Only when it's empty — once
            // there's text, the arrows are how you move around inside it.
            const recalls =
              !draft.body &&
              !draft.editing &&
              !e.shiftKey &&
              !e.metaKey &&
              !e.ctrlKey &&
              !e.altKey;

            if (recalls && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
              // Nothing to reach for: leave the key alone rather than
              // swallowing it for no reason.
              const took =
                e.key === "ArrowUp" ? editLastOwnMessage() : replyToLastOtherMessage();

              if (took) {
                e.preventDefault();
                return;
              }
            }

            // Enter sends; Shift+Enter breaks the line.
            const sends = e.key === "Enter" && !e.shiftKey && !e.metaKey;

            if (sends) {
              e.preventDefault();
              void submit();
              if (input.current) input.current.style.height = "auto";
            }
            if (e.key === "Escape" && (draft.replyTo || draft.editing)) {
              setDraft(key, { replyTo: null, editing: null, body: "" });
            }
          }}
          onKeyUp={(e) => updateCompletion(e.currentTarget.value, e.currentTarget.selectionStart)}
          onBlur={() => setCompleting(null)}
          placeholder={`say something cute in ${roomName}~`}
          style={{
            flex: 1,
            background: "transparent",
            border: "none",
            outline: "none",
            resize: "none",
            color: "var(--text-primary)",
            fontSize: "var(--text-message)",
            lineHeight: 1.5,
            padding: space("9px 6px"),
            maxHeight: 160,
          }}
        />

        <div style={{ display: "flex", alignItems: "center", gap: space(4) }}>
          <button
            ref={emojiButton}
            onClick={() =>
              setPickerAnchor((open) =>
                open ? null : (emojiButton.current?.getBoundingClientRect() ?? null),
              )
            }
            title="emoji and stickers"
            aria-label="emoji and stickers"
            style={{
              width: 34,
              height: 38,
              flex: "none",
              borderRadius: 999,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              cursor: "pointer",
            }}
          >
            <Icon name="smiley" size={20} color="var(--text-tertiary)" />
          </button>

          <button
            onClick={() => void submit()}
            // Keep the caret — and therefore the keyboard — in the composer.
            //
            // Pressing a button moves focus to it, and iOS drops the keyboard
            // the instant focus leaves a text field. Sending a message is
            // rarely the last thing you do, so losing the keyboard on every
            // send means reopening it for the next line. Blocking the default
            // on press keeps focus where it is; the click still fires.
            //
            // The shortcode menu does the same thing for the same reason.
            // Both events: WebKit is not consistent about whether blocking
            // `pointerdown` also suppresses the compatibility mouse event that
            // moves focus.
            onPointerDown={(e) => e.preventDefault()}
            onMouseDown={(e) => e.preventDefault()}
            disabled={!draft.body.trim()}
            title="send"
            style={{
              width: 38,
              height: 38,
              borderRadius: 999,
              background: "var(--accent-primary)",
              color: "var(--text-on-accent)",
              border: "2px solid var(--ink-950)",
              boxShadow: "var(--shadow-sticker-ink)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              cursor: draft.body.trim() ? "pointer" : "not-allowed",
              opacity: draft.body.trim() ? 1 : 0.45,
              transition: "transform var(--dur-fast) var(--ease-bounce)",
            }}
            onMouseEnter={(e) => {
              if (draft.body.trim()) {
                e.currentTarget.style.transform = "rotate(calc(-6deg * var(--tilt-scale))) scale(1.05)";
              }
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.transform = "";
            }}
          >
            <Icon name="paper-plane-right" size={17} color="var(--text-on-accent)" />
          </button>
        </div>
      </div>

      {/* Only the bad news. That a room is encrypted is already said by the
          lock in the header, and repeating it under every composer spends a
          line on the expected case — which also trains people to stop reading
          the line, exactly where the *unencrypted* warning has to land. */}
      {!encrypted && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(8),
            marginTop: 8,
            padding: space("0 6px"),
          }}
        >
          <Icon name="lock-key-open" size={11} color="var(--status-warning)" />
          <span
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: textSize(10.5),
              color: "var(--text-tertiary)",
            }}
          >
            not encrypted · your homeserver can read this
          </span>
        </div>
      )}
    </div>
  );
}

/**
 * The `:shortcode` menu, sitting above the composer.
 *
 * Mouse-down rather than click to choose: the textarea blurs on mouse-down, and
 * blur dismisses the menu, so waiting for the click would mean the menu is gone
 * before the click lands.
 */
function ShortcodeMenu({
  matches,
  active,
  onPick,
}: {
  matches: PackImage[];
  active: number;
  onPick: (image: PackImage) => void;
}) {
  return (
    <div
      role="listbox"
      aria-label="custom emotes"
      style={{
        position: "absolute",
        bottom: "100%",
        left: 22,
        marginBottom: 4,
        minWidth: 220,
        maxWidth: 320,
        padding: space(5),
        borderRadius: corner(16),
        background: "var(--surface-card-raised)",
        border: "1px solid var(--border-default)",
        boxShadow: "var(--shadow-pop)",
        zIndex: 30,
      }}
    >
      {matches.map((image, i) => (
        <div
          key={`${image.shortcode}-${image.url}`}
          role="option"
          aria-selected={i === active}
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(image);
          }}
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(9),
            padding: space("5px 9px"),
            borderRadius: corner(11),
            cursor: "pointer",
            background: i === active ? "rgba(255,255,255,.08)" : "transparent",
          }}
        >
          <EmoteThumb image={image} />
          <span
            className="uwu-ellipsis"
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: textSize(12),
              color: i === active ? "var(--text-primary)" : "var(--text-secondary)",
            }}
          >
            :{image.shortcode}:
          </span>
        </div>
      ))}
    </div>
  );
}

function EmoteThumb({ image }: { image: PackImage }) {
  const src = ipc.mediaUrl(image.url, { width: 44, height: 44 });
  if (!src) return <span style={{ width: 22 }} />;

  return (
    <img
      src={src}
      alt={image.shortcode}
      draggable={false}
      style={{ width: 22, height: 22, objectFit: "contain", flex: "none" }}
    />
  );
}
