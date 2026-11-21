//! Which verification the dialog shows, and what it says when one stops:
//! the React store's `setVerificationRequest` / `applyVerificationUpdate`
//! and VerificationModal's `cancelBody` (src/store/index.ts,
//! src/components/VerificationModal.tsx), ported so both apps follow the
//! same rules. Pure, so they're tested here.

use horo_core::verification::{RequestState, SasStage, SasStateDto, VerificationRequestDto};

/// The verification on screen: the request, and its emoji half once the
/// other side picks SAS.
#[derive(Debug, Clone)]
pub struct Flow {
    pub request: VerificationRequestDto,
    pub sas: Option<SasStateDto>,
}

/// Still going: neither finished nor cancelled.
pub fn is_live(flow: &Option<Flow>) -> bool {
    flow.as_ref().is_some_and(|f| !matches!(f.request.state, RequestState::Done | RequestState::Cancelled))
}

/// A request we've just learned about. One arriving while another is still
/// running doesn't replace it: abandoned flows are common (a retry leaves
/// the first open; the other client announces a fresh one each time you
/// press verify), and when the last one won, the dialog's buttons acted on a
/// flow nobody was looking at.
pub fn offer(current: &mut Option<Flow>, request: VerificationRequestDto) {
    if is_live(current) {
        return;
    }
    *current = Some(Flow { request, sas: None });
}

/// A request or its SAS moved on. Updates for any other flow are dropped:
/// abandoned flows keep emitting, sometimes minutes later, and one of their
/// cancels used to overwrite a verification that was going fine.
///
/// A finished flow stays up so its outcome can be read; closing is the
/// user's.
pub fn update_request(current: &mut Option<Flow>, request: VerificationRequestDto) {
    if let Some(flow) = current
        && flow.request.flow_id == request.flow_id
    {
        flow.request = request;
    }
}

pub fn update_sas(current: &mut Option<Flow>, sas: SasStateDto) {
    if let Some(flow) = current
        && flow.request.flow_id == sas.flow_id
    {
        flow.sas = Some(sas);
    }
}

/// Where a flow stands, for the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Before the emoji: theirs to accept, or ours to accept.
    Waiting,
    /// The emoji are up for comparing.
    Compare,
    /// We confirmed; their turn.
    Confirmed,
    Done,
    Cancelled,
}

impl Flow {
    /// Either half can carry the outcome: the SAS finishes first, but a flow
    /// can also stop before SAS ever starts.
    pub fn stage(&self) -> Stage {
        let sas = self.sas.as_ref().map(|s| s.state);
        if sas == Some(SasStage::Done) || self.request.state == RequestState::Done {
            Stage::Done
        } else if sas == Some(SasStage::Cancelled) || self.request.state == RequestState::Cancelled {
            Stage::Cancelled
        } else if sas == Some(SasStage::Confirmed) {
            Stage::Confirmed
        } else if self.sas.as_ref().is_some_and(|s| s.emoji.is_some()) {
            Stage::Compare
        } else {
            Stage::Waiting
        }
    }

    /// "your other device", or the other person's localpart.
    pub fn who(&self) -> String {
        if self.request.is_self_verification {
            "your other device".into()
        } else {
            localpart(&self.request.other_user_id).into()
        }
    }

    /// Why it stopped, saying which side stopped it: the SDK's reason for
    /// `m.user` reads the same whichever side sent it, and people take "the
    /// user" to mean the other one.
    pub fn cancel_body(&self) -> String {
        let (code, by_us, reason) = match &self.sas {
            Some(sas) if sas.cancel_code.is_some() => (&sas.cancel_code, sas.cancelled_by_us, &sas.cancel_reason),
            _ => (&self.request.cancel_code, self.request.cancelled_by_us, &self.request.cancel_reason),
        };
        let retry = "nothing has changed — you can try again.";
        match (code.as_deref(), reason) {
            (Some("m.timeout"), _) => format!("nobody answered in time. {retry}"),
            (Some("m.user"), _) if by_us == Some(true) => format!("the verification was cancelled from this device. {retry}"),
            (Some("m.user"), _) => format!("{} cancelled the verification. {retry}", self.who()),
            (_, Some(reason)) => format!("{} — {retry}", reason.trim_end_matches('.').to_lowercase()),
            _ => format!("the verification was cancelled. {retry}"),
        }
    }
}

fn localpart(user_id: &str) -> &str {
    user_id.trim_start_matches('@').split(':').next().unwrap_or(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(flow_id: &str, state: RequestState) -> VerificationRequestDto {
        VerificationRequestDto {
            flow_id: flow_id.into(),
            other_user_id: "@bob:example.org".into(),
            other_device_id: None,
            is_self_verification: false,
            we_started: false,
            state,
            cancel_reason: None,
            cancelled_by_us: None,
            cancel_code: None,
        }
    }

    fn sas(flow_id: &str, state: SasStage) -> SasStateDto {
        SasStateDto {
            flow_id: flow_id.into(),
            other_user_id: "@bob:example.org".into(),
            state,
            emoji: None,
            decimals: None,
            cancel_reason: None,
            cancelled_by_us: None,
            cancel_code: None,
        }
    }

    #[test]
    fn a_second_request_does_not_displace_a_live_one() {
        let mut flow = None;
        offer(&mut flow, request("live", RequestState::Requested));
        offer(&mut flow, request("other", RequestState::Requested));
        assert_eq!(flow.unwrap().request.flow_id, "live");
    }

    #[test]
    fn a_finished_flow_gives_way_to_the_next() {
        let mut flow = None;
        offer(&mut flow, request("live", RequestState::Requested));
        update_request(&mut flow, request("live", RequestState::Done));
        offer(&mut flow, request("next", RequestState::Requested));
        assert_eq!(flow.unwrap().request.flow_id, "next");
    }

    #[test]
    fn updates_for_other_flows_are_ignored() {
        let mut flow = None;
        offer(&mut flow, request("live", RequestState::Requested));
        let mut cancelled = request("abandoned", RequestState::Cancelled);
        cancelled.cancel_code = Some("m.user".into());
        update_request(&mut flow, cancelled);
        update_sas(&mut flow, sas("abandoned", SasStage::Cancelled));
        let flow = flow.unwrap();
        assert_eq!(flow.request.state, RequestState::Requested);
        assert!(flow.sas.is_none());
        assert_eq!(flow.stage(), Stage::Waiting);
    }

    #[test]
    fn the_sas_carries_the_outcome_and_the_request_stays_up() {
        let mut flow = None;
        offer(&mut flow, request("live", RequestState::Ready));
        let mut keys = sas("live", SasStage::KeysExchanged);
        keys.emoji = Some(Vec::new());
        update_sas(&mut flow, keys);
        assert_eq!(flow.as_ref().unwrap().stage(), Stage::Compare);
        update_sas(&mut flow, sas("live", SasStage::Confirmed));
        assert_eq!(flow.as_ref().unwrap().stage(), Stage::Confirmed);
        update_sas(&mut flow, sas("live", SasStage::Done));
        assert_eq!(flow.as_ref().unwrap().stage(), Stage::Done);
        assert_eq!(flow.unwrap().request.state, RequestState::Ready);
    }

    #[test]
    fn a_cancel_says_who_cancelled() {
        let mut ours = request("f", RequestState::Cancelled);
        ours.cancel_code = Some("m.user".into());
        ours.cancelled_by_us = Some(true);
        let flow = Flow { request: ours.clone(), sas: None };
        assert!(flow.cancel_body().starts_with("the verification was cancelled from this device."));

        ours.cancelled_by_us = Some(false);
        let flow = Flow { request: ours, sas: None };
        assert!(flow.cancel_body().starts_with("bob cancelled the verification."));

        let mut timeout = sas("f", SasStage::Cancelled);
        timeout.cancel_code = Some("m.timeout".into());
        let flow = Flow { request: request("f", RequestState::Ready), sas: Some(timeout) };
        assert!(flow.cancel_body().starts_with("nobody answered in time."));

        let mut other = request("f", RequestState::Cancelled);
        other.cancel_code = Some("m.key_mismatch".into());
        other.cancel_reason = Some("The keys did not match.".into());
        let flow = Flow { request: other, sas: None };
        assert!(flow.cancel_body().starts_with("the keys did not match — "));
    }
}
