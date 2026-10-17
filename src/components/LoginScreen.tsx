import { useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import * as ipc from "../lib/ipc";
import { nextStage } from "../lib/registration";
import type { HomeserverInfo, RegisterAuth, RegisterChallenge } from "../lib/types";
import { useIsMobile } from "../lib/viewport";
import { useStore } from "../store";
import { BackdropPattern, Button, dragRegion, Icon, Logo, RaveLabel, Spinner, Toggle } from "./ui";
import { corner, space, textSize } from "../lib/scale";

type Stage = "server" | "credentials" | "register";

export function LoginScreen() {
  const setSession = useStore((s) => s.setSession);
  const isMobile = useIsMobile();

  const [stage, setStage] = useState<Stage>("server");
  const [server, setServer] = useState("matrix.org");
  const [info, setInfo] = useState<HomeserverInfo | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  // Sign-up in progress: what the server still wants, and the answers to it.
  const [challenge, setChallenge] = useState<RegisterChallenge | null>(null);
  const [token, setToken] = useState("");
  const [agreed, setAgreed] = useState(false);
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

  const next = challenge ? nextStage(challenge) : null;

  /** Switch screens, dropping any half-finished sign-up. */
  function goTo(target: Stage) {
    setStage(target);
    setChallenge(null);
    setToken("");
    setAgreed(false);
    setError(null);
  }

  async function register(auth?: RegisterAuth) {
    setBusy(true);
    setError(null);
    try {
      const attempt = (session: string | null, auth?: RegisterAuth) =>
        ipc.registerAccount({ server, username, password, session, auth });

      let step = await attempt(challenge?.session ?? null, auth);
      // A dummy stage asks nothing of anyone, so it's answered here rather than
      // shown. Bounded, in case a server keeps asking.
      for (
        let tries = 0;
        tries < 3 &&
        step.status === "needsAuth" &&
        !step.challenge.error &&
        nextStage(step.challenge)?.kind === "dummy";
        tries++
      ) {
        step = await attempt(step.challenge.session, { type: "dummy" });
      }

      if (step.status === "done") {
        setSession(step.session);
        return;
      }
      setChallenge(step.challenge);
      setError(step.challenge.error);
    } catch (e) {
      setError(ipc.asUwuError(e).message);
    } finally {
      setBusy(false);
    }
  }

  function submit() {
    if (stage === "server") return probeServer();
    if (stage === "credentials") return signIn();
    if (!challenge) {
      if (password !== confirm) {
        setError("those passwords don't match");
        return;
      }
      return register();
    }
    switch (next?.kind) {
      case "dummy":
        return register({ type: "dummy" });
      case "registrationToken":
        return register({ type: "registrationToken", token });
      case "terms":
        return register({ type: "terms" });
      case "fallback":
        return register({ type: "fallback" });
    }
  }

  function canSubmit() {
    if (stage === "server") return !!server.trim();
    if (stage === "credentials") return !!username.trim() && !!password;
    if (!challenge) return !!username.trim() && !!password && !!confirm;
    if (next?.kind === "registrationToken") return !!token.trim();
    if (next?.kind === "terms") return agreed;
    return true;
  }

  const submitLabel =
    stage === "server"
      ? "next~"
      : stage === "credentials"
        ? "sign in~"
        : !challenge
          ? "create account~"
          : next?.kind === "fallback"
            ? "i've done it~"
            : "continue~";

  const showSubmit =
    stage === "server" ||
    (stage === "credentials" && info?.supportsPassword) ||
    (stage === "register" && (!challenge || next));

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
          void submit();
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
            {stage === "server"
              ? "homeserver"
              : stage === "credentials"
                ? "sign in"
                : "new account"}
          </RaveLabel>

          {stage === "server" ? (
            <>
              <Field
                icon="globe-hemisphere-west"
                value={server}
                onChange={setServer}
                placeholder="matrix.org"
                inputMode="url"
                enterKeyHint="go"
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
                  onClick={() => goTo("server")}
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

              {stage === "register" ? (
                <RegisterBody
                  challenge={challenge}
                  next={next}
                  username={username}
                  setUsername={setUsername}
                  password={password}
                  setPassword={setPassword}
                  confirm={confirm}
                  setConfirm={setConfirm}
                  token={token}
                  setToken={setToken}
                  agreed={agreed}
                  setAgreed={setAgreed}
                  busy={busy}
                />
              ) : (
                <>
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
                        autoComplete="current-password"
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
                          <span
                            style={{ flex: 1, height: 1, background: "var(--border-subtle)" }}
                          />
                          <RaveLabel>or</RaveLabel>
                          <span
                            style={{ flex: 1, height: 1, background: "var(--border-subtle)" }}
                          />
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

        {showSubmit && (
          <div style={{ marginTop: 22, display: "flex", alignItems: "center", gap: space(12) }}>
            <Button type="submit" disabled={busy || !canSubmit()} style={{ flex: 1 }}>
              {submitLabel}
            </Button>
            {busy && <Spinner />}
          </div>
        )}

        {stage === "credentials" && info?.supportsPassword && (
          <SwitchLink
            prompt="new here?"
            action="create an account"
            onClick={() => goTo("register")}
          />
        )}
        {stage === "register" && (
          <SwitchLink
            prompt={challenge ? "wrong details?" : "already have one?"}
            action={challenge ? "start over" : "sign in"}
            onClick={() => goTo(challenge ? "register" : "credentials")}
          />
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

/** What the server still wants for a new account, one stage at a time. */
function RegisterBody({
  challenge,
  next,
  username,
  setUsername,
  password,
  setPassword,
  confirm,
  setConfirm,
  token,
  setToken,
  agreed,
  setAgreed,
  busy,
}: {
  challenge: RegisterChallenge | null;
  next: ReturnType<typeof nextStage>;
  username: string;
  setUsername: (v: string) => void;
  password: string;
  setPassword: (v: string) => void;
  confirm: string;
  setConfirm: (v: string) => void;
  token: string;
  setToken: (v: string) => void;
  agreed: boolean;
  setAgreed: (v: boolean) => void;
  busy: boolean;
}) {
  if (!challenge) {
    return (
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
          autoComplete="new-password"
        />
        <div style={{ height: 10 }} />
        <Field
          icon="lock-key"
          value={confirm}
          onChange={setConfirm}
          placeholder="password, again"
          type="password"
          autoComplete="new-password"
        />
      </>
    );
  }

  switch (next?.kind) {
    case "registrationToken":
      return (
        <>
          <Hint>this server is invite-only. paste the registration token you were given.</Hint>
          <Field
            icon="ticket"
            value={token}
            onChange={setToken}
            placeholder="registration token"
            autoFocus
          />
        </>
      );

    case "terms":
      return (
        <>
          <Hint>before it makes your account, the server asks you to accept:</Hint>
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: space(6),
              margin: "4px 0 14px",
            }}
          >
            {challenge.policies.map((policy) => (
              <button
                key={policy.url}
                type="button"
                onClick={() => void openUrl(policy.url).catch(() => {})}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: space(8),
                  color: "var(--accent-quaternary)",
                  cursor: "pointer",
                  fontSize: textSize(13),
                  textAlign: "left",
                }}
              >
                <Icon name="arrow-square-out" size={13} />
                {policy.name}
              </button>
            ))}
            {!challenge.policies.length && <Hint>the server's terms of service</Hint>}
          </div>
          <label
            style={{
              display: "flex",
              alignItems: "center",
              gap: space(10),
              fontSize: textSize(13),
              color: "var(--text-secondary)",
            }}
          >
            <Toggle on={agreed} onToggle={setAgreed} label="i agree" />i agree
          </label>
        </>
      );

    case "fallback":
      return (
        <>
          <Hint>
            the server wants a quick check that you're human. it opens in your browser — come back
            here once it says you're done.
          </Hint>
          <Button
            variant="ghost"
            disabled={busy || !next.fallbackUrl}
            onClick={() => next.fallbackUrl && void openUrl(next.fallbackUrl).catch(() => {})}
            style={{ width: "100%" }}
          >
            open in browser
          </Button>
        </>
      );

    case "dummy":
      // Normally answered automatically; only shown when the server rejected it.
      return <Hint>the server needs one more go — press continue.</Hint>;

    default:
      return (
        <div style={{ fontSize: textSize(13), color: "var(--status-warning)", lineHeight: 1.5 }}>
          this server needs an email address or phone number to sign up, which horo can't do yet.
          sign up on the server's website, then sign in here.
        </div>
      );
  }
}

function Hint({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        fontSize: textSize(12.5),
        color: "var(--text-secondary)",
        lineHeight: 1.5,
        marginBottom: 10,
      }}
    >
      {children}
    </div>
  );
}

function SwitchLink({
  prompt,
  action,
  onClick,
}: {
  prompt: string;
  action: string;
  onClick: () => void;
}) {
  return (
    <div style={{ marginTop: 14, fontSize: textSize(12.5), color: "var(--text-tertiary)" }}>
      {prompt}{" "}
      <button
        type="button"
        onClick={onClick}
        style={{ color: "var(--accent-quaternary)", cursor: "pointer", fontSize: "inherit" }}
      >
        {action}
      </button>
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
  autoComplete,
  inputMode,
  enterKeyHint,
}: {
  icon: string;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  type?: string;
  autoFocus?: boolean;
  autoComplete?: string;
  inputMode?: React.HTMLAttributes<HTMLInputElement>["inputMode"];
  enterKeyHint?: React.HTMLAttributes<HTMLInputElement>["enterKeyHint"];
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
        autoComplete={autoComplete}
        inputMode={inputMode}
        enterKeyHint={enterKeyHint}
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
