import { useState } from "react";

import * as ipc from "../lib/ipc";
import type { HomeserverInfo } from "../lib/types";
import { useIsMobile } from "../lib/viewport";
import { useStore } from "../store";
import { BackdropPattern, Button, dragRegion, Icon, Logo, RaveLabel, Spinner } from "./ui";
import { corner, space, textSize } from "../lib/scale";

type Stage = "server" | "credentials";

export function LoginScreen() {
  const setSession = useStore((s) => s.setSession);
  const isMobile = useIsMobile();

  const [stage, setStage] = useState<Stage>("server");
  const [server, setServer] = useState("matrix.org");
  const [info, setInfo] = useState<HomeserverInfo | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function probeServer() {
    setBusy(true);
    setError(null);
    try {
      const discovered = await ipc.discoverHomeserver(server);
      setInfo(discovered);
      setStage("credentials");
    } catch (e) {
      setError(ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function signIn() {
    setBusy(true);
    setError(null);
    try {
      setSession(await ipc.loginPassword(server, username, password));
    } catch (e) {
      setError(ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  async function signInWithSso(providerId?: string) {
    setBusy(true);
    setError(null);
    try {
      setSession(await ipc.loginSso(server, providerId));
    } catch (e) {
      setError(ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      style={{
        height: "100vh",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "var(--surface-app)",
        position: "relative",
        overflow: "hidden",
      }}
    >
      <BackdropPattern />
      {/* The drag strip keeps the frameless window movable on macOS. */}
      <div
        {...dragRegion(!isMobile)}
        style={{ position: "absolute", top: 0, left: 0, right: 0, height: 32 }}
      />

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void (stage === "server" ? probeServer() : signIn());
        }}
        style={{
          position: "relative",
          zIndex: 1,
          width: 396,
          padding: space(30),
          borderRadius: corner(28),
          background: "var(--surface-card)",
          border: "1px solid var(--border-subtle)",
          boxShadow: "var(--shadow-pop)",
        }}
      >
        <Logo height={40} />
        <div style={{ color: "var(--text-secondary)", fontSize: textSize(13.5), marginTop: 10 }}>
          a cute matrix client~
        </div>

        <div style={{ marginTop: 26 }}>
          <RaveLabel style={{ marginBottom: 8 }}>
            {stage === "server" ? "homeserver" : "sign in"}
          </RaveLabel>

          {stage === "server" ? (
            <>
              <Field
                icon="globe-hemisphere-west"
                value={server}
                onChange={setServer}
                placeholder="matrix.org"
                autoFocus
              />
              <div
                style={{
                  fontSize: textSize(12),
                  color: "var(--text-tertiary)",
                  marginTop: 8,
                  lineHeight: 1.5,
                }}
              >
                where your account lives. we'll ask it how to sign you in.
              </div>
            </>
          ) : (
            <>
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: space(8),
                  marginBottom: 12,
                  fontFamily: "var(--font-mono)",
                  fontSize: textSize(11),
                  color: "var(--text-tertiary)",
                }}
              >
                <Icon name="check-circle" size={13} color="var(--accent-primary)" />
                <span className="uwu-ellipsis">{info?.homeserverUrl}</span>
                <button
                  type="button"
                  onClick={() => {
                    setStage("server");
                    setError(null);
                  }}
                  style={{
                    marginLeft: "auto",
                    color: "var(--accent-quaternary)",
                    cursor: "pointer",
                    fontSize: textSize(11),
                    fontFamily: "var(--font-mono)",
                  }}
                >
                  change
                </button>
              </div>

              {info?.supportsPassword && (
                <>
                  <Field
                    icon="user"
                    value={username}
                    onChange={setUsername}
                    placeholder="username"
                    autoFocus
                  />
                  <div style={{ height: 10 }} />
                  <Field
                    icon="lock-key"
                    value={password}
                    onChange={setPassword}
                    placeholder="password"
                    type="password"
                  />
                </>
              )}

              {info?.supportsSso && (
                <div style={{ marginTop: info.supportsPassword ? 16 : 0 }}>
                  {info.supportsPassword && (
                    <div
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: space(10),
                        margin: "4px 0 14px",
                      }}
                    >
                      <span style={{ flex: 1, height: 1, background: "var(--border-subtle)" }} />
                      <RaveLabel>or</RaveLabel>
                      <span style={{ flex: 1, height: 1, background: "var(--border-subtle)" }} />
                    </div>
                  )}
                  <div style={{ display: "flex", flexDirection: "column", gap: space(8) }}>
                    {(info.ssoProviders.length
                      ? info.ssoProviders
                      : [{ id: "", name: "single sign-on", icon: null }]
                    ).map((provider) => (
                      <Button
                        key={provider.id || "sso"}
                        variant="ghost"
                        onClick={() => void signInWithSso(provider.id || undefined)}
                        disabled={busy}
                        style={{ width: "100%" }}
                      >
                        continue with {provider.name.toLowerCase()}
                      </Button>
                    ))}
                  </div>
                </div>
              )}

              {!info?.supportsPassword && !info?.supportsSso && (
                <div style={{ fontSize: textSize(13), color: "var(--status-warning)" }}>
                  this server doesn't offer a sign-in method horo supports yet.
                </div>
              )}
            </>
          )}
        </div>

        {error && (
          <div
            style={{
              marginTop: 16,
              padding: space("10px 14px"),
              borderRadius: corner(14),
              background: "color-mix(in srgb, var(--red-500) 12%, transparent)",
              border: "1px solid color-mix(in srgb, var(--red-500) 35%, transparent)",
              color: "var(--status-danger)",
              fontSize: textSize(12.5),
              lineHeight: 1.5,
            }}
          >
            {error}
          </div>
        )}

        {(stage === "server" || info?.supportsPassword) && (
          <div style={{ marginTop: 22, display: "flex", alignItems: "center", gap: space(12) }}>
            <Button
              type="submit"
              disabled={
                busy ||
                (stage === "server" ? !server.trim() : !username.trim() || !password)
              }
              style={{ flex: 1 }}
            >
              {stage === "server" ? "next~" : "sign in~"}
            </Button>
            {busy && <Spinner />}
          </div>
        )}

        <div
          style={{
            marginTop: 20,
            display: "flex",
            alignItems: "center",
            gap: space(8),
            fontFamily: "var(--font-mono)",
            fontSize: textSize(10.5),
            color: "var(--text-tertiary)",
          }}
        >
          <Icon name="shield-check" size={12} color="var(--accent-primary)" />
          keys stay on this device, in your system keychain
        </div>
      </form>
    </div>
  );
}

function Field({
  icon,
  value,
  onChange,
  placeholder,
  type = "text",
  autoFocus,
}: {
  icon: string;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  type?: string;
  autoFocus?: boolean;
}) {
  return (
    <label
      style={{
        display: "flex",
        alignItems: "center",
        gap: space(10),
        background: "var(--surface-inset)",
        border: "1px solid var(--border-subtle)",
        borderRadius: 999,
        padding: space("11px 16px"),
      }}
    >
      <Icon name={icon} size={14} color="var(--text-tertiary)" />
      <input
        className="selectable"
        type={type}
        value={value}
        autoFocus={autoFocus}
        spellCheck={false}
        autoCapitalize="none"
        autoCorrect="off"
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        style={{
          flex: 1,
          background: "transparent",
          border: "none",
          outline: "none",
          color: "var(--text-primary)",
          fontSize: textSize(14),
        }}
      />
    </label>
  );
}
