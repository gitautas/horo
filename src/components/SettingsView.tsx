import { useEffect, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { getVersion } from "@tauri-apps/api/app";
import { open } from "@tauri-apps/plugin-dialog";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";

import { call } from "../lib/call";
import { useIsMobile } from "../lib/viewport";
import * as ipc from "../lib/ipc";
import { mediaUrl } from "../lib/ipc";
import { invalidateProfile } from "../lib/profiles";
import {
  ACCENT_LABELS,
  ACCENT_SWATCHES,
  DEFAULTS,
  FONT_PRESETS,
  listMediaDevices,
  MESSAGE_FONT_SIZE_RANGE,
  THEMES,
  ZOOM_RANGE,
  type Accent,
  type AudioDevice,
  type MediaDevices,
} from "../lib/settings";
import {
  CALL_SOUNDS,
  MESSAGE_SOUNDS,
  NONE,
  playCallSound,
  playMessageSound,
  type Sound,
} from "../lib/sounds";
import type { DeviceInfo, Profile, RecoveryStatus } from "../lib/types";
import { useStore } from "../store";
import { CameraBackdrop } from "./CameraBackdrop";
import { MicSettings } from "./MicSettings";
import { PacksSection } from "./PackSettings";
import { Card, Field, Heading, inputStyle, Row } from "./settingsUi";
import { Avatar, Button, dragRegion, Icon, Logo, RaveLabel, Spinner, Toggle } from "./ui";
import { UpdateSettings } from "./UpdateNotice";
import { corner, headingSize, space, textSize } from "../lib/scale";

type Section =
  | "account"
  | "security"
  | "voice"
  | "notifications"
  | "packs"
  | "appearance"
  | "about";

const SECTIONS: { id: Section; label: string; icon: string }[] = [
  { id: "account", label: "account", icon: "user-circle" },
  { id: "security", label: "security", icon: "shield-check" },
  { id: "voice", label: "voice & video", icon: "microphone" },
  { id: "notifications", label: "notifications", icon: "bell" },
  { id: "packs", label: "emotes & stickers", icon: "sticker" },
  { id: "appearance", label: "appearance", icon: "palette" },
  { id: "about", label: "about", icon: "info" },
];

/**
 * The full-window settings surface, in the shape people already know from
 * Discord: a nav rail of sections on the left, one pane of content on the
 * right, and Escape to get out.
 */
export function SettingsView() {
  const { open, close, session } = useStore(
    useShallow((s) => ({
      open: s.showSettings,
      close: s.closeSettings,
      session: s.session,
    })),
  );

  const [section, setSection] = useState<Section>("account");

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, close]);

  const isMobile = useIsMobile();

  if (!open || !session) return null;

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 120,
        display: "flex",
        // A 232px nav beside the content leaves 158px for the settings
        // themselves on a phone. Stack them instead: the nav becomes a strip
        // the sections scroll along horizontally.
        flexDirection: isMobile ? "column" : "row",
        background: "var(--surface-app)",
      }}
    >
      <div
        {...dragRegion(!isMobile)}
        style={{ position: "absolute", top: 0, left: 0, right: 0, height: 34 }}
      />

      {/* nav */}
      <div
        className="uwu-scroll"
        style={{
          width: isMobile ? "100%" : 232,
          flex: "none",
          // Not theme-scaled: the top clears the macOS window controls.
          padding: isMobile
            ? "calc(var(--safe-top) + 10px) 10px 8px"
            : "52px 12px 20px",
          background: "var(--ink-900)",
          borderRight: isMobile ? undefined : "1px solid var(--border-subtle)",
          borderBottom: isMobile ? "1px solid var(--border-subtle)" : undefined,
          display: isMobile ? "flex" : undefined,
          gap: isMobile ? space(6) : undefined,
          overflowX: isMobile ? "auto" : undefined,
          overflowY: isMobile ? "hidden" : undefined,
        }}
      >
        {!isMobile && <RaveLabel style={{ padding: space("0 10px 10px") }}>settings</RaveLabel>}
        {SECTIONS.map((entry) => (
          <button
            key={entry.id}
            onClick={() => setSection(entry.id)}
            style={{
              display: "flex",
              alignItems: "center",
              gap: space(10),
              width: isMobile ? "auto" : "100%",
              flex: isMobile ? "none" : undefined,
              whiteSpace: isMobile ? "nowrap" : undefined,
              padding: space("9px 12px"),
              marginBottom: isMobile ? 0 : 3,
              borderRadius: corner(12),
              cursor: "pointer",
              textAlign: "left",
              background: section === entry.id ? "rgba(255,255,255,.07)" : "transparent",
              color:
                section === entry.id ? "var(--text-primary)" : "var(--text-secondary)",
              fontFamily: "var(--font-heading)",
              fontWeight: 700,
              fontSize: headingSize(14),
            }}
          >
            <Icon
              name={entry.icon}
              size={16}
              color={
                section === entry.id ? "var(--accent-primary)" : "var(--text-tertiary)"
              }
            />
            {entry.label}
          </button>
        ))}
      </div>

      {/* content */}
      <div
        className="uwu-scroll"
        style={{
          flex: 1,
          padding: isMobile ? "20px 0 calc(var(--safe-bottom) + 32px)" : "52px 0 40px",
        }}
      >
        <div
          style={{
            maxWidth: 620,
            margin: "0 auto",
            padding: space(isMobile ? "0 16px" : "0 40px"),
          }}
        >
          {section === "account" && <AccountSection />}
          {section === "security" && <SecuritySection />}
          {section === "voice" && <VoiceSection />}
          {section === "notifications" && <NotificationsSection />}
          {section === "packs" && <PacksSection />}
          {section === "appearance" && <AppearanceSection />}
          {section === "about" && <AboutSection />}
        </div>
      </div>

      <button
        onClick={close}
        title="close settings (esc)"
        style={{
          position: "absolute",
          top: 46,
          right: 34,
          width: 38,
          height: 38,
          borderRadius: 999,
          border: "2px solid var(--border-default)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          cursor: "pointer",
        }}
      >
        <Icon name="x" size={16} color="var(--text-secondary)" />
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// layout helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// account
// ---------------------------------------------------------------------------

function AccountSection() {
  const { session, showBanner } = useStore(
    useShallow((s) => ({ session: s.session!, showBanner: s.showBanner })),
  );
  const [confirmingWipe, setConfirmingWipe] = useState(false);

  async function signOut(wipe: boolean) {
    try {
      await ipc.logout(wipe);
      // A sign-out invalidates everything the UI is holding, so start clean.
      window.location.reload();
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    }
  }

  return (
    <>
      <Heading>account</Heading>

      <Card>
        <div style={{ display: "flex", alignItems: "center", gap: space(16) }}>
          <Avatar
            id={session.userId}
            name={session.displayName ?? session.userId}
            mxc={session.avatarUrl}
            size={64}
            radius={22}
          />
          <div style={{ minWidth: 0 }}>
            <div
              style={{ fontFamily: "var(--font-heading)", fontWeight: 800, fontSize: headingSize(20) }}
            >
              {session.displayName ?? session.userId}
            </div>
            <div
              className="selectable"
              style={{
                fontFamily: "var(--font-mono)",
                fontSize: textSize(12),
                color: "var(--text-tertiary)",
              }}
            >
              {session.userId}
            </div>
          </div>
        </div>
      </Card>

      <Card>
        <Row title="homeserver" subtitle={session.homeserver} />
        <Row title="this device" subtitle={session.deviceId} />
      </Card>

      <Heading>profile</Heading>
      <ProfileEditor />

      {session.insecureStorage && (
        <Card tone="warning">
          <div style={{ display: "flex", gap: space(12) }}>
            <Icon name="warning" size={20} color="var(--status-warning)" />
            <div style={{ fontSize: textSize(13), color: "var(--text-secondary)", lineHeight: 1.55 }}>
              no system keychain was available, so your access token and encryption
              keys are in a file readable only by your user account. anyone who can
              read your home directory can read them.
            </div>
          </div>
        </Card>
      )}

      <Heading>sign out</Heading>
      <Card>
        <div style={{ fontSize: textSize(13), color: "var(--text-secondary)", lineHeight: 1.6 }}>
          signing out keeps your local encryption keys, so you can sign back in and
          still read your history.
        </div>
        <div style={{ display: "flex", gap: space(10), marginTop: 14, flexWrap: "wrap" }}>
          <Button variant="ghost" onClick={() => void signOut(false)}>
            sign out
          </Button>
          <Button
            variant={confirmingWipe ? "danger" : "ghost"}
            onClick={() => {
              if (confirmingWipe) void signOut(true);
              else setConfirmingWipe(true);
            }}
          >
            {confirmingWipe ? "yes — erase everything" : "sign out and erase keys"}
          </Button>
        </div>
        {confirmingWipe && (
          <div
            style={{
              marginTop: 12,
              fontSize: textSize(12.5),
              color: "var(--status-danger)",
              lineHeight: 1.55,
            }}
          >
            this deletes the encrypted store on this machine. any messages whose keys
            aren't in your key backup become permanently unreadable — including for
            you.
          </div>
        )}
      </Card>
    </>
  );
}

/**
 * Bio, status and cover photo — MSC4133 extended profile fields.
 *
 * Bio and status use Commet's keys so the two clients show the same thing;
 * `set_profile` handles the shape difference. Everything here is public and
 * federated, which the UI says rather than assuming people know.
 */
function ProfileEditor() {
  const showBanner = useStore((s) => s.showBanner);

  const [profile, setProfile] = useState<Profile | null>(null);
  const [bio, setBio] = useState("");
  const [status, setStatus] = useState("");
  const [coverUrl, setCoverUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    let cancelled = false;
    ipc
      .getProfile()
      .then((p) => {
        if (cancelled) return;
        setProfile(p);
        setBio(p.bio ?? "");
        setStatus(p.status ?? "");
        setCoverUrl(p.coverUrl ?? "");
      })
      .catch((e) => {
        if (!cancelled) showBanner("error", ipc.asUwuError(e).message);
      });
    return () => {
      cancelled = true;
    };
  }, [showBanner]);

  const dirty =
    profile !== null &&
    (bio !== (profile.bio ?? "") ||
      status !== (profile.status ?? "") ||
      coverUrl !== (profile.coverUrl ?? ""));

  async function pickCover() {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "image", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
      });
      if (typeof selected !== "string") return;
      setBusy(true);
      setCoverUrl(await ipc.uploadMedia(selected));
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function save() {
    setBusy(true);
    setSaved(false);
    try {
      // Only send what changed; a field left alone stays alone server-side.
      await ipc.setProfile({
        ...(bio !== (profile?.bio ?? "") ? { bio } : {}),
        ...(status !== (profile?.status ?? "") ? { status } : {}),
        ...(coverUrl !== (profile?.coverUrl ?? "") ? { coverUrl } : {}),
      });
      const updated = await ipc.getProfile();
      setProfile(updated);
      // Your card and the top of your DMs read from the cache; without this
      // they'd keep showing the old bio for the rest of the session.
      invalidateProfile(updated.userId);
      setSaved(true);
    } catch (e) {
      showBanner("error", ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  if (!profile) {
    return (
      <Card>
        <div style={{ display: "flex", justifyContent: "center", padding: space(20) }}>
          <Spinner />
        </div>
      </Card>
    );
  }

  const coverPreview = mediaUrl(coverUrl, { width: 560, height: 140 });

  return (
    <Card>
      <Field label="cover photo" hint="shown behind your avatar on your profile card.">
        <div
          style={{
            height: 96,
            borderRadius: corner(16),
            marginBottom: 10,
            border: "1px solid var(--border-subtle)",
            background: coverPreview
              ? `center/cover no-repeat url("${coverPreview}")`
              : "var(--surface-inset)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "var(--text-tertiary)",
            fontSize: textSize(12.5),
          }}
        >
          {!coverPreview && "no cover yet"}
        </div>
        <div style={{ display: "flex", gap: space(8) }}>
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void pickCover()}>
            choose an image
          </Button>
          {coverUrl && (
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setCoverUrl("")}>
              remove
            </Button>
          )}
        </div>
      </Field>

      <Field label="status" hint="a short line — what you're up to.">
        <input
          className="selectable"
          value={status}
          maxLength={120}
          onChange={(e) => setStatus(e.target.value)}
          placeholder="vibing~"
          style={inputStyle}
        />
      </Field>

      <Field label="bio">
        <textarea
          className="selectable"
          value={bio}
          rows={4}
          maxLength={1000}
          onChange={(e) => setBio(e.target.value)}
          placeholder="say something about yourself~"
          style={{ ...inputStyle, resize: "vertical", lineHeight: 1.55 }}
        />
      </Field>

      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: space(12),
          marginTop: 4,
        }}
      >
        <Button disabled={!dirty || busy} onClick={() => void save()}>
          save
        </Button>
        {busy && <Spinner size={14} />}
        {saved && !dirty && (
          <span style={{ fontSize: textSize(12.5), color: "var(--accent-primary)" }}>saved~</span>
        )}
        <span
          style={{
            marginLeft: "auto",
            fontSize: textSize(11.5),
            color: "var(--text-tertiary)",
            textAlign: "right",
            lineHeight: 1.45,
          }}
        >
          public and federated — anyone
          <br />
          who can see you can read this
        </span>
      </div>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// security
// ---------------------------------------------------------------------------

function SecuritySection() {
  const showBanner = useStore((s) => s.showBanner);

  const [devices, setDevices] = useState<DeviceInfo[] | null>(null);
  const [recovery, setRecovery] = useState<RecoveryStatus | null>(null);
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const [restoreKey, setRestoreKey] = useState("");
  const [busy, setBusy] = useState(false);

  function refresh() {
    ipc.getOwnDevices().then(setDevices).catch(() => setDevices([]));
    ipc.getRecoveryStatus().then(setRecovery).catch(() => setRecovery(null));
  }

  useEffect(refresh, []);

  async function run(label: string, action: () => Promise<void>) {
    setBusy(true);
    try {
      await action();
    } catch (e) {
      showBanner("error", `${label}: ${ipc.asUwuError(e).message}`);
    } finally {
      setBusy(false);
      refresh();
    }
  }

  return (
    <>
      <Heading>your devices</Heading>
      <Card>
        <div
          style={{
            fontSize: textSize(13),
            color: "var(--text-secondary)",
            lineHeight: 1.6,
            marginBottom: 6,
          }}
        >
          verifying a device proves it's really yours and lets your encrypted history
          reach it. unverified devices show a warning beside their messages.
        </div>

        {!devices && (
          <div style={{ display: "flex", justifyContent: "center", padding: space(20) }}>
            <Spinner />
          </div>
        )}

        {devices?.map((device) => (
          <Row
            key={device.deviceId}
            icon={device.isCurrent ? "desktop" : "devices"}
            title={
              <span style={{ display: "flex", alignItems: "center", gap: space(8) }}>
                {device.displayName ?? device.deviceId}
                {device.isCurrent && (
                  <span
                    style={{
                      fontFamily: "var(--font-rave)",
                      fontSize: textSize(8.5),
                      fontWeight: 800,
                      letterSpacing: "0.1em",
                      textTransform: "uppercase",
                      color: "var(--accent-quaternary)",
                    }}
                  >
                    this one
                  </span>
                )}
              </span>
            }
            subtitle={device.deviceId}
          >
            {device.isVerified ? (
              <span
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: space(6),
                  color: "var(--accent-primary)",
                  fontSize: textSize(12.5),
                }}
              >
                <Icon name="seal-check" size={15} color="var(--accent-primary)" />
                verified
              </span>
            ) : device.isCurrent ? (
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() =>
                  void run("verification", () => ipc.requestVerification())
                }
              >
                verify from another device
              </Button>
            ) : (
              <Button
                size="sm"
                disabled={busy}
                onClick={() =>
                  void run("verification", () => ipc.verifyDevice(device.deviceId))
                }
              >
                verify
              </Button>
            )}
          </Row>
        ))}
      </Card>

      <Heading>key backup</Heading>
      <Card>
        {recovery ? (
          <>
            <Row
              icon={recovery.state === "enabled" ? "key" : "key-return"}
              title={
                recovery.state === "enabled"
                  ? "recovery is on"
                  : recovery.state === "incomplete"
                    ? "recovery is half set up"
                    : "recovery is off"
              }
              subtitle={
                recovery.crossSigningReady
                  ? "cross-signing ready"
                  : "cross-signing not set up"
              }
            />
            {recovery.state === "incomplete" ? (
              <div
                style={{
                  margin: "14px 0",
                  padding: space(14),
                  borderRadius: corner(16),
                  background: "color-mix(in srgb, var(--amber-500) 8%, transparent)",
                  border: "1px solid color-mix(in srgb, var(--amber-500) 35%, transparent)",
                  fontSize: textSize(13),
                  color: "var(--text-secondary)",
                  lineHeight: 1.6,
                }}
              >
                <strong style={{ color: "var(--status-warning)" }}>
                  this is why your old messages are unreadable.
                </strong>{" "}
                you have a key backup on the server, but this device doesn't have the
                key to open it. paste your recovery key below and your history will
                decrypt.
              </div>
            ) : (
              <div
                style={{
                  fontSize: textSize(13),
                  color: "var(--text-secondary)",
                  lineHeight: 1.6,
                  margin: "14px 0",
                }}
              >
                key backup stores your message keys on the server, encrypted with a
                recovery key only you hold. without it, losing this device means
                losing your encrypted history.
              </div>
            )}

            {recoveryKey ? (
              <div
                style={{
                  padding: space(16),
                  borderRadius: corner(16),
                  background: "var(--ink-950)",
                  border: "1px solid var(--accent-primary)",
                }}
              >
                <RaveLabel style={{ marginBottom: 8 }}>
                  write this down — you'll only see it once
                </RaveLabel>
                <div
                  className="selectable"
                  style={{
                    fontFamily: "var(--font-mono)",
                    fontSize: textSize(14),
                    color: "var(--accent-primary)",
                    wordBreak: "break-all",
                    lineHeight: 1.6,
                  }}
                >
                  {recoveryKey}
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  style={{ marginTop: 12 }}
                  onClick={() => setRecoveryKey(null)}
                >
                  i've saved it
                </Button>
              </div>
            ) : (
              <div style={{ display: "flex", gap: space(10), flexWrap: "wrap" }}>
                {recovery.state !== "enabled" && (
                  <Button
                    disabled={busy}
                    onClick={() =>
                      void run("key backup", async () => {
                        setRecoveryKey(await ipc.enableRecovery());
                      })
                    }
                  >
                    turn on key backup
                  </Button>
                )}
              </div>
            )}

            <div style={{ marginTop: 18 }}>
              <Field
                label="restore from a recovery key"
                hint="paste the key you saved when you turned on backup."
              >
                <div style={{ display: "flex", gap: space(8) }}>
                  <input
                    className="selectable"
                    value={restoreKey}
                    onChange={(e) => setRestoreKey(e.target.value)}
                    placeholder="EsT... "
                    spellCheck={false}
                    style={inputStyle}
                  />
                  <Button
                    disabled={busy || !restoreKey.trim()}
                    onClick={() =>
                      void run("restore", async () => {
                        await ipc.recoverWithKey(restoreKey.trim());
                        setRestoreKey("");
                      })
                    }
                  >
                    restore
                  </Button>
                </div>
              </Field>
            </div>
          </>
        ) : (
          <div style={{ display: "flex", justifyContent: "center", padding: space(20) }}>
            <Spinner />
          </div>
        )}
      </Card>
    </>
  );
}

// ---------------------------------------------------------------------------
// voice
// ---------------------------------------------------------------------------

/**
 * What a device picker shows: the saved device if it's still plugged in,
 * otherwise whichever one the system default is right now. There's no
 * "system default" entry; leaving the picker alone is how you get that.
 */
function shownDevice(saved: string, available: AudioDevice[], systemDefault: string | null) {
  if (saved && available.some((d) => d.deviceId === saved)) return saved;
  return systemDefault ?? available[0]?.deviceId ?? "";
}

function VoiceSection() {
  const { settings, updateSettings } = useStore(
    useShallow((s) => ({ settings: s.settings, updateSettings: s.updateSettings })),
  );

  const [devices, setDevices] = useState<MediaDevices | null>(null);

  useEffect(() => {
    let cancelled = false;
    listMediaDevices()
      .then((found) => {
        if (!cancelled) setDevices(found);
      })
      .catch(() => {
        if (!cancelled) setDevices({
            inputs: [],
            outputs: [],
            cameras: [],
            defaults: { input: null, output: null, camera: null },
          });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const spinner = (
    <div style={{ display: "flex", justifyContent: "center", padding: space(20) }}>
      <Spinner />
    </div>
  );
  const missing = (what: string) => (
    <div style={{ fontSize: textSize(12.5), color: "var(--status-warning)" }}>
      no {what} found — check that horo has permission in system settings.
    </div>
  );

  return (
    <>
      <Heading>audio</Heading>
      <Card>
        {!devices ? (
          spinner
        ) : (
          <>
            <Field label="output device" hint="where you hear everyone else.">
              <select
                value={shownDevice(settings.audioOutput, devices.outputs, devices.defaults.output)}
                onChange={(e) => {
                  updateSettings({ audioOutput: e.target.value });
                  void call.setAudioOutput(e.target.value);
                }}
                style={inputStyle}
              >
                {devices.outputs.map((device) => (
                  <option key={device.deviceId} value={device.deviceId}>
                    {device.label}
                  </option>
                ))}
              </select>
            </Field>

            <Field
              label="input device"
              hint="the microphone other people hear. changing this mid-call switches it live."
            >
              <select
                value={shownDevice(settings.audioInput, devices.inputs, devices.defaults.input)}
                onChange={(e) => {
                  updateSettings({ audioInput: e.target.value });
                  void call.setAudioInput(e.target.value);
                }}
                style={inputStyle}
              >
                {devices.inputs.map((device) => (
                  <option key={device.deviceId} value={device.deviceId}>
                    {device.label}
                  </option>
                ))}
              </select>
            </Field>

            <MicSettings />

            {devices.inputs.length === 0 && missing("microphones")}
          </>
        )}
      </Card>

      <Heading>video</Heading>
      <Card
        backdrop={
          devices && devices.cameras.length > 0 ? (
            <CameraBackdrop deviceId={settings.videoInput} />
          ) : undefined
        }
      >
        {!devices ? (
          spinner
        ) : (
          <div
            style={{
              // Room for the picture: the camera field sits along the bottom
              // like a video tile's controls, with your face above it.
              ...(devices.cameras.length > 0 && {
                minHeight: 220,
                display: "flex",
                flexDirection: "column",
                justifyContent: "flex-end",
              }),
            }}
          >
            <Field
              label="camera"
              hint="used when you turn video on in a call. changing this mid-call switches it live."
            >
              <select
                value={shownDevice(settings.videoInput, devices.cameras, devices.defaults.camera)}
                onChange={(e) => {
                  updateSettings({ videoInput: e.target.value });
                  void call.setVideoInput(e.target.value);
                }}
                style={inputStyle}
              >
                {devices.cameras.map((device) => (
                  <option key={device.deviceId} value={device.deviceId}>
                    {device.label}
                  </option>
                ))}
              </select>
            </Field>

            {devices.cameras.length === 0 && missing("cameras")}
          </div>
        )}
      </Card>
    </>
  );
}

// ---------------------------------------------------------------------------
// notifications
// ---------------------------------------------------------------------------

function NotificationsSection() {
  const { settings, updateSettings } = useStore(
    useShallow((s) => ({ settings: s.settings, updateSettings: s.updateSettings })),
  );

  // `null` while we're still asking the platform.
  const [permitted, setPermitted] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;
    isPermissionGranted()
      .then((granted) => {
        if (!cancelled) setPermitted(granted);
      })
      .catch(() => {
        if (!cancelled) setPermitted(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function askPermission() {
    try {
      setPermitted((await requestPermission()) === "granted");
    } catch {
      setPermitted(false);
    }
  }

  return (
    <>
      <Heading>notifications</Heading>

      {permitted === false && (
        <Card tone="warning">
          <div style={{ fontSize: textSize(13), color: "var(--text-secondary)", lineHeight: 1.6 }}>
            your system isn't letting horo show banners yet. sounds still work — but
            for pop-ups, allow notifications for horo in system settings.
          </div>
          <div style={{ marginTop: 12 }}>
            <Button variant="ghost" size="sm" onClick={askPermission}>
              ask again
            </Button>
          </div>
        </Card>
      )}

      <Card>
        <Row
          icon="chat-circle-dots"
          title="messages"
          subtitle="uses your account's push rules, so muted rooms stay quiet"
        >
          <Toggle
            on={settings.notifyMessages}
            onToggle={(next) => updateSettings({ notifyMessages: next })}
          />
        </Row>
        <Row icon="phone-call" title="calls" subtitle="when a call starts in one of your rooms">
          <Toggle
            on={settings.notifyCalls}
            onToggle={(next) => updateSettings({ notifyCalls: next })}
          />
        </Row>
        <Row
          icon="eye"
          title="notify while i'm here"
          subtitle="off: no banner for the room you're already reading"
        >
          <Toggle
            on={settings.notifyWhenFocused}
            onToggle={(next) => updateSettings({ notifyWhenFocused: next })}
          />
        </Row>
      </Card>

      <Heading>sounds</Heading>
      <Card>
        <SoundPicker
          label="message sound"
          hint="played when a message arrives you'd be pinged for."
          sounds={MESSAGE_SOUNDS}
          value={settings.messageSound}
          onChange={(messageSound) => updateSettings({ messageSound })}
          preview={(id) => playMessageSound(id, settings.notificationVolume)}
        />

        <SoundPicker
          label="call sound"
          hint="played when someone starts a call."
          sounds={CALL_SOUNDS}
          value={settings.callSound}
          onChange={(callSound) => updateSettings({ callSound })}
          preview={(id) => playCallSound(id, settings.notificationVolume)}
        />

        <Field
          label={`volume — ${Math.round(settings.notificationVolume * 100)}%`}
          hint="how loud the two sounds above are. doesn't touch call audio."
        >
          <input
            type="range"
            min={0}
            max={100}
            value={Math.round(settings.notificationVolume * 100)}
            onChange={(e) =>
              updateSettings({ notificationVolume: Number(e.target.value) / 100 })
            }
            // Release rather than every frame: dragging the slider shouldn't
            // fire a chirp per pixel.
            onPointerUp={() =>
              playMessageSound(settings.messageSound, settings.notificationVolume)
            }
            style={{ width: "100%", accentColor: "var(--accent-primary)" }}
          />
        </Field>
      </Card>
    </>
  );
}

/** A sound chosen from the built-in set, with a button to hear it first. */
function SoundPicker({
  label,
  hint,
  sounds,
  value,
  onChange,
  preview,
}: {
  label: string;
  hint: string;
  sounds: Sound[];
  value: string;
  onChange: (id: string) => void;
  preview: (id: string) => void;
}) {
  return (
    <Field label={label} hint={hint}>
      <div style={{ display: "flex", gap: space(10) }}>
        <select
          value={value}
          onChange={(e) => {
            onChange(e.target.value);
            // Picking a sound is the moment you want to hear it.
            preview(e.target.value);
          }}
          style={{ ...inputStyle, flex: 1 }}
        >
          {sounds.map((sound) => (
            <option key={sound.id} value={sound.id}>
              {sound.label}
            </option>
          ))}
          <option value={NONE}>silent</option>
        </select>
        <Button
          variant="ghost"
          size="sm"
          disabled={value === NONE}
          onClick={() => preview(value)}
        >
          play
        </Button>
      </div>
    </Field>
  );
}

// ---------------------------------------------------------------------------
// appearance
// ---------------------------------------------------------------------------

/** For naming the modifier in shortcut hints. */
const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

function AppearanceSection() {
  const { settings, updateSettings } = useStore(
    useShallow((s) => ({
      settings: s.settings,
      updateSettings: s.updateSettings,
    })),
  );

  return (
    <>
      <Heading>appearance</Heading>

      <Card>
        <Field label="theme">
          <div style={{ display: "flex", flexWrap: "wrap", gap: space(12) }}>
            {THEMES.map((theme) => (
              <ThemeTile
                key={theme.name}
                {...theme}
                active={settings.theme === theme.name}
                onPick={() => updateSettings({ theme: theme.name })}
              />
            ))}
          </div>
        </Field>

        <Field label="accent" hint="the colour of buttons, badges and highlights.">
          <div style={{ display: "flex", gap: space(12) }}>
            {ACCENT_SWATCHES.map(({ name, colour }) => (
              <button
                key={name}
                onClick={() => updateSettings({ accent: name as Accent })}
                title={ACCENT_LABELS[settings.theme]?.[name] ?? name}
                style={{
                  width: 54,
                  height: 54,
                  borderRadius: corner(18),
                  background: colour,
                  cursor: "pointer",
                  border:
                    settings.accent === name
                      ? "3px solid var(--text-primary)"
                      : "2px solid var(--ink-950)",
                  boxShadow:
                    settings.accent === name ? "var(--shadow-sticker-ink)" : "none",
                  transform: settings.accent === name ? "rotate(calc(-3deg * var(--tilt-scale)))" : "none",
                  transition: "all var(--dur-fast) var(--ease-bounce)",
                }}
              />
            ))}
          </div>
        </Field>
      </Card>

      <Card>
        <Field
          label={`zoom — ${Math.round(settings.zoom * 100)}%`}
          hint={`the size of everything in the window. ${isMac ? "⌘" : "ctrl"} + and ${isMac ? "⌘" : "ctrl"} − do the same, ${isMac ? "⌘" : "ctrl"} 0 resets.`}
        >
          <div style={{ display: "flex", alignItems: "center", gap: space(12) }}>
            <input
              type="range"
              min={ZOOM_RANGE.min}
              max={ZOOM_RANGE.max}
              step={ZOOM_RANGE.step}
              value={settings.zoom}
              onChange={(e) => updateSettings({ zoom: Number(e.target.value) })}
              style={{ flex: 1, accentColor: "var(--accent-primary)" }}
            />
            <Button
              variant="ghost"
              size="sm"
              disabled={settings.zoom === DEFAULTS.zoom}
              onClick={() => updateSettings({ zoom: DEFAULTS.zoom })}
            >
              reset
            </Button>
          </div>
        </Field>
      </Card>

      <Card>
        <FontPicker
          value={settings.fontFamily}
          onChange={(fontFamily) => updateSettings({ fontFamily })}
        />

        <Field
          label={`message text — ${settings.messageFontSize}px`}
          hint="the size of messages and the composer. the rest of the app stays put."
        >
          <div style={{ display: "flex", alignItems: "center", gap: space(12) }}>
            <input
              type="range"
              min={MESSAGE_FONT_SIZE_RANGE.min}
              max={MESSAGE_FONT_SIZE_RANGE.max}
              step={MESSAGE_FONT_SIZE_RANGE.step}
              value={settings.messageFontSize}
              onChange={(e) => updateSettings({ messageFontSize: Number(e.target.value) })}
              style={{ flex: 1, accentColor: "var(--accent-primary)" }}
            />
            <Button
              variant="ghost"
              size="sm"
              disabled={settings.messageFontSize === DEFAULTS.messageFontSize}
              onClick={() => updateSettings({ messageFontSize: DEFAULTS.messageFontSize })}
            >
              reset
            </Button>
          </div>
        </Field>

        <div
          style={{
            background: "var(--surface-inset)",
            border: "1px solid var(--border-subtle)",
            borderRadius: corner(14),
            padding: space("12px 14px"),
            fontSize: "var(--text-message)",
            lineHeight: 1.55,
          }}
        >
          the quick brown fox jumps over the lazy dog~ ✨
        </div>
      </Card>
    </>
  );
}

/**
 * A theme, drawn in its own colours. The tile carries `data-theme`, which is
 * all a theme stylesheet matches on, so its tokens resolve to that theme's
 * values whichever one the rest of the app is using.
 */
function ThemeTile({
  name,
  label,
  hint,
  active,
  onPick,
}: {
  name: string;
  label: string;
  hint: string;
  active: boolean;
  onPick: () => void;
}) {
  return (
    <button
      data-theme={name}
      onClick={onPick}
      aria-pressed={active}
      style={{
        flex: "1 1 180px",
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "flex-start",
        gap: space(8),
        padding: space(14),
        borderRadius: corner(18),
        background: "var(--ink-950)",
        border: active ? "2px solid var(--text-primary)" : "2px solid var(--border-default)",
        boxShadow: active ? "var(--shadow-sticker-ink)" : "none",
        cursor: "pointer",
        textAlign: "left",
        transition: "all var(--dur-fast) var(--ease-bounce)",
      }}
    >
      <span
        style={{
          fontFamily: "var(--font-heading)",
          fontWeight: 800,
          fontSize: headingSize(17),
          color: "var(--text-primary)",
        }}
      >
        {label}
      </span>
      <span style={{ fontSize: textSize(12), color: "var(--text-secondary)" }}>{hint}</span>
      <span style={{ display: "flex", gap: space(6), marginTop: 2 }}>
        {["--acid-500", "--pink-500", "--violet-500", "--cyan-500", "--ink-700"].map((token) => (
          <span
            key={token}
            style={{
              width: 18,
              height: 18,
              borderRadius: corner(7),
              background: `var(${token})`,
              border: "1px solid var(--border-subtle)",
            }}
          />
        ))}
      </span>
    </button>
  );
}

/**
 * The body font: a row of presets, and a box for the name of anything else
 * installed. The box keeps its own draft so typing doesn't repaint the whole
 * app with every half-written font name.
 */
function FontPicker({
  value,
  onChange,
}: {
  value: string;
  onChange: (family: string) => void;
}) {
  const preset = FONT_PRESETS.find((p) => p.family === value);
  const [draft, setDraft] = useState(preset ? "" : value);

  // Picking a preset clears the box; a value set elsewhere shows up in it.
  useEffect(() => {
    setDraft(FONT_PRESETS.some((p) => p.family === value) ? "" : value);
  }, [value]);

  const commit = () => {
    const next = draft.trim();
    if (next !== value) onChange(next);
  };

  return (
    <Field
      label="font"
      hint="type the name of any font installed on this machine. one that isn't falls back to the theme's own."
    >
      <div style={{ display: "flex", flexWrap: "wrap", gap: space(8), marginBottom: 10 }}>
        {FONT_PRESETS.map((p) => {
          const active = p.family === value;
          return (
            <button
              key={p.label}
              onClick={() => onChange(p.family)}
              style={{
                padding: space("7px 14px"),
                borderRadius: 999,
                cursor: "pointer",
                fontFamily: p.family || "var(--font-body-default)",
                fontSize: textSize(13.5),
                fontWeight: 700,
                color: active ? "var(--text-on-accent)" : "var(--text-primary)",
                background: active ? "var(--accent-primary)" : "var(--surface-inset)",
                border: `1px solid ${active ? "var(--accent-primary)" : "var(--border-subtle)"}`,
              }}
            >
              {p.label}
            </button>
          );
        })}
      </div>
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
        placeholder="custom font, e.g. Comic Sans MS"
        spellCheck={false}
        style={{ ...inputStyle, fontFamily: draft.trim() || undefined }}
      />
    </Field>
  );
}

// ---------------------------------------------------------------------------
// about
// ---------------------------------------------------------------------------

function AboutSection() {
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion, () => {});
  }, []);

  return (
    <>
      <Heading>about</Heading>
      <Card>
        <Logo height={32} />
        <div style={{ color: "var(--text-secondary)", fontSize: textSize(13.5), marginTop: 10 }}>
          a cute matrix client~
        </div>
        <div
          style={{
            marginTop: 16,
            fontFamily: "var(--font-mono)",
            fontSize: textSize(11.5),
            color: "var(--text-tertiary)",
            lineHeight: 1.9,
          }}
        >
          <div>version {version || "…"}</div>
          <div>matrix-rust-sdk 0.18 · e2ee via megolm</div>
          <div>voice over matrixrtc + livekit</div>
        </div>
      </Card>

      <UpdateSettings />
    </>
  );
}
