/**
 * Choosing a path through a homeserver's sign-up flows.
 *
 * The server offers several acceptable sequences of stages and reports which
 * stages are already done; the form only ever needs to know the one stage to
 * show next. Kept apart from the screen so it can be tested without one.
 */

import type { RegisterChallenge, RegisterStage, RegisterStageKind } from "./types";

/** Rough cost to the person signing up: a flow of cheap stages wins. */
const EFFORT: Record<RegisterStageKind, number> = {
  dummy: 0,
  terms: 1,
  registrationToken: 2,
  fallback: 3,
  unsupported: Infinity,
};

/**
 * The flow to follow: one that contains every stage already completed and
 * nothing horo can't do, asking the least of the person. `null` when every
 * flow needs something unsupported — an email or phone number, in practice.
 */
export function chooseFlow(challenge: RegisterChallenge): RegisterStage[] | null {
  const candidates = challenge.flows.filter(
    (flow) =>
      flow.every((stage) => stage.kind !== "unsupported") &&
      challenge.completed.every((done) => flow.some((stage) => stage.authType === done)),
  );
  const cost = (flow: RegisterStage[]) =>
    flow
      .filter((stage) => !challenge.completed.includes(stage.authType))
      .reduce((sum, stage) => sum + EFFORT[stage.kind], 0);

  return candidates.reduce<RegisterStage[] | null>(
    (best, flow) => (best === null || cost(flow) < cost(best) ? flow : best),
    null,
  );
}

/** The first stage of the chosen flow the server hasn't marked complete. */
export function nextStage(challenge: RegisterChallenge): RegisterStage | null {
  return (
    chooseFlow(challenge)?.find((stage) => !challenge.completed.includes(stage.authType)) ?? null
  );
}
