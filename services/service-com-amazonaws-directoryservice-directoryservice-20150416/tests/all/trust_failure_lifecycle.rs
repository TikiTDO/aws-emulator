// SPDX-License-Identifier: CC0-1.0

//! The WHOLE life of a trust relationship that did not establish: what a client polling
//! `DescribeTrusts` for a terminal failure is told, what it may then do with the
//! relationship, and what is left of it afterwards.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: the Amazon Web Services side of a trust is established only against
//! a peer the remote domain has, so a create naming a domain no peer answers for is
//! accepted and then settles, by the clock, on the modelled terminal `Failed` state carrying
//! the `TrustStateReason` that outcome explains itself with — while `VerifyTrust` needs a
//! relationship there is something to verify about, so a verification asked for from a state
//! the relationship did not establish in is refused `InvalidParameterException` naming that
//! state and leaves the stored relationship exactly as it was.
//!
//! The sibling suites assert the two halves of the outcome separately —
//! `tests/all/trust_outcomes.rs` over external domain names and
//! `tests/all/trust_peer_resolution.rs` over the directories this service holds. This one
//! asserts the chain a client actually drives across them: create, the bounded poll that
//! has to CONVERGE on the failure, the refused verification, the delete a failed
//! relationship still accepts, and the not-found that follows it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{PAST_ONE_SPAN, PAST_ONE_SPAN_ONLY};
use crate::harness::{call, ok, service, string, PAST_THE_TEARDOWN};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// A `TrustPassword` the modelled pattern `^(\p{LD}|\p{Punct}| )+$` admits, within its
/// `length 1..128`.
const TRUST_PASSWORD: &str = "Tru5t-P@ssword 1";

/// The `TrustState` values of the modelled enum this suite names
/// (`smithy_query(query="shape", shape="TrustState")`).
const TRUST_STATE_CREATING: &str = "Creating";
const TRUST_STATE_CREATED: &str = "Created";
const TRUST_STATE_FAILED: &str = "Failed";
const TRUST_STATE_VERIFYING: &str = "Verifying";
const TRUST_STATE_DELETING: &str = "Deleting";

/// The `TrustType` a request that names none gets, from `CreateTrustRequest$TrustType`'s own
/// documentation — "The trust relationship type. `Forest` is the default."
const TRUST_TYPE_DEFAULT: &str = "Forest";

/// The `SelectiveAuth` a request that asked for none gets: the modelled enum's not-enabled
/// value (`smithy_query(query="shape", shape="SelectiveAuth")`).
const SELECTIVE_AUTH_DEFAULT: &str = "Disabled";

/// The suffix `crate::state::refusal` appends to every message of this service, before the
/// per-run request id it correlates the call with.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// The domain of the directory that holds every trust below. An ordinary name, so the
/// outcome each row gets is decided by the domain the TRUST points at.
const HOLDER_DOMAIN: &str = "corp.sim2real.net";

/// One row of the table: a `CreateTrust` request, and whether a peer answers for the remote
/// domain it names — which is what decides the state the relationship settles on.
struct Row {
    case: &'static str,
    remote_domain_name: &'static str,
    /// One value of the modelled `TrustDirection` catalog
    /// (`smithy_query(query="shape", shape="TrustDirection")`): `One-Way: Incoming`,
    /// `One-Way: Outgoing`, `Two-Way`.
    trust_direction: &'static str,
    /// One value of the modelled `TrustType` catalog, or `None` for a request that names
    /// none and takes the documented default.
    trust_type: Option<&'static str>,
    /// One value of the modelled `SelectiveAuth` catalog, or `None` for a request that
    /// names none.
    selective_auth: Option<&'static str>,
    has_a_peer: bool,
}

impl Row {
    /// This row's `CreateTrust` body: the required members, plus the optional ones the row
    /// names and nothing else, so a defaulted member is one the service derived.
    fn create_body(&self, directory_id: &str) -> Value {
        let mut body = json!({
            "DirectoryId": directory_id,
            "RemoteDomainName": self.remote_domain_name,
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": self.trust_direction,
        });
        if let Some(trust_type) = self.trust_type {
            body["TrustType"] = json!(trust_type);
        }
        if let Some(selective_auth) = self.selective_auth {
            body["SelectiveAuth"] = json!(selective_auth);
        }
        body
    }

    /// The `TrustState` this row's relationship settles on once the transition its create
    /// owed has fallen due.
    fn settled_state(&self) -> &'static str {
        if self.has_a_peer {
            TRUST_STATE_CREATED
        } else {
            TRUST_STATE_FAILED
        }
    }
}

/// The table: the remote domains a create can name, spanning the top-level domains the IETF
/// reserves as names NEVER installed in the global DNS — RFC 6761's `invalid`, `test` and
/// `localhost`, and RFC 2606's `example` (the catalog
/// `state::NON_RESOLVABLE_TOP_LEVEL_DOMAINS` names) — against ordinary names the DNS can
/// hold, in the spellings one domain name has (one label under the top-level domain, several
/// labels, mixed case, the fully qualified form with its root dot), and with each row
/// carrying a different combination of the modelled `TrustDirection`, `TrustType` and
/// `SelectiveAuth` values, so the outcome is the remote domain's rather than the request's
/// other members'.
fn rows() -> [Row; 7] {
    [
        Row {
            case: "a reserved invalid domain",
            remote_domain_name: "unreachable.sim2real.invalid",
            trust_direction: "One-Way: Outgoing",
            trust_type: None,
            selective_auth: None,
            has_a_peer: false,
        },
        Row {
            case: "a reserved test domain, one label under the top-level domain",
            remote_domain_name: "sim2real.test",
            trust_direction: "One-Way: Incoming",
            trust_type: Some("External"),
            selective_auth: Some("Enabled"),
            has_a_peer: false,
        },
        Row {
            case: "a reserved localhost domain",
            remote_domain_name: "peer.deep.sim2real.localhost",
            trust_direction: "Two-Way",
            trust_type: Some("Forest"),
            selective_auth: Some("Disabled"),
            has_a_peer: false,
        },
        Row {
            case: "a reserved example top-level domain in mixed case",
            remote_domain_name: "Peer.SIM2REAL.Example",
            trust_direction: "Two-Way",
            trust_type: Some("External"),
            selective_auth: None,
            has_a_peer: false,
        },
        Row {
            case: "a reserved invalid domain fully qualified",
            remote_domain_name: "rooted.sim2real.invalid.",
            trust_direction: "One-Way: Outgoing",
            trust_type: None,
            selective_auth: Some("Enabled"),
            has_a_peer: false,
        },
        Row {
            case: "an ordinary domain",
            remote_domain_name: "remote.sim2real.net",
            trust_direction: "One-Way: Outgoing",
            trust_type: Some("External"),
            selective_auth: None,
            has_a_peer: true,
        },
        Row {
            case: "a delegated documentation domain in mixed case",
            remote_domain_name: "Remote.Example.COM",
            trust_direction: "Two-Way",
            trust_type: None,
            selective_auth: Some("Enabled"),
            has_a_peer: true,
        },
    ]
}

/// A Managed Microsoft AD directory in service, the only type a trust is supported for.
async fn holder(router: &MultiServiceRouter, case: &str) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(HOLDER_DOMAIN), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// The one `Trust` `DescribeTrusts` reports for `trust_id`, asked for the way a client
/// polling one relationship of one directory asks: both filters at once, which are a
/// conjunction.
async fn describe_scoped(
    router: &MultiServiceRouter,
    directory_id: &str,
    trust_id: &str,
    case: &str,
) -> Value {
    let answer = ok(
        router,
        "DescribeTrusts",
        json!({ "DirectoryId": directory_id, "TrustIds": [trust_id] }),
        case,
    )
    .await;
    let trusts = answer["Trusts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Trusts, got {answer}"));
    assert_eq!(
        trusts.len(),
        1,
        "case {case:?}: expected exactly one trust for {trust_id}, got {trusts:?}"
    );
    trusts[0].clone()
}

/// The message text of a refusal, with the per-run request id this service appends split
/// off: the fixed sentence is the requirement, the id it correlates the call with is not.
fn refusal_text(parsed: &Value, case: &str) -> String {
    let message = parsed["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {parsed}"));
    let (text, request_id) = message
        .split_once(REQUEST_ID_SEPARATOR)
        .unwrap_or_else(|| panic!("case {case:?}: expected the request-id suffix, got {message:?}"));
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected the refusal to carry a request id, got {message:?}"
    );
    text.to_string()
}

/// RULE: a create naming a remote domain no peer answers for settles on the terminal
/// `Failed` state with a non-empty `TrustStateReason` — reporting the remote domain the
/// request gave it — a verification from that state is refused `InvalidParameterException`
/// naming the state and changes nothing, and the failed relationship can still be deleted,
/// after which it is gone from this service.
#[tokio::test]
async fn a_trust_that_did_not_establish_reports_its_failure_refuses_a_verify_and_then_goes() {
    let router = service();
    let table = rows();
    let directory_id = holder(&router, "the holder").await;

    // Every create is answered while the relationship is still being established — the
    // outcome is the clock's — so the whole table starts before anything is waited out.
    let mut created: Vec<(&Row, String)> = Vec::new();
    for row in &table {
        let answer = ok(
            &router,
            "CreateTrust",
            row.create_body(&directory_id),
            row.case,
        )
        .await;
        let trust_id = string(&answer, "TrustId", row.case);
        let transient = describe_scoped(&router, &directory_id, &trust_id, row.case).await;
        assert_eq!(
            transient["TrustState"],
            json!(TRUST_STATE_CREATING),
            "case {case:?}: expected the transient state right after the create, got {transient}",
            case = row.case
        );
        created.push((row, trust_id));
    }

    // The transition each create owed has fallen due — by the clock, not by the read.
    tokio::time::sleep(PAST_ONE_SPAN_ONLY).await;

    for (row, trust_id) in &created {
        let case = row.case;
        let settled = describe_scoped(&router, &directory_id, trust_id, case).await;
        let expected = row.settled_state();
        assert_eq!(
            settled["TrustState"],
            json!(expected),
            "case {case:?}: expected the trust to settle on {expected}, got {settled}"
        );

        // What the relationship reports is what the request asked for: the remote domain in
        // the caller's own spelling, the directory the trust hangs off, the direction the
        // request required, and the type and selective-authentication values it named or
        // the documented defaults.
        for (member, value) in [
            ("RemoteDomainName", json!(row.remote_domain_name)),
            ("DirectoryId", json!(&directory_id)),
            ("TrustId", json!(trust_id)),
            ("TrustDirection", json!(row.trust_direction)),
            (
                "TrustType",
                json!(row.trust_type.unwrap_or(TRUST_TYPE_DEFAULT)),
            ),
            (
                "SelectiveAuth",
                json!(row.selective_auth.unwrap_or(SELECTIVE_AUTH_DEFAULT)),
            ),
        ] {
            assert_eq!(
                settled[member], value,
                "case {case:?}: expected {member} to be {value}, got {settled}"
            );
        }

        if row.has_a_peer {
            assert!(
                settled["TrustStateReason"].is_null(),
                "case {case:?}: expected no reason for a state the relationship reached as \
                 asked, got {settled}"
            );
            // The established relationship IS verifiable, which is what makes the refusals
            // below about the state the trust is in rather than about the operation.
            let answer = ok(
                &router,
                "VerifyTrust",
                json!({ "TrustId": trust_id }),
                case,
            )
            .await;
            assert_eq!(
                answer["TrustId"],
                json!(trust_id),
                "case {case:?}: expected the verification to name the trust it started on, \
                 got {answer}"
            );
            let verifying = describe_scoped(&router, &directory_id, trust_id, case).await;
            assert_eq!(
                verifying["TrustState"],
                json!(TRUST_STATE_VERIFYING),
                "case {case:?}: expected the verification to start, got {verifying}"
            );
            continue;
        }

        let reason = settled["TrustStateReason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: expected the failed trust to state why, got {settled}"
        );

        // The verification is refused by naming the state it refused from, and the whole
        // relationship — its reason and the instant its state was last updated included —
        // is the one that was there before.
        let (status, parsed) = call(&router, "VerifyTrust", json!({ "TrustId": trust_id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the verification to be refused, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {parsed}"
        );
        assert_eq!(
            refusal_text(&parsed, case),
            format!("Cannot verify trust in state: {expected}"),
            "case {case:?}: expected the refusal to name the state it refused from, got {parsed}"
        );
        let after = describe_scoped(&router, &directory_id, trust_id, case).await;
        assert_eq!(
            after, settled,
            "case {case:?}: expected the refused verification to leave the relationship \
             alone, got {after}"
        );
    }

    // A relationship that did not establish is still one the caller can take away: the
    // delete is accepted and answers while the teardown is under way.
    for (row, trust_id) in &created {
        let case = row.case;
        let answer = ok(&router, "DeleteTrust", json!({ "TrustId": trust_id }), case).await;
        assert_eq!(
            answer["TrustId"],
            json!(trust_id),
            "case {case:?}: expected the delete to name the trust it tore down, got {answer}"
        );
        let tearing_down = describe_scoped(&router, &directory_id, trust_id, case).await;
        assert_eq!(
            tearing_down["TrustState"],
            json!(TRUST_STATE_DELETING),
            "case {case:?}: expected the teardown to be under way, got {tearing_down}"
        );
    }

    // Once the teardown has run out the relationship is gone, so resolving it by id is
    // reported as the entity that does not exist rather than served as an empty page.
    tokio::time::sleep(PAST_THE_TEARDOWN).await;
    for (row, trust_id) in &created {
        let case = row.case;
        let (status, parsed) = call(
            &router,
            "DescribeTrusts",
            json!({ "DirectoryId": &directory_id, "TrustIds": [trust_id] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the trust to be gone, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("EntityDoesNotExistException"),
            "case {case:?}: expected EntityDoesNotExistException, got {parsed}"
        );
        assert_eq!(
            refusal_text(&parsed, case),
            format!("Trust: {trust_id} does not exist"),
            "case {case:?}: expected the not-found sentence for the id the request named, got \
             {parsed}"
        );
    }
}

/// RULE (the same one, read as the client reads it): `Failed` is TERMINAL, so a bounded poll
/// of `DescribeTrusts` converges on it — no number of reads, and no refused verification
/// between them, moves the relationship on or changes what it reports.
#[tokio::test]
async fn a_failed_trust_stays_failed_however_long_a_client_polls_it() {
    let router = service();
    let directory_id = holder(&router, "the poll").await;
    let case = "the poll";
    let remote_domain_name = "polled.sim2real.invalid";
    let answer = ok(
        &router,
        "CreateTrust",
        json!({
            "DirectoryId": &directory_id,
            "RemoteDomainName": remote_domain_name,
            "TrustPassword": TRUST_PASSWORD,
            "TrustDirection": "One-Way: Outgoing",
        }),
        case,
    )
    .await;
    let trust_id = string(&answer, "TrustId", case);

    // The poll a client runs: read, wait, read again — each round past one transition span,
    // so a state machine that still owed a move would have made it between two rounds.
    let mut settled: Option<Value> = None;
    for round in 0..4u32 {
        tokio::time::sleep(PAST_ONE_SPAN_ONLY).await;
        let round_case = format!("{case}, round {round}");
        let reported = describe_scoped(&router, &directory_id, &trust_id, &round_case).await;
        assert_eq!(
            reported["TrustState"],
            json!(TRUST_STATE_FAILED),
            "case {round_case:?}: expected the poll to converge on {TRUST_STATE_FAILED}, got \
             {reported}"
        );
        let reason = reported["TrustStateReason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {round_case:?}: expected the failed trust to state why, got {reported}"
        );
        assert_eq!(
            reported["RemoteDomainName"],
            json!(remote_domain_name),
            "case {round_case:?}: expected the remote domain the create was given, got \
             {reported}"
        );
        if let Some(first) = settled.as_ref() {
            assert_eq!(
                &reported, first,
                "case {round_case:?}: expected every later poll to report the same \
                 relationship, got {reported}"
            );
        } else {
            settled = Some(reported);
        }

        // A verification attempted between two polls is refused from the state the trust is
        // in, and that refusal is not a transition either.
        let (status, parsed) = call(&router, "VerifyTrust", json!({ "TrustId": &trust_id })).await;
        assert_eq!(
            status, 400,
            "case {round_case:?}: expected the verification to be refused, got {status} with \
             {parsed}"
        );
        assert_eq!(
            refusal_text(&parsed, &round_case),
            format!("Cannot verify trust in state: {TRUST_STATE_FAILED}"),
            "case {round_case:?}: expected the refusal to name the state it refused from, got \
             {parsed}"
        );
    }
}
