//! Choosing a path through a homeserver's sign-up flows (registration.ts).
//!
//! The server offers several acceptable sequences of stages and reports
//! which are done; the screen only needs the one stage to show next.

use horo_core::dto::{RegisterChallenge, RegisterStage, RegisterStageKind};

/// Rough cost to the person signing up: a flow of cheap stages wins.
fn effort(kind: RegisterStageKind) -> u32 {
    match kind {
        RegisterStageKind::Dummy => 0,
        RegisterStageKind::Terms => 1,
        RegisterStageKind::RegistrationToken => 2,
        RegisterStageKind::Fallback => 3,
        // Leaving the app for an inbox and back asks the most of anyone.
        RegisterStageKind::Email => 4,
        RegisterStageKind::Unsupported => u32::MAX,
    }
}

/// The flow to follow: one holding every stage already done and nothing
/// horo can't do, asking the least. `None` when every flow needs
/// something unsupported (a phone number, in practice).
pub fn choose_flow(challenge: &RegisterChallenge) -> Option<&[RegisterStage]> {
    let cost = |flow: &[RegisterStage]| -> u32 {
        flow.iter().filter(|s| !challenge.completed.contains(&s.auth_type)).map(|s| effort(s.kind)).sum()
    };
    challenge
        .flows
        .iter()
        .filter(|flow| {
            flow.iter().all(|s| s.kind != RegisterStageKind::Unsupported)
                && challenge.completed.iter().all(|done| flow.iter().any(|s| &s.auth_type == done))
        })
        .map(Vec::as_slice)
        // The first of equally cheap flows, as reduce with `<` keeps it.
        .fold(None, |best: Option<&[RegisterStage]>, flow| match best {
            Some(b) if cost(b) <= cost(flow) => Some(b),
            _ => Some(flow),
        })
}

/// The first stage of the chosen flow the server hasn't marked done.
pub fn next_stage(challenge: &RegisterChallenge) -> Option<&RegisterStage> {
    choose_flow(challenge)?.iter().find(|s| !challenge.completed.contains(&s.auth_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(kind: RegisterStageKind, auth_type: &str) -> RegisterStage {
        RegisterStage { kind, auth_type: auth_type.into(), fallback_url: None }
    }

    fn challenge(flows: Vec<Vec<RegisterStage>>, completed: &[&str]) -> RegisterChallenge {
        RegisterChallenge { session: None, flows, completed: completed.iter().map(|s| s.to_string()).collect(), policies: vec![], error: None }
    }

    #[test]
    fn the_cheapest_doable_flow_is_followed() {
        use RegisterStageKind::*;
        let c = challenge(
            vec![
                vec![stage(Email, "m.login.email.identity"), stage(Terms, "m.login.terms")],
                vec![stage(RegistrationToken, "m.login.registration_token"), stage(Terms, "m.login.terms")],
                vec![stage(Unsupported, "m.login.msisdn")],
            ],
            &[],
        );
        assert_eq!(next_stage(&c).map(|s| s.kind), Some(RegistrationToken));
        // A stage done in one flow keeps us on a flow that has it.
        let c = challenge(c.flows.clone(), &["m.login.email.identity"]);
        assert_eq!(next_stage(&c).map(|s| s.kind), Some(Terms));
    }

    #[test]
    fn a_phone_only_server_has_no_way_through() {
        let c = challenge(vec![vec![stage(RegisterStageKind::Unsupported, "m.login.msisdn")]], &[]);
        assert!(next_stage(&c).is_none());
    }
}
