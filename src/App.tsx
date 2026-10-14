import { useCallback, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { useShallow } from "zustand/react/shallow";

import * as ipc from "./lib/ipc";
import { startNotifications } from "./lib/notify";
import { startRinger } from "./lib/ringer";
import { startUpdateChecks } from "./lib/update";
import { resetPresence } from "./lib/presence";
import { applyAccent, applyTypography, load as loadSettings } from "./lib/settings";
import type { RoomSummary } from "./lib/types";
import { useEdgeSwipeBack, useIsMobile, useKeyboardSafeArea } from "./lib/viewport";
import { selectActiveRoom, useStore } from "./store";
import { ChatPane, EmptyPane } from "./components/ChatPane";
import { CreateRoom } from "./components/CreateRoom";
import { DropZone } from "./components/DropZone";
import { Lightbox } from "./components/Lightbox";
import { LoginScreen } from "./components/LoginScreen";
import { ProfileCard } from "./components/ProfileCard";
import { RoomInfo } from "./components/RoomInfo";
import { RoomList } from "./components/RoomList";
import { SettingsView } from "./components/SettingsView";
import { SpacesRail } from "./components/SpacesRail";
import { IncomingCallModal } from "./components/IncomingCallModal";
import { UpdateBanner } from "./components/UpdateNotice";
import { VerificationModal } from "./components/VerificationModal";
import { BackdropPattern, dragRegion, Icon, Logo, Spinner } from "./components/ui";

export default function App() {
  const { session, bootstrapped, setSession, setBootstrapped, showBanner } = useStore(
    useShallow((s) => ({
      session: s.session,
      bootstrapped: s.bootstrapped,
      setSession: s.setSession,
      setBootstrapped: s.setBootstrapped,
      showBanner: s.showBanner,
    })),
  );

  // Paint the saved accent and fonts before anything renders, so the app never
  // flashes the defaults on the way to the chosen ones.
  useEffect(() => {
    const saved = loadSettings();
    applyAccent(saved.accent);
    applyTypography(saved);
  }, []);

  // Try to pick up where we left off before showing anything, so a returning
  // user never sees the login screen flash past.
  useEffect(() => {
    let cancelled = false;

    ipc
      .restoreSession()
      .then((restored) => {
        if (cancelled) return;
        if (restored) setSession(restored);
      })
      .catch((e) => {
        if (!cancelled) showBanner("error", ipc.asUwuError(e).message);
      })
      .finally(() => {
        if (!cancelled) setBootstrapped(true);
      });

    return () => {
      cancelled = true;
    };
  }, [setSession, setBootstrapped, showBanner]);

  if (!bootstrapped) return <Booting />;
  if (!session) return <LoginScreen />;
  return <Shell />;
}

function Booting() {
  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 18,
        background: "var(--surface-app)",
        position: "relative",
        overflow: "hidden",
      }}
    >
      <BackdropPattern />
      <div style={{ position: "relative", zIndex: 1 }} className="uwu-wobble">
        <Logo height={44} />
      </div>
      <div style={{ position: "relative", zIndex: 1 }}>
        <Spinner size={20} />
      </div>
    </div>
  );
}

function Shell() {
  const { activeRoomId, showInfo, banner, dismissBanner } = useStore(
    useShallow((s) => ({
      activeRoomId: s.activeRoomId,
      showInfo: s.showInfo,
      banner: s.banner,
      dismissBanner: s.dismissBanner,
    })),
  );
  const activeRoom = useStore(selectActiveRoom);
  const isMobile = useIsMobile();

  useKeyboardSafeArea(isMobile);
  useBackendEvents();
  useAppShortcuts();
  useNotifications();
  usePresenceReporter();

  return (
    <div
      style={{
        display: "flex",
        // `dvh` rather than `vh`: on iOS the visual viewport shrinks when the
        // keyboard comes up, and `vh` doesn't notice — the composer would sit
        // underneath it. `minHeight` is a desktop window constraint and would
        // force a scroll on a short phone, so it only applies there.
        height: isMobile ? "100dvh" : "100vh",
        minHeight: isMobile ? undefined : 620,
        background: "var(--surface-app)",
        color: "var(--text-primary)",
        fontFamily: "var(--font-body)",
        overflow: "hidden",
        position: "relative",
      }}
    >
      <BackdropPattern />

      {isMobile ? (
        <MobilePanes room={activeRoom} showInfo={showInfo} />
      ) : (
        <>
          <SpacesRail />
          <RoomList />

          {activeRoom ? <ChatPane room={activeRoom} /> : <EmptyPane />}

          {showInfo && activeRoom && <RoomInfo room={activeRoom} />}
        </>
      )}

      <SettingsView />
      <ProfileCard />
      <DropZone />
      <Lightbox />
      <CreateRoom />
      <IncomingCallModal />
      <VerificationModal />
      <SyncIndicator />
      <UpdateBanner />

      {banner && (
        <div
          style={{
            position: "fixed",
            bottom: 22,
            left: "50%",
            transform: "translateX(-50%)",
            zIndex: 200,
            display: "flex",
            alignItems: "center",
            gap: 12,
            padding: "12px 18px",
            borderRadius: 18,
            maxWidth: 520,
            background: "var(--surface-card-raised)",
            border: `1px solid ${
              banner.tone === "error" ? "rgba(255,84,112,.4)" : "var(--border-default)"
            }`,
            boxShadow: "var(--shadow-pop)",
          }}
        >
          <Icon
            name={banner.tone === "error" ? "warning-circle" : "info"}
            size={16}
            color={banner.tone === "error" ? "var(--status-danger)" : "var(--accent-quaternary)"}
          />
          <span style={{ fontSize: 13, color: "var(--text-secondary)", lineHeight: 1.45 }}>
            {banner.message}
          </span>
          <button onClick={dismissBanner} style={{ cursor: "pointer", display: "flex" }}>
            <Icon name="x" size={13} color="var(--text-tertiary)" />
          </button>
        </div>
      )}

      {/* Fills the space left of the traffic lights so the window is draggable.
          There is no title bar to drag on a phone. */}
      {activeRoomId === null && !isMobile && (
        <div
          {...dragRegion(true)}
          style={{ position: "absolute", top: 0, left: 76, right: 0, height: 28 }}
        />
      )}
    </div>
  );
}

/**
 * The phone layout: one pane at a time.
 *
 * The stack falls out of state that already exists — no room is the root, a
 * room is the chat, and a room plus `showInfo` is the details view — so
 * navigation stays a single source of truth shared with the desktop shell
 * rather than a parallel one that can drift out of step with it.
 *
 * The spaces rail is the exception. On desktop it is always visible, which on a
 * 390pt screen would cost a fifth of the width to something touched once a
 * session, so here it is a drawer over the list.
 */
function MobilePanes({ room, showInfo }: { room: RoomSummary | undefined; showInfo: boolean }) {
  const { selectRoom, closeInfo } = useStore(
    useShallow((s) => ({ selectRoom: s.selectRoom, closeInfo: s.closeInfo })),
  );
  const [spacesOpen, setSpacesOpen] = useState(false);
  const roomId = room?.id;
  const leaveRoom = useCallback(() => void selectRoom(null), [selectRoom]);

  // `showInfo` is a saved *desktop* preference, so a phone can arrive with it
  // already true and drop the user into the details view the instant they open
  // a room. Entering a room always starts at the messages.
  useEffect(() => {
    closeInfo();
  }, [roomId, closeInfo]);

  if (room && showInfo) {
    return (
      <SwipeBack onBack={closeInfo}>
        <RoomInfo room={room} onBack={closeInfo} />
      </SwipeBack>
    );
  }
  if (room) {
    return (
      <SwipeBack onBack={leaveRoom}>
        <ChatPane room={room} onBack={leaveRoom} />
      </SwipeBack>
    );
  }

  return (
    <>
      <RoomList onOpenSpaces={() => setSpacesOpen(true)} />
      {spacesOpen && <SpacesRail asDrawer onClose={() => setSpacesOpen(false)} />}
    </>
  );
}

/**
 * Wraps a pushed view so it can be swiped away from the left edge.
 *
 * A plain element rather than something the panes handle themselves: the swipe
 * belongs to the *stack*, not to a chat or a details view, and the two would
 * otherwise each grow their own copy of it.
 */
function SwipeBack({ onBack, children }: { onBack: () => void; children: ReactNode }) {
  const { ref, style } = useEdgeSwipeBack(onBack);

  return (
    <div
      ref={ref}
      style={{
        flex: 1,
        minWidth: 0,
        display: "flex",
        // The pane's own background travels with it, so the shell's backdrop
        // shows through the gap rather than a slice of white.
        background: "var(--surface-app)",
        ...style,
      }}
    >
      {children}
    </div>
  );
}

/** A quiet dot in the corner, loud only when something is actually wrong. */
function SyncIndicator() {
  const status = useStore((s) => s.syncStatus);
  const insecure = useStore((s) => s.session?.insecureStorage ?? false);

  const problem = status.state === "offline" || status.state === "error";
  if (!problem && !insecure) return null;

  return (
    <div
      style={{
        position: "fixed",
        top: 10,
        right: 14,
        zIndex: 150,
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "6px 12px",
        borderRadius: 999,
        background: "var(--surface-card-raised)",
        border: "1px solid rgba(255,194,77,.4)",
        fontFamily: "var(--font-mono)",
        fontSize: 10.5,
        color: "var(--status-warning)",
      }}
      title={
        insecure
          ? "no system keychain was available, so your session is stored in a file readable only by your user account"
          : (status.message ?? "")
      }
    >
      <Icon
        name={problem ? "wifi-slash" : "shield-warning"}
        size={12}
        color="var(--status-warning)"
      />
      {problem ? (status.message ?? status.state) : "session stored without a keychain"}
    </div>
  );
}

/**
 * Shortcuts that belong to the app rather than to any one view.
 *
 * `cmd+,` on macOS arrives as a *menu event* rather than a keypress: the menu
 * bar is offered command-key combinations first, and anything no menu item
 * claims is swallowed before the web content ever sees it. So the mac half of
 * this lives in `install_settings_menu_item` on the Rust side and comes back as
 * an event. The listener below is for Windows and Linux, where `ctrl+,` reaches
 * the page normally.
 *
 * Settings closes on Escape already, so this only has to open.
 */
function useAppShortcuts() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "," && (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey) {
        e.preventDefault();
        useStore.getState().openSettings();
      }
    };

    window.addEventListener("keydown", onKey);
    const menu = ipc.onOpenSettings(() => useStore.getState().openSettings());

    return () => {
      window.removeEventListener("keydown", onKey);
      void menu.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);
}

/**
 * Watch the room list for things worth a banner, for as long as we're signed
 * in. Mounted with the shell rather than the app so signing out tears the
 * watcher down with it.
 */
function useNotifications() {
  useEffect(() => startNotifications(), []);
  useEffect(() => startRinger(), []);
  // Same lifetime as the rest: nothing should be polling for releases behind a
  // login screen, and signing out stops the timer with everything else.
  useEffect(() => startUpdateChecks(), []);
}

/**
 * Publish our own presence: online while the app is being used, away once it
 * has been sat idle for a while.
 *
 * The homeserver can't tell idle from gone — it only sees requests — so the
 * "away" half has to come from the client noticing that nobody has touched the
 * window. The backend keeps "online" alive with its own heartbeat; this only
 * has to report the transitions.
 */
function usePresenceReporter() {
  useEffect(() => {
    /** Discord's threshold, near enough, and long enough not to flap. */
    const IDLE_AFTER_MS = 5 * 60 * 1000;

    let idle = false;
    let timer: number | undefined;

    // Repeated calls with an unchanged state are dropped in the backend, so
    // every mouse move doesn't become a request.
    const report = (state: "online" | "unavailable") => {
      void ipc.setOwnPresence(state).catch(() => {});
    };

    const arm = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        idle = true;
        report("unavailable");
      }, IDLE_AFTER_MS);
    };

    const onActivity = () => {
      if (idle) {
        idle = false;
        report("online");
      }
      arm();
    };

    // A hidden window is a minimised or fully covered one — away without
    // waiting out the timer.
    const onVisibility = () => {
      if (document.visibilityState === "hidden") {
        idle = true;
        window.clearTimeout(timer);
        report("unavailable");
      } else {
        onActivity();
      }
    };

    const events = ["mousemove", "mousedown", "keydown", "wheel", "touchstart"];
    for (const event of events) {
      window.addEventListener(event, onActivity, { passive: true });
    }
    document.addEventListener("visibilitychange", onVisibility);
    arm();

    return () => {
      window.clearTimeout(timer);
      for (const event of events) window.removeEventListener(event, onActivity);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);
}

/** Subscribe to everything Rust pushes, for as long as the shell is mounted. */
function useBackendEvents() {
  useEffect(() => {
    const store = useStore.getState();

    const subscriptions = [
      ipc.onRooms((diffs) => useStore.getState().applyRoomDiffs(diffs)),
      ipc.onTimeline((update) => useStore.getState().applyTimelineUpdate(update)),
      ipc.onTyping((update) => useStore.getState().applyTyping(update)),
      ipc.onPresence((update) => useStore.getState().applyPresence(update)),
      ipc.onSyncStatus((status) => useStore.getState().setSyncStatus(status)),
      ipc.onVerificationRequest((request) =>
        useStore.getState().setVerificationRequest(request),
      ),
      ipc.onVerificationUpdate((update) =>
        useStore.getState().applyVerificationUpdate(update),
      ),
    ];

    // Sync is already running by the time this mounts, so the backend's first
    // room-list push has come and gone. Take a snapshot to catch up — but only
    // once the listeners are actually attached.
    //
    // `listen` is asynchronous: it registers the handler over IPC and resolves
    // afterwards. Firing the snapshot alongside it leaves a window where a diff
    // is emitted and nobody is listening, and a *dropped* diff is worse than a
    // late one — every index in every diff after it refers to a list one entry
    // longer or shorter than ours, so rooms start landing in the wrong place.
    // (`applyDiff` clamps rather than corrupting, and a later `reset` puts it
    // right, but the drift is real until then.)
    void Promise.all(subscriptions)
      .then(() => ipc.getRooms())
      .then(store.setRooms)
      .catch((e) => store.showBanner("error", ipc.asUwuError(e).message));

    // Spaces don't stream — they change rarely enough that a fetch on mount and
    // a slow poll is plenty.
    const loadSpaces = () => {
      ipc
        .getSpaces()
        .then(store.setSpaces)
        .catch(() => {});
    };
    loadSpaces();
    const spacesTimer = window.setInterval(loadSpaces, 60_000);

    // Presence the backend already polled for before this mounted — same race
    // the room snapshot above solves, and the same fix.
    ipc
      .getPresence()
      .then(store.setPresenceSnapshot)
      .catch(() => {});

    return () => {
      window.clearInterval(spacesTimer);
      // Nothing is on screen to want presence any more; drop the watch set so
      // signing back in starts from what the new session actually draws.
      resetPresence();
      for (const subscription of subscriptions) {
        void subscription.then((unlisten) => unlisten()).catch(() => {});
      }
    };
  }, []);
}
