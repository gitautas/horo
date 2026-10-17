import { describe, expect, it } from "vitest";

import { chooseFlow, nextStage } from "./registration";
import type { RegisterChallenge, RegisterStage } from "./types";

const dummy: RegisterStage = { kind: "dummy", authType: "m.login.dummy", fallbackUrl: null };
const terms: RegisterStage = { kind: "terms", authType: "m.login.terms", fallbackUrl: null };
const token: RegisterStage = {
  kind: "registrationToken",
  authType: "m.login.registration_token",
  fallbackUrl: null,
};
const captcha: RegisterStage = {
  kind: "fallback",
  authType: "m.login.recaptcha",
  fallbackUrl: "https://hs.example/_matrix/client/v3/auth/m.login.recaptcha/fallback/web?session=s",
};
const email: RegisterStage = {
  kind: "unsupported",
  authType: "m.login.email.identity",
  fallbackUrl: null,
};

function challenge(flows: RegisterStage[][], completed: string[] = []): RegisterChallenge {
  return { session: "s", flows, completed, policies: [], error: null };
}

describe("chooseFlow", () => {
  it("skips flows with a stage horo can't do", () => {
    expect(
      chooseFlow(
        challenge([
          [email, terms],
          [captcha, terms],
        ]),
      ),
    ).toEqual([captcha, terms]);
  });

  it("prefers the flow that asks the least", () => {
    expect(chooseFlow(challenge([[captcha, terms], [token, terms], [dummy]]))).toEqual([dummy]);
  });

  it("sticks to a flow consistent with what's already done", () => {
    // Token is done: the dummy-only flow is no longer the one in progress.
    const c = challenge([[dummy], [token, terms]], [token.authType]);
    expect(chooseFlow(c)).toEqual([token, terms]);
  });

  it("gives up when every flow needs an email", () => {
    expect(chooseFlow(challenge([[email], [email, terms]]))).toBeNull();
  });
});

describe("nextStage", () => {
  it("walks a flow stage by stage", () => {
    const flows = [[captcha, terms]];
    expect(nextStage(challenge(flows))).toEqual(captcha);
    expect(nextStage(challenge(flows, [captcha.authType]))).toEqual(terms);
  });

  it("has nothing to offer when no flow fits", () => {
    expect(nextStage(challenge([[email]]))).toBeNull();
  });
});
