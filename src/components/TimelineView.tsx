import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { formatDayDivider, joinNames, localpart } from "../lib/display";
import { cachedProfile, loadProfile } from "../lib/profiles";
import { dismissKeyboard, useIsMobile } from "../lib/viewport";
import type { TimelineItem, TypingUser } from "../lib/types";
import { useStore } from "../store";
import { MessageRow } from "./MessageRow";
import { Avatar, Spinner } from "./ui";
import { space, textSize } from "../lib/scale";

/**
 * Shared empties for rooms we haven't loaded yet.
 *
 * These must be stable references: zustand compares snapshots by identity, so a
 * fresh `[]` inside the selector would look like a change on every read and
 * re-render forever.
 */
const NO_ITEMS: TimelineItem[] = [];
const NO_TYPING: TypingUser[] = [];

/** Consecutive messages from one person within this window share a header. */
const GROUPING_WINDOW_MS = 5 * 60 * 1000;

/** How close to the top counts as "load more". */
const PAGINATE_THRESHOLD_PX = 240;

/** Ceiling on the initial "fill the viewport" pagination, so it can't spin. */
const MAX_FILL_PAGES = 8;

export function TimelineView({
  roomId,
  threadRoot,
}: {
  roomId: string;
  threadRoot?: string;
}) {
  const isMobile = useIsMobile();
  const key = threadRoot ? `${roomId}|${threadRoot}` : roomId;

  const {
    items,
    ready,
    typing,
    paginating,
    exhausted,
    loadOlder,
    setDraft,
    openThread,
  } = useStore(
    useShallow((s) => ({
      items: s.timelines[key] ?? NO_ITEMS,
      // The room is selected before `open_timeline` resolves; until the entry
      // exists there is no timeline on the Rust side to paginate.
      ready: s.timelines[key] !== undefined,
      typing: s.typing[roomId] ?? NO_TYPING,
      paginating: s.paginating[key] ?? false,
      exhausted: s.exhausted[key] ?? false,
      loadOlder: s.loadOlder,
      setDraft: s.setDraft,
      openThread: s.openThread,
    })),
  );

  const scroller = useRef<HTMLDivElement>(null);
  /** True while the user is parked at the bottom, so new messages should follow. */
  const stuckToBottom = useRef(true);
  /**
   * The row we hold still, and where it sat at the previous commit.
   *
   * This is scroll anchoring, done by hand because WebKit has none. Two earlier
   * attempts failed for the same reason: they tried to identify *which* change
   * should move the viewport — a pagination, and only a pagination. But the
   * spinner that appears above the list while loading shifts every row down as
   * surely as a prepend does, and so does an image finishing, and a name
   * resolving. Deciding which shifts to correct is guesswork.
   *
   * So this corrects all of them. The rule is simply: the row the reader is
   * looking at does not move. Whatever happened above it — content, spinner,
   * a picture growing — is cancelled out.
   *
   * It is a list, not one row: the rows on screen, top first. A pagination can
   * remove the row the reader is on — the day divider at the very top is
   * dropped and re-inserted above the older messages under a new id — and with
   * a single anchor that left nothing to correct against, so the viewport
   * stayed at the top and asked for another page, and another. The first
   * candidate that survives the commit is the one held still.
   */
  const anchorIds = useRef<string[]>([]);
  /** Where those rows were *before* the commit now being applied. See below. */
  const anchorWas = useRef<Map<string, number> | null>(null);
  const lastKey = useRef(key);

  /** A row's top edge, relative to the top of the scrolling viewport. */
  function offsetOf(el: HTMLElement, row: HTMLElement): number {
    return row.getBoundingClientRect().top - el.getBoundingClientRect().top;
  }

  function rowById(el: HTMLElement, id: string): HTMLElement | null {
    return el.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`);
  }

  /**
   * The rows on screen, topmost first.
   *
   * Partially visible counts — that row is what the eye is on, and taking the
   * first *fully* visible one lets the anchor drift by up to a row.
   */
  function visibleRows(el: HTMLElement): HTMLElement[] {
    const visible: HTMLElement[] = [];
    for (const row of el.querySelectorAll<HTMLElement>("[data-row-id]")) {
      const top = offsetOf(el, row);
      if (top >= el.clientHeight) break;
      if (top + row.offsetHeight > 0) visible.push(row);
    }
    return visible;
  }

  // Read, during render, where the anchor sits *before* React commits.
  //
  // This is the one measurement a `useLayoutEffect` cannot take: by the time it
  // runs the DOM has already changed, and the old position is gone. Class
  // components have `getSnapshotBeforeUpdate` for exactly this; with hooks the
  // render pass is the only moment left. Reading layout here is safe — it is a
  // read, it writes nothing, and it happens before any mutation.
  if (!stuckToBottom.current && scroller.current && anchorIds.current.length) {
    const el = scroller.current;
    const was = new Map<string, number>();
    for (const id of anchorIds.current) {
      const row = rowById(el, id);
      if (row) was.set(id, offsetOf(el, row));
    }
    anchorWas.current = was;
  } else {
    anchorWas.current = null;
  }

  // Jump to the newest message when switching rooms.
  useEffect(() => {
    if (lastKey.current !== key) {
      lastKey.current = key;
      stuckToBottom.current = true;
      anchorIds.current = [];
      anchorWas.current = null;
    }
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [key]);

  // Applied after every commit, not just the ones we think matter.
  //
  // No dependency array on purpose: any render can move things above the
  // reader, and the pre-commit snapshot above is only valid for the commit that
  // immediately follows it. When nothing moved the delta is zero and this
  // writes nothing, so the cost of running always is a single measurement.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;

    // Parked at the bottom wins: at room open the filling pass prepends while
    // we are pinned there, and following the newest message is what the reader
    // asked for by staying put.
    if (stuckToBottom.current) {
      el.scrollTop = el.scrollHeight;
      anchorIds.current = [];
      return;
    }

    const before = anchorWas.current;
    anchorWas.current = null;
    if (!before) return;

    for (const [id, was] of before) {
      const row = rowById(el, id);
      if (!row) continue;
      const drift = offsetOf(el, row) - was;
      if (drift !== 0) el.scrollTop += drift;
      return;
    }
  });

  // Keep loading until the timeline actually overflows.
  //
  // Scroll-triggered pagination can't bootstrap itself: a room that opens with
  // one message isn't scrollable, so the user has no way to ask for more. This
  // fills the viewport first, after which scrolling takes over. `exhausted`
  // ends it at the start of the room, and the attempt cap stops a room whose
  // history is all filtered-out events from looping forever.
  /**
   * Ask for older messages, at most one request at a time.
   *
   * The store's `paginating` flag only reaches this component on the *next*
   * render, so every scroll event fired in between still sees `false` and asks
   * again. The store dedupes, so the extra calls were harmless — but they are
   * noise on every scroll to the top, and a local ref is the honest guard.
   */
  const fetching = useRef(false);

  const paginate = useCallback(() => {
    if (fetching.current) return;
    fetching.current = true;
    void loadOlder().finally(() => {
      fetching.current = false;
    });
  }, [loadOlder]);

  const fillAttempts = useRef(0);

  useEffect(() => {
    fillAttempts.current = 0;
  }, [key]);

  useEffect(() => {
    const el = scroller.current;
    if (!el || !ready || paginating || exhausted) return;
    if (fillAttempts.current >= MAX_FILL_PAGES) return;

    const overflows = el.scrollHeight > el.clientHeight + 40;
    if (overflows) return;

    fillAttempts.current += 1;
    paginate();
  }, [items, ready, paginating, exhausted, paginate, key]);

  function onScroll() {
    const el = scroller.current;
    if (!el) return;

    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    stuckToBottom.current = distanceFromBottom < 80;

    // Whatever is at the top of the viewport now is what must stay there
    // through the next commit. Re-picking on every scroll is what keeps the
    // correction from ever fighting the reader: if they moved, the anchor moved
    // with them, and the next delta is zero.
    anchorIds.current = visibleRows(el).map((row) => row.dataset.rowId!);

    if (
      el.scrollTop < PAGINATE_THRESHOLD_PX &&
      ready &&
      !paginating &&
      !exhausted
    ) {
      paginate();
    }
  }

  const rows = withGrouping(items.filter(isWorthShowing));

  return (
    <div
      ref={scroller}
      onScroll={onScroll}
      className="uwu-scroll"
      // Scrolling back through the conversation dismisses the keyboard, the way
      // a native list does. Harmless on desktop, where nothing is focused by
      // touch and there is no keyboard to drop.
      onTouchMove={dismissKeyboard}
      style={{
        flex: 1,
        // 22px each side is a fifth of a phone's width spent on margin.
        padding: space(isMobile ? "12px 12px 8px" : "18px 22px 8px"),
        display: "flex",
        flexDirection: "column",
        gap: space(3),
      }}
    >
      {paginating && (
        <div
          style={{
            display: "flex",
            justifyContent: "center",
            padding: space("10px 0"),
          }}
        >
          <Spinner />
        </div>
      )}

      {exhausted && (
        <div
          style={{
            alignSelf: "center",
            padding: space("14px 0 18px"),
            fontSize: textSize(12.5),
            color: "var(--text-tertiary)",
          }}
        >
          this is the very beginning~
        </div>
      )}

      {rows.map(({ item, showHeader }) => {
        if (item.kind === "dateDivider") {
          return (
            <div
              key={item.id}
              data-row-id={item.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: space(12),
                margin: "18px 0 10px",
              }}
            >
              <span
                style={{ flex: 1, height: 1, background: "var(--border-subtle)" }}
              />
              <span
                style={{
                  fontSize: textSize(11),
                  fontWeight: 600,
                  color: "var(--text-tertiary)",
                  whiteSpace: "nowrap",
                }}
              >
                {formatDayDivider(item.timestamp ?? 0)}
              </span>
              <span
                style={{ flex: 1, height: 1, background: "var(--border-subtle)" }}
              />
            </div>
          );
        }

        if (item.kind === "readMarker") {
          return (
            <div
              key={item.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: space(10),
                margin: "8px 0",
              }}
            >
              <span
                style={{
                  flex: 1,
                  height: 1,
                  background: "var(--accent-secondary)",
                }}
              />
              <span
                style={{
                  fontFamily: "var(--font-rave)",
                  fontSize: textSize(8.5),
                  fontWeight: 800,
                  letterSpacing: "0.14em",
                  textTransform: "uppercase",
                  color: "var(--accent-secondary)",
                }}
              >
                new
              </span>
            </div>
          );
        }

        if (item.kind !== "event" || !item.event) return null;

        return (
          <MessageRow
            key={item.id}
            rowId={item.id}
            item={item.event}
            roomId={roomId}
            threadRoot={threadRoot}
            showHeader={showHeader}
            onReply={(eventId) =>
              setDraft(key, { replyTo: eventId, editing: null })
            }
            onOpenThread={(root) => void openThread(root)}
          />
        );
      })}

      <ReadReceipts items={items} />

      {typing.length > 0 && (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: space(9),
            padding: `${space("2px 8px 10px")} calc(40px + ${space(20)})`,
            color: "var(--text-secondary)",
            fontSize: textSize(12.5),
          }}
        >
          <span style={{ display: "inline-flex", gap: space(3) }}>
            {[0, 0.2, 0.4].map((delay) => (
              <span
                key={delay}
                style={{
                  width: 5,
                  height: 5,
                  borderRadius: "50%",
                  background: "var(--accent-secondary)",
                  animation: `uwuPulse 1s ${delay}s infinite`,
                }}
              />
            ))}
          </span>
          <span>
            {joinNames(typing.map((u) => u.displayName ?? localpart(u.userId)))}{" "}
            {typing.length === 1 ? "is" : "are"} typing~
          </span>
        </div>
      )}
    </div>
  );
}

/** How many faces the pile shows before it's just a number. */
const MAX_FACES = 5;

/** The little facepile under the last message. Exported for its test. */
export function ReadReceipts({ items }: { items: TimelineItem[] }) {
  const lastEvent = [...items].reverse().find((i) => i.kind === "event")?.event;
  const receipts = lastEvent?.readReceipts ?? [];

  // Anyone who has spoken in view already carries their picture on their
  // messages, so their face costs nothing. Built for the whole timeline rather
  // than per-receipt so five readers make one pass, not five.
  const seen = useMemo(() => {
    const known = new Map<string, { name: string | null; avatar: string | null }>();
    for (const item of items) {
      const event = item.event;
      if (event && !known.has(event.sender)) {
        known.set(event.sender, { name: event.senderName, avatar: event.senderAvatar });
      }
    }
    return known;
  }, [items]);

  if (receipts.length === 0) return null;

  const overflow = receipts.length - MAX_FACES;

  // Just the faces. The count lives on the label for screen readers and the
  // hover tooltip, and only shows up on screen once the pile runs out of room.
  return (
    <div
      role="group"
      aria-label={`seen by ${receipts.length}`}
      title={`seen by ${receipts.length}`}
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(8),
        padding: `${space("4px 8px 10px")} calc(40px + ${space(20)})`,
      }}
    >
      {receipts.slice(0, MAX_FACES).map((userId) => (
        <ReceiptFace key={userId} userId={userId} known={seen.get(userId)} />
      ))}
      {overflow > 0 && (
        <span
          style={{
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          +{overflow}
        </span>
      )}
    </div>
  );
}

/**
 * One face in the pile.
 *
 * A read receipt is a bare user ID — the timeline carries nothing else about
 * the reader — which is why this used to draw a coloured monogram and nothing
 * more. Whoever is on screen supplies most of them for free; the rest come from
 * the profile cache, which is one request per person per five minutes and
 * usually zero, since these are the same few people over and over.
 */
function ReceiptFace({
  userId,
  known,
}: {
  userId: string;
  known?: { name: string | null; avatar: string | null };
}) {
  const [fetched, setFetched] = useState(() => cachedProfile(userId));

  useEffect(() => {
    // Nothing to look up for someone whose message is right there.
    if (known?.avatar) return;

    let live = true;
    // A profile that won't load leaves the monogram in place, which is what
    // this drew before and is a perfectly good answer.
    loadProfile(userId)
      .then((profile) => live && setFetched(profile))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [userId, known?.avatar]);

  const name = known?.name ?? fetched?.displayName ?? localpart(userId);

  return (
    <span title={name} style={{ display: "flex", marginLeft: -6 }}>
      <Avatar
        id={userId}
        name={name}
        mxc={known?.avatar ?? fetched?.avatarUrl}
        size={18}
        radius={9}
        fontSize={8}
        style={{ borderWidth: 1.5 }}
      />
    </span>
  );
}

/**
 * State events that are pure call bookkeeping.
 *
 * MatrixRTC republishes membership on join, leave and every periodic refresh,
 * so a single call can produce dozens of these. They say nothing a person wants
 * to read — the call UI already shows who's in it.
 */
const CALL_STATE_EVENTS = new Set([
  "org.matrix.msc3401.call.member",
  "m.rtc.member",
  "m.call.member",
]);

function isWorthShowing(item: TimelineItem): boolean {
  const content = item.event?.content;
  if (content?.kind !== "state") return true;
  return !CALL_STATE_EVENTS.has(content.eventType);
}

/**
 * Decide which messages get a full header (avatar + name + time) and which
 * continue the run above them.
 */
function withGrouping(
  items: TimelineItem[],
): { item: TimelineItem; showHeader: boolean }[] {
  let previousSender: string | null = null;
  let previousTimestamp = 0;

  return items.map((item) => {
    if (item.kind !== "event" || !item.event) {
      previousSender = null;
      return { item, showHeader: false };
    }

    const event = item.event;

    // System lines break a run — a "joined" note between two messages means the
    // second one deserves its own header again.
    const isSystem =
      event.content.kind === "membership" ||
      event.content.kind === "profileChange" ||
      event.content.kind === "state";

    if (isSystem) {
      previousSender = null;
      return { item, showHeader: false };
    }

    const showHeader =
      event.sender !== previousSender ||
      event.timestamp - previousTimestamp > GROUPING_WINDOW_MS ||
      event.reply != null;

    previousSender = event.sender;
    previousTimestamp = event.timestamp;

    return { item, showHeader };
  });
}
