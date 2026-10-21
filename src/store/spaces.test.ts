import { describe, expect, it } from "vitest";

import type { RoomSummary, SpaceSummary } from "../lib/bindings";
import { filterRooms, roomsInSpace, roomsOnlyInSpace } from "./index";

function room(id: string, over: Partial<RoomSummary> = {}): RoomSummary {
  return {
    id,
    name: id,
    membership: "joined",
    isSpace: false,
    isUtility: false,
    isMuted: false,
    parentSpaces: [],
    recency: 0,
    latest: null,
    ...over,
  } as RoomSummary;
}

function space(id: string, children: string[]): SpaceSummary {
  return { id, name: id, avatarUrl: null, children, notificationCount: 0, highlightCount: 0 };
}

/**
 * Leaving a space can take its rooms along, so which rooms count matters: a
 * room shared with another space must survive, or leaving one space quietly
 * empties a different one.
 */
describe("roomsOnlyInSpace", () => {
  const spaces = [space("!a", ["!solo", "!shared"]), space("!b", ["!shared"])];

  it("takes rooms only this space holds, and keeps shared ones", () => {
    const rooms = [room("!solo"), room("!shared"), room("!loose")];
    expect(roomsOnlyInSpace(rooms, spaces, "!a").map((r) => r.id)).toEqual(["!solo"]);
  });

  it("counts a room that only names the space as its parent", () => {
    const rooms = [room("!child", { parentSpaces: ["!a"] })];
    expect(roomsOnlyInSpace(rooms, spaces, "!a").map((r) => r.id)).toEqual(["!child"]);
  });

  it("treats a parent pointer to another space as shared", () => {
    const rooms = [room("!solo", { parentSpaces: ["!b"] })];
    expect(roomsOnlyInSpace(rooms, spaces, "!a")).toEqual([]);
  });

  it("skips invites, rooms already left, and sub-spaces", () => {
    const rooms = [
      room("!solo", { membership: "invited" }),
      room("!shared", { membership: "left" }),
      room("!sub", { isSpace: true, parentSpaces: ["!a"] }),
    ];
    expect(roomsOnlyInSpace(rooms, spaces, "!a")).toEqual([]);
  });
});

/** What the space menu's mute, notify and mark-read entries act on. */
describe("roomsInSpace", () => {
  const spaces = [space("!a", ["!child"])];

  it("includes rooms listed by the space and rooms that point back at it", () => {
    const rooms = [room("!child"), room("!pointer", { parentSpaces: ["!a"] }), room("!loose")];
    expect(roomsInSpace(rooms, spaces, "!a").map((r) => r.id)).toEqual(["!child", "!pointer"]);
  });

  it("is empty for a space we don't know", () => {
    expect(roomsInSpace([room("!child")], spaces, "!gone")).toEqual([]);
  });
});

describe("filterRooms with hideMuted", () => {
  const spaces = [space("!a", ["!loud", "!quiet"])];
  const rooms = [room("!loud"), room("!quiet", { isMuted: true }), room("!elsewhere", { isMuted: true })];
  const base = { filter: "all" as const, search: "", spaces, hideMuted: true };

  it("drops muted rooms inside the space", () => {
    const ids = filterRooms(rooms, { ...base, activeSpaceId: "!a" }).map((r) => r.id);
    expect(ids).toEqual(["!loud"]);
  });

  it("leaves \"everything\" alone, like Discord's per-server setting", () => {
    const ids = filterRooms(rooms, { ...base, activeSpaceId: null }).map((r) => r.id).sort();
    expect(ids).toEqual(["!elsewhere", "!loud", "!quiet"]);
  });
});
