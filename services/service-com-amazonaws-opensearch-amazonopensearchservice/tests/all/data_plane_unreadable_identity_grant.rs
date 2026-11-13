// SPDX-License-Identifier: CC0-1.0

//! What the identity side of a data-plane authorization decision is read from: the policy
//! documents this emulator HOLDS for the calling principal, and whether that principal also
//! carries a grant whose document it cannot read.
//!
//! RULE: a signed request to the address a domain publishes is refused only on what the
//! policies this emulator can READ say — so a principal carrying a managed policy whose
//! document is AWS's own, and which the IAM emulator publishes no document for, is a
//! principal the identity side does not bound: the absence of an allow among its readable
//! documents is no evidence about it, and a domain access policy that merely says nothing
//! about the principal does not refuse it. What still refuses it is the rule that wins over
//! every allow whichever side states it — an explicit `Deny` in a document that CAN be read.
//! That is the posture this service's control-plane gate takes for the same grant
//! (`crate::state::authorization`), so the two planes of one service cannot answer
//! differently about one caller.
//!
//! The rows differ along what the rule generalises over: WHERE the principal's grant lives
//! and what can be shown about it — a document embedded in the role, an attachment naming one
//! of AWS's own managed policies (a grant AWS owns), that same attachment beside a readable
//! explicit `Deny`, an attachment naming a policy of the ACCOUNT that the account does not
//! hold (which grants nothing, because the account is the authority on its own policies), and
//! a role carrying nothing of this service at all. Each row is probed over both methods the
//! write actions are read from, so the action a refusal names is the request's and not a
//! fixed string, and the second test reads the index back as the principal the domain's own
//! policy allows, so every verdict is backed by what the cluster holds afterwards.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, sent to the address the control plane PUBLISHED for the domain.
//! The control-plane harness comes from [`super::domain_lifecycle`], the session and
//! published-address helpers from [`super::published_domain_address`], and the domain and the
//! principals from [`super::data_plane_identity_grant_evidence`].

use serde_json::json;

use super::data_plane_identity_grant_evidence::{
    clause_of, given_domain, given_principals, Row, CONTROL_KEY, INDEX, ROWS, SESSION,
};
use super::domain_lifecycle::Emulator;
use super::published_domain_address::{at_address, refusal_message, session_arn};

/// The methods a write action is read from, as `(method, the action it asks for)`: the
/// `ESHttp*` actions of the IAM Service Authorization Reference, one per HTTP method.
const METHODS: [(&str, &str); 2] = [("POST", "es:ESHttpPost"), ("PUT", "es:ESHttpPut")];

/// The path one row's probe addresses, which carries the row's own document id so no two
/// probes write the same document.
fn probe_path(row: &Row, suffix: &str) -> String {
    format!("/{INDEX}/_doc/{}-{suffix}", row.role)
}

/// RULE: the identity side of the decision is what this emulator can SHOW about the
/// principal — a grant whose document it cannot read is not evidence of no permission, and a
/// readable explicit `Deny` still refuses.
#[tokio::test]
async fn an_unreadable_grant_is_not_refused_but_a_readable_deny_still_refuses() {
    let emulator = Emulator::new();
    given_principals(&emulator);
    let address = given_domain(&emulator).await;

    for row in ROWS {
        for (method, action) in METHODS {
            let case = row.case;
            let answer = at_address(
                &emulator,
                &address,
                row.access_key_id,
                method,
                &probe_path(row, method),
                Some(json!({ "case": case, "method": method })),
            )
            .await;
            if row.admitted {
                assert!(
                    (200..=201).contains(&answer.status),
                    "case {case:?} over {method}: a write the identity side does not refuse \
                     must be admitted, got {} with {:?}",
                    answer.status,
                    answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 403,
                "case {case:?} over {method}: expected the IAM layer's refusal, got {} with \
                 {:?}",
                answer.status, answer.body
            );
            let message = refusal_message(&answer);
            assert_eq!(
                message,
                format!(
                    "User: {} is not authorized to perform: {action} {}",
                    session_arn(row.role, SESSION),
                    clause_of(row, action)
                ),
                "case {case:?} over {method}: expected IAM's own refusal sentence, got \
                 {message:?}"
            );
        }
    }
}

/// RULE (the same one, read from the index): the write an unreadable grant was not refused
/// really landed, and the one a readable deny refused left nothing behind — so a verdict is
/// backed by what the cluster holds and not by the status alone.
#[tokio::test]
async fn the_admitted_write_lands_and_the_refused_one_leaves_nothing() {
    let emulator = Emulator::new();
    given_principals(&emulator);
    let address = given_domain(&emulator).await;

    for row in ROWS {
        let case = row.case;
        let path = probe_path(row, "landing");
        let written = at_address(
            &emulator,
            &address,
            row.access_key_id,
            "PUT",
            &path,
            Some(json!({ "case": case })),
        )
        .await;
        // The document is read back by the CONTROL role, whom the domain's own access policy
        // allows, so what is observed is the index's content rather than the writer's own
        // verdict a second time.
        let read = at_address(&emulator, &address, CONTROL_KEY, "GET", &path, None).await;
        let found = read.body["found"].as_bool().unwrap_or(false);
        assert_eq!(
            found, row.admitted,
            "case {case:?}: expected the document to be in the index={}, got the write \
             answered {} with {:?} and the read {} with {:?}",
            row.admitted, written.status, written.body, read.status, read.body
        );
    }
}
