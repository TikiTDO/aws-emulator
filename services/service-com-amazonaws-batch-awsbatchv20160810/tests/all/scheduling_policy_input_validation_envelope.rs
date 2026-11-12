// SPDX-License-Identifier: CC0-1.0

//! How the scheduling-policy read plane renders the refusals its INPUT VALIDATION raises,
//! end to end over the wire.
//!
//! The rule under test: a refusal this service decides from the values the REQUEST itself
//! carries — a pattern, a bound, a required member — is answered with the modelled
//! `ClientException` (HTTP 400) inside Batch's input-validation envelope,
//! `Error executing request, Exception : <detail>, RequestId: <this call's id>`, so a client
//! classifies every validator of one operation with ONE matcher. A refusal decided from
//! STATE instead (not found, already exists, still in use) keeps its bare sentence, which
//! the suites beside this one assert.
//!
//! Asserted over the SET of validators `DescribeSchedulingPolicies` applies, each over
//! several inputs: the entry-shape rule on `arns`, the documented maximum number of
//! entries, and the required identity `arns` carries. The envelope's correlation id is
//! asserted too: it is minted per call, distinct for every refusal, and it is the id the
//! response reports in the header AWS carries a request id in — never a second one.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, error_code, error_message, Batch, REGION};
use crate::refusal_contract::{assert_uuid_shaped, enveloped, ENVELOPE_REQUEST_ID};

/// The header AWS reports a response's own request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The rule a scheduling-policy identifier of the wrong shape broke, spelled out here
/// rather than read from the crate's constant, so a reworded refusal is a failing test.
const PATTERN_RULE: &str = "Scheduling policy arn should match a valid Scheduling policy arn pattern";

/// The detail the arity validator of this operation states: the RANGE `arns` admits, not the
/// number of entries the request carried. Spelled out here rather than read from the crate,
/// so a reworded refusal is a failing test. The bounds are the member's own — "a list of up
/// to 100 scheduling policy Amazon Resource Name (ARN) entries"
/// (`smithy_query(query="operation", operation="DescribeSchedulingPolicies")`), and at least
/// one, the member being required.
const ARITY_RULE: &str = "Number of scheduling policies should be between 1 and 100.";

/// One refusal a validator of this operation raises.
struct Refusal {
    /// What this row varies; named in every assertion.
    label: String,
    /// The `DescribeSchedulingPolicies` request that provokes it.
    body: Value,
    /// The detail the envelope must carry, where its wording is settled: the entry-shape
    /// class states the rule and then the entry the request submitted, and the arity class
    /// states the range the member admits. `None` for the row whose inner sentence is
    /// owned by its own open issue — the envelope is the rule here, and the detail is only
    /// required to be a non-empty sentence that is not itself wrapped a second time.
    detail: Option<String>,
}

/// Every refusal `DescribeSchedulingPolicies` raises from the request's own values.
///
/// The entry-shape rows vary the KIND of malformation (not an ARN at all, an ARN of another
/// Batch resource kind, an ARN of another region or partition, an ARN with no resource) and
/// the POSITION of the offending entry among valid ones; the bound rows vary how far past
/// the documented maximum the list reaches; the required row names nothing at all.
fn refusals(stored_arn: &str) -> Vec<Refusal> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    let shapes: Vec<(&str, String)> = vec![
        ("a bare policy name", "sp-envelope".to_string()),
        ("a name carrying a space", "sp envelope".to_string()),
        ("an empty entry", String::new()),
        ("a job-queue ARN", arn("job-queue/jq-envelope")),
        ("an ARN with nothing after the account", format!("arn:aws:batch:{REGION}:{account}")),
        (
            "an ARN of another partition",
            format!("arn:aws-cn:batch:{REGION}:{account}:scheduling-policy/sp-envelope"),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:batch:eu-west-3:{account}:scheduling-policy/sp-envelope"),
        ),
    ];

    let mut rows: Vec<Refusal> = Vec::new();
    for (label, entry) in shapes {
        rows.push(Refusal {
            label: format!("{label} alone"),
            body: json!({"arns": [&entry]}),
            detail: Some(format!("{PATTERN_RULE}: {entry}")),
        });
        // The same entry travelling behind one that resolves: the refusal is the entry's,
        // whatever its position.
        rows.push(Refusal {
            label: format!("{label} after a stored ARN"),
            body: json!({"arns": [stored_arn, &entry]}),
            detail: Some(format!("{PATTERN_RULE}: {entry}")),
        });
    }

    // "A list of up to 100 scheduling policy Amazon Resource Name (ARN) entries" (`arns`),
    // one past the maximum and well past it. The detail states the RANGE the member admits,
    // not the number of entries the request carried, so every row expects one sentence.
    for over in [1usize, 7, 51] {
        let count = state::MAX_DESCRIBE_FILTER_ENTRIES + over;
        let entries: Vec<String> = (0..count)
            .map(|index| arn(&format!("scheduling-policy/sp-envelope-{index}")))
            .collect();
        rows.push(Refusal {
            label: format!("{count} entries, {over} past the documented maximum"),
            body: json!({"arns": entries}),
            detail: Some(ARITY_RULE.to_string()),
        });
    }

    // `arns` is the REQUIRED identity of what to describe, so a list that names nothing
    // identifies nothing.
    rows.push(Refusal {
        label: "no entries at all".to_string(),
        body: json!({"arns": []}),
        detail: None,
    });
    rows
}

/// RULE: every refusal `DescribeSchedulingPolicies` decides from the request's own values is
/// answered 400 / `ClientException` inside Batch's input-validation envelope, naming the id
/// of the call it refused.
#[tokio::test]
async fn every_input_validation_refusal_of_the_reader_carries_the_envelope() {
    let batch = Batch::new();

    let created = batch
        .ok("/v1/createschedulingpolicy", json!({"name": "sp-envelope-stored"}))
        .await;
    let stored_arn = created["arn"]
        .as_str()
        .expect("the create answers the policy ARN")
        .to_string();

    let mut request_ids: Vec<String> = Vec::new();
    for row in refusals(&stored_arn) {
        let case = &row.label;
        let (status, headers, refused) = batch
            .send_reported("POST", "/v1/describeschedulingpolicies", Some(row.body))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled ClientException 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case:?}: expected the modelled ClientException, got {refused}"
        );
        let message = error_message(&refused);
        let (detail, request_id) = enveloped(case, &message);
        match row.detail {
            Some(expected) => assert_eq!(
                detail, expected,
                "case {case:?}: expected the validator's sentence inside the envelope, got \
                 {message:?}"
            ),
            None => {
                assert!(
                    !detail.is_empty(),
                    "case {case:?}: expected the envelope to carry a sentence, got {message:?}"
                );
                assert!(
                    !detail.contains(ENVELOPE_REQUEST_ID),
                    "case {case:?}: expected ONE envelope around the sentence, got {message:?}"
                );
            }
        }
        // The id the envelope states is the id the response reports: a client correlating on
        // the header and one reading the message must see one call, not two.
        let reported = headers
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported, request_id,
            "case {case:?}: expected {REQUEST_ID_HEADER} to report the id the message states \
             ({request_id:?}), got {reported:?}"
        );
        assert_uuid_shaped(case, &request_id);
        request_ids.push(request_id);

        // Every refusal is effect-free: the stored policy is still readable by its ARN.
        let body = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [&stored_arn]}),
            )
            .await;
        assert_eq!(
            body["schedulingPolicies"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "case {case:?}: a refused read must leave the stored policy describable, got {body}"
        );
    }

    let distinct: std::collections::BTreeSet<&String> = request_ids.iter().collect();
    assert_eq!(
        distinct.len(),
        request_ids.len(),
        "case every refusal of the reader: expected one request id per call, got {request_ids:?}"
    );
}

/// RULE: the envelope wraps a REFUSAL and nothing else — a request whose `arns` are all of
/// the right shape, at any size up to the documented maximum, is served.
#[tokio::test]
async fn a_well_formed_read_is_served_and_carries_no_envelope() {
    let batch = Batch::new();

    let stored: Vec<String> = {
        let mut arns = Vec::new();
        for index in 0..3 {
            let created = batch
                .ok(
                    "/v1/createschedulingpolicy",
                    json!({"name": format!("sp-envelope-ok-{index}")}),
                )
                .await;
            arns.push(created["arn"].as_str().unwrap_or_default().to_string());
        }
        arns
    };

    // The sizes the rule generalises over: one entry, every stored one, and a list filled
    // to the documented maximum with well-formed ARNs that name nothing.
    let rows: Vec<(String, Vec<String>, usize)> = vec![
        ("one stored ARN".to_string(), vec![stored[0].clone()], 1),
        ("every stored ARN".to_string(), stored.clone(), stored.len()),
        (
            format!("{} entries, the documented maximum", state::MAX_DESCRIBE_FILTER_ENTRIES),
            {
                let mut entries = stored.clone();
                while entries.len() < state::MAX_DESCRIBE_FILTER_ENTRIES {
                    entries.push(arn(&format!(
                        "scheduling-policy/sp-envelope-absent-{}",
                        entries.len()
                    )));
                }
                entries
            },
            stored.len(),
        ),
    ];

    for (case, arns, expected) in rows {
        let body = batch
            .ok("/v1/describeschedulingpolicies", json!({"arns": arns}))
            .await;
        let found = body["schedulingPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            found.len(),
            expected,
            "case {case:?}: expected {expected} policy/policies, got {found:?}"
        );
    }
}
