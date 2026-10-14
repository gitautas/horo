/**
 * Making and editing custom emote and sticker packs.
 *
 * Every pack lives in a room — that's the whole of Matrix's model — and the
 * only real difference between "mine" and "ours" is who else is in that room.
 * A pack of your own gets a private room made for it, which is why there's no
 * limit on how many you can have; a pack in a room you share is the room's, and
 * editing it needs the power level to send state there.
 *
 * Accounts that used an older client may also have the MSC's single personal
 * pack in account data. It's shown and editable when it exists, but nothing
 * here makes a new one: it can't be duplicated, and rooms can.
 *
 * Every change is one edit sent to the backend, which reads the pack and writes
 * it back, rather than this screen posting a whole pack it snapshotted a minute
 * ago. Two people tidying the same pack shouldn't erase each other.
 */

import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import * as ipc from "../lib/ipc";
import { mediaUrl } from "../lib/ipc";
import type { ImagePack, PackImage, PackRoom, PackTarget } from "../lib/types";
import { useStore } from "../store";
import { Card, Heading, inputStyle } from "./settingsUi";
import { Button, Icon, RaveLabel, Spinner, Toggle } from "./ui";
import { corner, headingSize, space, textSize } from "../lib/scale";

export function PacksSection() {
  const showBanner = useStore((s) => s.showBanner);
  const activeRoomId = useStore((s) => s.activeRoomId);
  const loadPacks = useStore((s) => s.loadPacks);

  const [packs, setPacks] = useState<ImagePack[] | null>(null);
  const [rooms, setRooms] = useState<PackRoom[]>([]);
  const [busy, setBusy] = useState(false);

  async function refresh() {
    try {
      const [all, editable] = await Promise.all([
        ipc.getAllImagePacks(),
        ipc.getPackRooms(),
      ]);
      setPacks(all);
      setRooms(editable);
    } catch (e) {
      setPacks([]);
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  // Once, when the screen opens. Every edit below re-reads for itself, so there
  // is nothing here that should re-run on a render.
  useEffect(() => {
    void refresh();
  }, []);

  /**
   * Run one edit, then re-read.
   *
   * The picker's copy of the packs is refreshed too, so a shortcode added here
   * is usable in the room behind this screen without a reload.
   */
  async function run(action: () => Promise<void>) {
    setBusy(true);
    try {
      await action();
      await refresh();
      void loadPacks(activeRoomId);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  // The personal pack only exists on accounts that have used a client which
  // makes one; there's no way to create another, so it's shown when it's there
  // and never advertised when it isn't.
  const legacyPack = packs?.find((p) => p.source === "user" && p.images.length > 0);
  const roomPacks = packs?.filter((p) => p.source === "room") ?? [];

  return (
    <>
      <Heading>emotes &amp; stickers</Heading>

      {packs === null ? (
        <div style={{ display: "flex", justifyContent: "center", padding: space(40) }}>
          <Spinner />
        </div>
      ) : (
        <>
          {roomPacks.length === 0 && !legacyPack ? (
            <Card>
              <div style={{ fontSize: textSize(13), color: "var(--text-secondary)", lineHeight: 1.55 }}>
                no packs yet. make as many as you like — one for your own images, or
                one in a room to share with everyone in it.
              </div>
            </Card>
          ) : (
            <>
              {roomPacks.map((pack) => (
                <PackCard key={pack.id} pack={pack} busy={busy} onRun={run} />
              ))}
              {legacyPack && <PackCard pack={legacyPack} busy={busy} onRun={run} />}
            </>
          )}

          <NewPack rooms={rooms} busy={busy} onRun={run} />
        </>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// one pack
// ---------------------------------------------------------------------------

function PackCard({
  pack,
  busy,
  onRun,
}: {
  pack: ImagePack;
  busy: boolean;
  onRun: (action: () => Promise<void>) => Promise<void>;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [name, setName] = useState(pack.displayName);

  const target: PackTarget = { roomId: pack.roomId, stateKey: pack.stateKey };
  const locked = !pack.canEdit;

  return (
    <Card>
      <div style={{ display: "flex", alignItems: "center", gap: space(10), marginBottom: 14 }}>
        <Icon
          name={pack.source === "user" ? "user-circle" : "users-three"}
          size={17}
          color="var(--accent-primary)"
        />

        {locked ? (
          <div style={{ flex: 1, fontFamily: "var(--font-heading)", fontWeight: 700, fontSize: headingSize(15) }}>
            {pack.displayName}
          </div>
        ) : (
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => {
              if (name !== pack.displayName) {
                void onRun(() => ipc.editImagePack(target, { kind: "setName", name }));
              }
            }}
            aria-label="pack name"
            style={{
              ...inputStyle,
              flex: 1,
              padding: space("6px 11px"),
              fontFamily: "var(--font-display)",
              fontWeight: 700,
              fontSize: textSize(15),
            }}
          />
        )}

        {pack.source === "room" && pack.roomId && pack.stateKey !== null && (
          <Toggle
            on={pack.everywhere}
            onToggle={(on) =>
              void onRun(() =>
                ipc.setPackEverywhere(pack.roomId!, pack.stateKey!, on),
              )
            }
            label="use everywhere"
          />
        )}
      </div>

      {pack.source === "room" && (
        <div
          style={{
            marginTop: -8,
            marginBottom: 12,
            fontSize: textSize(12),
            color: "var(--text-tertiary)",
            lineHeight: 1.5,
          }}
        >
          {locked
            ? "you can use this pack, but you can't change it — that needs permission to send state in that room."
            : "shared with everyone in that room."}{" "}
          {pack.everywhere
            ? "on in every room."
            : "on in that room only, until you turn it on everywhere."}
        </div>
      )}

      {pack.images.length === 0 ? (
        <div style={{ fontSize: textSize(13), color: "var(--text-tertiary)", padding: space("4px 0 12px") }}>
          nothing in here yet~
        </div>
      ) : (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(56px, 1fr))",
            gap: space(6),
            marginBottom: 12,
          }}
        >
          {pack.images.map((image) => (
            <button
              key={image.shortcode}
              onClick={() => setEditing(editing === image.shortcode ? null : image.shortcode)}
              title={`:${image.shortcode}:`}
              aria-label={`:${image.shortcode}:`}
              disabled={locked}
              style={{
                height: 56,
                borderRadius: corner(14),
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                overflow: "hidden",
                cursor: locked ? "default" : "pointer",
                background:
                  editing === image.shortcode
                    ? "color-mix(in srgb, var(--accent-primary) 16%, transparent)"
                    : "var(--surface-inset)",
                border:
                  editing === image.shortcode
                    ? "1px solid var(--accent-primary)"
                    : "1px solid var(--border-subtle)",
              }}
            >
              <Thumb image={image} size={38} />
            </button>
          ))}
        </div>
      )}

      {editing && !locked && (
        <ImageEditor
          key={editing}
          image={pack.images.find((i) => i.shortcode === editing)!}
          busy={busy}
          onRun={onRun}
          target={target}
          onDone={() => setEditing(null)}
        />
      )}

      {!locked && (
        <AddImages target={target} busy={busy} onRun={onRun} />
      )}
    </Card>
  );
}

function ImageEditor({
  image,
  target,
  busy,
  onRun,
  onDone,
}: {
  image: PackImage;
  target: PackTarget;
  busy: boolean;
  onRun: (action: () => Promise<void>) => Promise<void>;
  onDone: () => void;
}) {
  const [shortcode, setShortcode] = useState(image.shortcode);

  /** Change what this image is usable as, keeping everything else about it. */
  function setUsage(isEmoticon: boolean, isSticker: boolean) {
    void onRun(() =>
      ipc.editImagePack(target, {
        kind: "putImage",
        shortcode: image.shortcode,
        url: image.url,
        body: image.body,
        isEmoticon,
        isSticker,
        width: image.width,
        height: image.height,
        size: image.size,
        mimetype: image.mimetype,
      }),
    );
  }

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(10),
        padding: space(12),
        marginBottom: 12,
        borderRadius: corner(16),
        background: "var(--surface-inset)",
        border: "1px solid var(--border-subtle)",
        flexWrap: "wrap",
      }}
    >
      <Thumb image={image} size={34} />

      <div style={{ display: "flex", alignItems: "center", gap: space(4), flex: 1, minWidth: 160 }}>
        <span style={{ fontFamily: "var(--font-mono)", color: "var(--text-tertiary)" }}>:</span>
        <input
          value={shortcode}
          onChange={(e) => setShortcode(e.target.value)}
          onBlur={() => {
            const next = shortcode.trim();
            if (next && next !== image.shortcode) {
              void onRun(async () => {
                await ipc.editImagePack(target, {
                  kind: "rename",
                  from: image.shortcode,
                  to: next,
                });
                onDone();
              });
            }
          }}
          aria-label="shortcode"
          style={{ ...inputStyle, padding: space("6px 10px"), fontFamily: "var(--font-mono)" }}
        />
        <span style={{ fontFamily: "var(--font-mono)", color: "var(--text-tertiary)" }}>:</span>
      </div>

      <UsagePill
        label="emote"
        on={image.isEmoticon}
        onClick={() => setUsage(!image.isEmoticon, image.isSticker)}
      />
      <UsagePill
        label="sticker"
        on={image.isSticker}
        onClick={() => setUsage(image.isEmoticon, !image.isSticker)}
      />

      <button
        onClick={() =>
          void onRun(async () => {
            await ipc.editImagePack(target, {
              kind: "removeImage",
              shortcode: image.shortcode,
            });
            onDone();
          })
        }
        disabled={busy}
        title={`remove :${image.shortcode}:`}
        aria-label={`remove :${image.shortcode}:`}
        style={{ cursor: "pointer", display: "flex", padding: space(6) }}
      >
        <Icon name="trash" size={15} color="var(--status-danger)" />
      </button>
    </div>
  );
}

function UsagePill({
  label,
  on,
  onClick,
}: {
  label: string;
  on: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      role="switch"
      aria-checked={on}
      aria-label={label}
      style={{
        padding: space("5px 12px"),
        borderRadius: 999,
        cursor: "pointer",
        fontFamily: "var(--font-mono)",
        fontSize: textSize(11.5),
        color: on ? "var(--accent-primary)" : "var(--text-tertiary)",
        background: on
          ? "color-mix(in srgb, var(--accent-primary) 14%, transparent)"
          : "transparent",
        border: `1px solid ${on ? "var(--accent-primary)" : "var(--border-subtle)"}`,
      }}
    >
      {label}
    </button>
  );
}

/**
 * Upload files and add them to a pack.
 *
 * The shortcode starts as the filename, which is nearly always what someone
 * wants — `blobcat.png` becomes `:blobcat:` — and is editable straight after.
 * Dimensions are read back from the uploaded image rather than asked for: other
 * clients use them to size stickers, and nobody wants to type them in.
 */
function AddImages({
  target,
  busy,
  onRun,
}: {
  target: PackTarget;
  busy: boolean;
  onRun: (action: () => Promise<void>) => Promise<void>;
}) {
  return (
    <Button
      variant="ghost"
      disabled={busy}
      onClick={() =>
        void onRun(async () => {
          const chosen = await open({
            multiple: true,
            filters: [{ name: "images", extensions: ["png", "gif", "webp", "jpg", "jpeg"] }],
          });
          const paths = typeof chosen === "string" ? [chosen] : (chosen ?? []);

          for (const path of paths) {
            const url = await ipc.uploadMedia(path);
            const size = await measure(url);

            await ipc.editImagePack(target, {
              kind: "putImage",
              shortcode: shortcodeFromPath(path),
              url,
              isEmoticon: true,
              isSticker: true,
              width: size?.width ?? null,
              height: size?.height ?? null,
              mimetype: mimeFromPath(path),
            });
          }
        })
      }
    >
      <Icon name="plus" size={14} color="var(--text-secondary)" /> add images
    </Button>
  );
}

// ---------------------------------------------------------------------------
// making one
// ---------------------------------------------------------------------------

function NewPack({
  rooms,
  busy,
  onRun,
}: {
  rooms: PackRoom[];
  busy: boolean;
  onRun: (action: () => Promise<void>) => Promise<void>;
}) {
  const [roomId, setRoomId] = useState("");
  const [name, setName] = useState("");
  const [mineName, setMineName] = useState("");

  return (
    <>
      <Card>
        <RaveLabel style={{ marginBottom: 10 }}>new pack of your own</RaveLabel>
        <div style={{ display: "flex", gap: space(8), flexWrap: "wrap" }}>
          <input
            value={mineName}
            onChange={(e) => setMineName(e.target.value)}
            placeholder="pack name"
            aria-label="new personal pack name"
            style={{ ...inputStyle, flex: 1, minWidth: 150 }}
          />
          <Button
            disabled={busy || !mineName.trim()}
            onClick={() =>
              void onRun(async () => {
                await ipc.createPersonalPack(mineName.trim());
                setMineName("");
              })
            }
          >
            make it~
          </Button>
        </div>
        <div
          style={{
            marginTop: 10,
            fontSize: textSize(12),
            color: "var(--text-tertiary)",
            lineHeight: 1.5,
          }}
        >
          as many as you like. each one gets a private room to live in — matrix keeps
          packs in rooms — and is turned on everywhere straight away.
        </div>
      </Card>

      {rooms.length > 0 && <NewRoomPack rooms={rooms} busy={busy} onRun={onRun} roomId={roomId} setRoomId={setRoomId} name={name} setName={setName} />}
    </>
  );
}

function NewRoomPack({
  rooms,
  busy,
  onRun,
  roomId,
  setRoomId,
  name,
  setName,
}: {
  rooms: PackRoom[];
  busy: boolean;
  onRun: (action: () => Promise<void>) => Promise<void>;
  roomId: string;
  setRoomId: (id: string) => void;
  name: string;
  setName: (name: string) => void;
}) {
  return (
    <Card>
      <RaveLabel style={{ marginBottom: 10 }}>new pack in a room</RaveLabel>
      <div style={{ display: "flex", gap: space(8), flexWrap: "wrap" }}>
        <select
          value={roomId}
          onChange={(e) => setRoomId(e.target.value)}
          aria-label="room for the new pack"
          style={{ ...inputStyle, flex: 1, minWidth: 150 }}
        >
          <option value="">pick a room…</option>
          {rooms.map((room) => (
            <option key={room.id} value={room.id}>
              {room.name}
            </option>
          ))}
        </select>

        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="pack name"
          aria-label="new pack name"
          style={{ ...inputStyle, flex: 1, minWidth: 150 }}
        />

        <Button
          disabled={busy || !roomId || !name.trim()}
          onClick={() =>
            void onRun(async () => {
              // The state key is what makes packs in one room distinct, and
              // it's never shown — derived from the name so it's readable in a
              // state-event dump rather than a random string.
              await ipc.editImagePack(
                { roomId, stateKey: stateKeyFor(name) },
                { kind: "setName", name: name.trim() },
              );
              setName("");
            })
          }
        >
          make it~
        </Button>
      </div>
      <div
        style={{
          marginTop: 10,
          fontSize: textSize(12),
          color: "var(--text-tertiary)",
          lineHeight: 1.5,
        }}
      >
        everyone in the room gets these, and other matrix clients can read them too.
      </div>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function Thumb({ image, size }: { image: PackImage; size: number }) {
  const src = mediaUrl(image.url, { width: size * 2, height: size * 2 });
  if (!src) return <span style={{ fontSize: textSize(10) }}>{image.shortcode}</span>;

  return (
    <img
      src={src}
      alt={image.shortcode}
      loading="lazy"
      draggable={false}
      style={{ width: size, height: size, objectFit: "contain", flex: "none" }}
    />
  );
}

/** The filename, reduced to something typeable between colons. */
export function shortcodeFromPath(path: string): string {
  const base = path.split(/[/\\]/).pop() ?? path;
  const stem = base.replace(/\.[^.]+$/, "");
  const cleaned = stem.toLowerCase().replace(/[^a-z0-9_+-]+/g, "_").replace(/^_+|_+$/g, "");
  return cleaned || "emote";
}

function mimeFromPath(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase();
  const known: Record<string, string> = {
    png: "image/png",
    gif: "image/gif",
    webp: "image/webp",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
  };
  return ext ? (known[ext] ?? null) : null;
}

/** A readable, unique-enough state key for a new pack. */
export function stateKeyFor(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || `pack-${Date.now().toString(36)}`;
}

/** Read an uploaded image's dimensions back, or nothing if it won't load. */
function measure(mxc: string): Promise<{ width: number; height: number } | null> {
  const src = mediaUrl(mxc);
  if (!src) return Promise.resolve(null);

  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    img.onerror = () => resolve(null);
    img.src = src;
  });
}
