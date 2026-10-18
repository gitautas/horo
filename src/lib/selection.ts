/**
 * Keep double/triple-click selections inside the text they started in.
 *
 * The body is `user-select: none` with `.selectable` islands, and WebKit's
 * word/paragraph granularity doesn't treat an island's edge as a boundary:
 * multi-clicking the last message runs the selection on through the
 * unselectable chrome into the next selectable thing — the composer — so ⌘C
 * copies the draft along with the message. A drag (single click) is left
 * alone, since selecting across several messages on purpose is fine.
 */
export function installSelectionClamp(): void {
  document.addEventListener("mouseup", (e) => {
    if (e.detail < 2) return;
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return;

    const anchor = sel.anchorNode;
    const el = anchor instanceof Element ? anchor : anchor?.parentElement;
    const box = el?.closest(".selectable");
    if (!box) return;

    const range = sel.getRangeAt(0);
    const startsInside = box.contains(range.startContainer);
    const endsInside = box.contains(range.endContainer);
    if (startsInside && endsInside) return;

    if (!startsInside) range.setStart(box, 0);
    if (!endsInside) range.setEnd(box, box.childNodes.length);
    sel.removeAllRanges();
    sel.addRange(range);
  });
}
