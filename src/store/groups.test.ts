import { describe, expect, it } from "vitest";

import type { PublicRoom, RoomSummary } from "../lib/bindings";
import { groupRooms, withUnjoined } from "./index";

function room(id: string, over: Partial<RoomSummary> = {}): RoomSummary {
  return {
    id,
    name: id,
    membership: "joined",
    isDirect: false,
    isVideoRoom: false,
    ...over,
  } as RoomSummary;
}

const titles = (rooms: RoomSummary[], order?: string[]) =>
  groupRooms(rooms, order).map((g) => [g.title, g.rooms.map((r) => r.id)]);

describe("groupRooms", () => {
  it("puts voice rooms in their own section", () => {
    expect(titles([room("general"), room("hangout", { isVideoRoom: true })])).toEqual([
      ["rooms", ["general"]],
      ["voice", ["hangout"]],
    ]);
  });

  it("keeps rooms in place however recently they were active", () => {
    // Arrives most recent first, the way the SDK sorts it.
    const rooms = [room("memes"), room("art"), room("general")];
    expect(titles(rooms)).toEqual([["rooms", ["art", "general", "memes"]]]);
  });

  it("follows the space's order, then names for rooms it doesn't list", () => {
    const rooms = [room("a"), room("z"), room("m"), room("b")];
    expect(titles(rooms, ["z", "m"])).toEqual([["rooms", ["z", "m", "a", "b"]]]);
  });

  it("leaves direct messages most recent first", () => {
    const rooms = [room("kai", { isDirect: true }), room("ann", { isDirect: true })];
    expect(titles(rooms)).toEqual([["direct messages", ["kai", "ann"]]]);
  });
});

describe("withUnjoined", () => {
  const preview = (id: string, over: Partial<PublicRoom> = {}) =>
    ({ id, name: id, isSpace: false, isVoice: false, via: [], ...over }) as PublicRoom;
  const sections = (groups: ReturnType<typeof withUnjoined>) =>
    groups.map((g) => [g.title, g.rooms.map((r) => r.id), (g.unjoined ?? []).map((r) => r.id)]);

  it("lists them after the rooms already joined, in the space's order", () => {
    const groups = groupRooms([room("general")]);
    expect(sections(withUnjoined(groups, [preview("b"), preview("a")], ["b", "a"]))).toEqual([
      ["rooms", ["general"], ["b", "a"]],
    ]);
  });

  it("puts voice rooms with the voice rooms", () => {
    const groups = groupRooms([room("general"), room("hangout", { isVideoRoom: true })]);
    expect(sections(withUnjoined(groups, [preview("gaming", { isVoice: true })]))).toEqual([
      ["rooms", ["general"], []],
      ["voice", ["hangout"], ["gaming"]],
    ]);
  });

  it("brings a section along when nothing in it is joined yet", () => {
    const groups = groupRooms([room("kai", { isDirect: true }), room("hangout", { isVideoRoom: true })]);
    expect(sections(withUnjoined(groups, [preview("general")]))).toEqual([
      ["direct messages", ["kai"], []],
      ["rooms", [], ["general"]],
      ["voice", ["hangout"], []],
    ]);
  });
});
