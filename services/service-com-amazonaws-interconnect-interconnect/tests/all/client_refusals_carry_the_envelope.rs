// SPDX-License-Identifier: CC0-1.0

//! ONE refusal form for every client fault this service words itself.
//!
//! This service states its own rejections through a single envelope
//! (`crate::state::REFUSAL_ENVELOPE`, "Unable to process request: "), so a client that
//! strips or matches that prefix sees one message shape from the whole service rather
//! than one shape per failure mode. The rule: every refusal of a CLIENT fault this
//! service words itself opens with that envelope, whichever operation answers it and
//! whichever member or resource is at fault — with the one exception the service
//! documents as such, the ARN-scope mismatch, whose sentence is worded the way an
//! ARN-scope mismatch is worded service-wide and carries neither the envelope nor the
//! ARN.
//!
//! The rule is asserted twice over, in both directions:
//!
//! * over the WIRE, as a table whose rows differ along what the rule generalises over:
//!   the operation (a create, an update, each of the three listings, a read), the kind
//!   of fault (an input value the resource it names does not offer, an idempotency
//!   token reused differently, a page cursor the operation never issued, an identifier
//!   that resolves to nothing), the modelled error shape it travels in, and the value
//!   each row sends;
//! * over every refusal sentence the service builds, including the two no request to
//!   the published catalog can reach — an Environment in the `unavailable` state, which
//!   the published catalog holds none of, and the account's connection quota, which
//!   `connection_lifecycle` drives end to end — so a sentence added beside them cannot
//!   quietly leave the envelope out either.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::{
    activation_key_encoding_violation, activation_key_not_base64_message,
    arn_resource_absent_message, arn_scope_violation, attach_point_not_valid_message,
    bandwidth_availability_violation, client_token_reused_message, connection_absent_message,
    connection_not_retrievable_message, connection_not_updatable_message,
    connection_quota_exceeded_message, cursor_not_issued_message, environment_in_region,
    environment_not_found_message, environment_unavailable_message, not_a_connection_arn_message,
    not_a_gateway_identifier_message, parse_connection_arn, proposal_already_accepted_message,
    proposal_not_acceptable_message, proposal_not_retrievable_message, reserved_tag_key_violation,
    tag_write_violation, REFUSAL_ENVELOPE, TAG_KEY_LENGTH, UNAVAILABLE_ENVIRONMENT_STATE,
};

use crate::emulator::{
    available_bandwidths, code, create_request, message, placements, text, Emulator,
    DEFAULT_SIGNING_REGION,
};

/// The error this service models for a fault in the request's own input (HTTP 400).
const VALIDATION_CODE: &str = "InterconnectValidationException";

/// The error this service models for its own client faults (HTTP 400).
const CLIENT_CODE: &str = "InterconnectClientException";

/// The error a handle that resolves to no resource is answered with (HTTP 400 here, as
/// `client_fault_status_and_validation_wording` pins).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// Bandwidths written to `ConnectionBandwidth`'s own `^\d+[MG]bps$` pattern and 1..8
/// length (`smithy_query(query="shape",shape="ConnectionBandwidth")`) that no
/// Environment of the catalog currently offers, so the availability rule — not the
/// boundary validator — is what refuses them. Both modelled units, either side of the
/// sizes the catalog offers.
const NOT_OFFERED_BANDWIDTHS: [&str; 3] = ["1Mbps", "40Gbps", "999Gbps"];

/// `environmentId`s inside `EnvironmentId`'s 1..64 length that no Region's catalog
/// holds, so the create resolves nothing for them.
const UNRESOLVABLE_ENVIRONMENTS: [&str; 2] = ["mce-aws-nobody-xxx", "env-does-not-exist"];

/// `ConnectionId`s inside the member's `^(mcc|lmcc)-[a-z0-9]{8}$` pattern that no create
/// ever allocates, so a read of one resolves to nothing.
const UNALLOCATED_IDS: [&str; 2] = ["mcc-00000000", "lmcc-zzzzzzzz"];

/// Page cursors each listing never issued, per operation: a token of no cursor shape at
/// all, and one carrying ANOTHER listing's tag — never the operation's own tag, which
/// would be a cursor it did issue. `ListEnvironments` also gets a well-formed cursor of
/// its own tag naming an Environment no Region's catalog holds, which is not a cursor it
/// handed out either.
const FOREIGN_CURSORS: [(&str, [&str; 3]); 3] = [
    (
        "ListConnections",
        [
            "not-a-token",
            "env:mce-aws-nobody-xxx",
            "ap:00000000-0000-0000-0000-000000000000",
        ],
    ),
    (
        "ListEnvironments",
        [
            "not-a-token",
            "conn:mcc-00000000",
            "env:mce-aws-nobody-xxx",
        ],
    ),
    (
        "ListAttachPoints",
        ["not-a-token", "conn:mcc-00000000", "env:mce-aws-nobody-xxx"],
    ),
];

/// One row of the wire table: the request to send, and the refusal it must be answered
/// with — the status, the modelled error code and the message in full.
struct Refusal {
    case: String,
    operation: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
    message: String,
}

/// A `CreateConnection` body on `placement` asking for `bandwidth` instead of the
/// placement's own, with the identifying information the Environment asks a create for.
async fn create_with_bandwidth(
    emulator: &Emulator,
    placement: &(String, String),
    bandwidth: &str,
    seed: usize,
) -> Value {
    let mut request = create_request(emulator, DEFAULT_SIGNING_REGION, placement, seed, None, None)
        .await
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    request.insert("bandwidth".to_string(), json!(bandwidth));
    Value::Object(request)
}

/// RULE: every client-fault refusal this service words itself opens with the service's
/// refusal envelope, whichever operation answers it and whatever the request got wrong —
/// driven over the wire, and compared against the full sentence as well as the prefix.
#[tokio::test]
async fn every_client_fault_refusal_opens_with_the_service_envelope() {
    let emulator = Emulator::new();
    let placements = placements(&emulator).await;
    let placement = placements[0].clone();
    let environment = emulator.environment(&placement.0).await;
    let offered = available_bandwidths(&environment);

    // A live connection, so the update rows have something to update. It is created in
    // the `requested` state, which is a state an update is permitted from, so what
    // refuses those rows is the bandwidth and nothing else.
    let created = emulator
        .create_connection(
            &placement.0,
            &placement.1,
            &emulator.attach_point().await,
            "envelope suite connection",
        )
        .await;
    let identifier = text(&created, "id");

    // A clientToken that has already created a connection, replayed with a different
    // description so it names a connection other than the one this call describes.
    let token = "envelope-suite-token";
    let first = create_request(
        &emulator,
        DEFAULT_SIGNING_REGION,
        &placement,
        1,
        None,
        Some(token),
    )
    .await;
    let replayed = create_request(
        &emulator,
        DEFAULT_SIGNING_REGION,
        &placement,
        2,
        None,
        Some(token),
    )
    .await;
    let token_connection = emulator.ic_ok("CreateConnection", first).await;
    let token_connection_id = text(&token_connection["connection"], "id");

    let mut rows: Vec<Refusal> = Vec::new();

    // A bandwidth the Environment does not currently offer, through BOTH writers of a
    // connection's bandwidth: the create that allocates the first one and the update
    // that allocates a new one. One row per unit and magnitude, plus the sharpest row
    // the rule has — a size the Environment merely `supported`s.
    let unavailable: Vec<String> = environment["bandwidths"]["supported"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|bandwidth| bandwidth.as_str().unwrap_or_default().to_string())
        .filter(|bandwidth| !offered.contains(bandwidth))
        .chain(
            NOT_OFFERED_BANDWIDTHS
                .iter()
                .map(|bandwidth| (*bandwidth).to_string()),
        )
        .collect();
    for (seed, bandwidth) in unavailable.iter().enumerate() {
        // The sentence the handlers build, from the Environment of the called Region's
        // own catalog — the same record the handler resolves — rather than from a copy
        // of its wording.
        let expected = bandwidth_availability_violation(
            environment_in_region(DEFAULT_SIGNING_REGION, &placement.0).as_ref(),
            &placement.0,
            bandwidth,
        )
        .expect("a bandwidth the Environment does not offer is refused");
        rows.push(Refusal {
            case: format!("CreateConnection at {bandwidth}"),
            operation: "CreateConnection",
            body: create_with_bandwidth(&emulator, &placement, bandwidth, 100 + seed).await,
            status: 400,
            code: VALIDATION_CODE,
            message: expected.clone(),
        });
        rows.push(Refusal {
            case: format!("UpdateConnection to {bandwidth}"),
            operation: "UpdateConnection",
            body: json!({ "identifier": identifier, "bandwidth": bandwidth }),
            status: 400,
            code: VALIDATION_CODE,
            message: expected,
        });
    }

    // An idempotency token reused with different parameters.
    rows.push(Refusal {
        case: "CreateConnection replaying a clientToken differently".to_string(),
        operation: "CreateConnection",
        body: replayed,
        status: 400,
        code: VALIDATION_CODE,
        message: client_token_reused_message(token, &token_connection_id),
    });

    // An `environmentId` that resolves to no Environment of the called Region.
    for environment_id in UNRESOLVABLE_ENVIRONMENTS {
        rows.push(Refusal {
            case: format!("CreateConnection on {environment_id}"),
            operation: "CreateConnection",
            body: json!({
                "environmentId": environment_id,
                "bandwidth": placement.1,
                "attachPoint": { "directConnectGateway": emulator.attach_point().await },
            }),
            status: 400,
            code: CLIENT_CODE,
            message: environment_not_found_message(environment_id),
        });
    }

    // A page cursor the listing never issued, for each of the three paginated
    // operations.
    for (operation, tokens) in FOREIGN_CURSORS {
        for token in tokens {
            let mut body = Map::new();
            body.insert("nextToken".to_string(), json!(token));
            if operation == "ListAttachPoints" {
                body.insert("environmentId".to_string(), json!(placement.0));
            }
            rows.push(Refusal {
                case: format!("{operation} with nextToken {token}"),
                operation,
                body: Value::Object(body),
                status: 400,
                code: VALIDATION_CODE,
                message: cursor_not_issued_message(operation),
            });
        }
    }

    // An identifier that resolves to no connection — a refusal that already carried the
    // envelope, so the table spans the whole class rather than only what was reworded.
    for unallocated in UNALLOCATED_IDS {
        rows.push(Refusal {
            case: format!("GetConnection of {unallocated}"),
            operation: "GetConnection",
            body: json!({ "identifier": unallocated }),
            status: 400,
            code: NOT_FOUND_CODE,
            message: connection_not_retrievable_message(unallocated),
        });
        rows.push(Refusal {
            case: format!("UpdateConnection of {unallocated}"),
            operation: "UpdateConnection",
            body: json!({ "identifier": unallocated, "description": "a new description" }),
            status: 400,
            code: NOT_FOUND_CODE,
            message: connection_absent_message(unallocated),
        });
    }

    // The table spans the whole class rather than one row: every operation above
    // contributes, so a row quietly dropping out of it fails here rather than passing
    // vacuously.
    assert!(
        rows.len() >= 20,
        "case \"the refusal table\": expected the rule to be driven over every refusal in it, got {} rows",
        rows.len()
    );

    for row in &rows {
        let case = &row.case;
        let (status, answered) = emulator.ic(row.operation, row.body.clone()).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {answered:?}",
            row.status
        );
        let reported_code = code(&answered);
        assert!(
            reported_code.contains(row.code),
            "case {case:?}: expected the modelled {}, got {reported_code:?}",
            row.code
        );
        let reported = message(&answered);
        assert!(
            reported.starts_with(REFUSAL_ENVELOPE),
            "case {case:?}: expected a refusal opening with {REFUSAL_ENVELOPE:?}, got {reported:?}"
        );
        assert_eq!(
            reported, row.message,
            "case {case:?}: expected {:?}, got {reported:?}",
            row.message
        );
    }

    // The sentences themselves, spelled out rather than taken from the helpers the
    // handlers word them with: the envelope, each refusal's own clause, and the values
    // the REQUEST and the store supplied interpolated into it.
    let replayed_again = create_request(
        &emulator,
        DEFAULT_SIGNING_REGION,
        &placement,
        3,
        None,
        Some(token),
    )
    .await;
    let verbatim: [(&str, &str, Value, String); 3] = [
        (
            "the clientToken sentence",
            "CreateConnection",
            replayed_again,
            format!(
                "Unable to process request: clientToken '{token}' was already used to create \
                 connection '{token_connection_id}' with different parameters"
            ),
        ),
        (
            "the bandwidth sentence",
            "CreateConnection",
            create_with_bandwidth(&emulator, &placement, NOT_OFFERED_BANDWIDTHS[1], 200).await,
            format!(
                "Unable to process request: bandwidth '{}' is not available on environment \
                 '{}'; available bandwidths are [{}]",
                NOT_OFFERED_BANDWIDTHS[1],
                placement.0,
                offered.join(", ")
            ),
        ),
        (
            "the page-cursor sentence",
            "ListConnections",
            json!({ "nextToken": "not-a-token" }),
            "Unable to process request: nextToken is not a pagination token issued by \
             ListConnections"
                .to_string(),
        ),
    ];
    for (case, operation, body, expected) in verbatim {
        let (status, answered) = emulator.ic(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered:?}"
        );
        let reported = message(&answered);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// RULE, from the other side: every refusal sentence this service builds for a client
/// fault opens with the envelope — including the two no request to the published catalog
/// can reach — and the one the service documents as an exception does not.
#[test]
fn every_refusal_sentence_the_service_builds_carries_the_envelope() {
    let environment_id = "mce-aws-partner-iad";
    let connection_id = "mcc-0a1b2c3d";
    let gateway = "00000000-0000-0000-0000-000000000000";

    let (key_min, key_max) = TAG_KEY_LENGTH;
    let over_long_key: String = "k".repeat(key_max + 1);
    let mut over_long_tags: HashMap<String, String> = HashMap::new();
    over_long_tags.insert(over_long_key, "value".to_string());

    let sentences: Vec<(&str, String)> = vec![
        (
            "an unresolvable environmentId",
            environment_not_found_message(environment_id),
        ),
        (
            "an Environment that takes no new connections",
            environment_unavailable_message(environment_id, UNAVAILABLE_ENVIRONMENT_STATE),
        ),
        (
            "a clientToken reused with different parameters",
            client_token_reused_message("a-token", connection_id),
        ),
        (
            "the account's connection quota",
            connection_quota_exceeded_message(),
        ),
        (
            "a bandwidth the Environment does not offer",
            bandwidth_availability_violation(None, environment_id, "40Gbps")
                .expect("an Environment offering nothing offers no bandwidth"),
        ),
        (
            "a page cursor the listing never issued",
            cursor_not_issued_message("ListConnections"),
        ),
        (
            "an identifier no read resolves",
            connection_not_retrievable_message(connection_id),
        ),
        (
            "an identifier no update resolves",
            connection_absent_message(connection_id),
        ),
        (
            "a connection no update is permitted from",
            connection_not_updatable_message(connection_id),
        ),
        ("an ARN resolving to nothing", arn_resource_absent_message()),
        (
            "an arn that is not this service's",
            not_a_connection_arn_message("arn:aws:iam::123456789012:user/someone"),
        ),
        (
            "an activationKey resolving to no proposal",
            proposal_not_retrievable_message(),
        ),
        (
            "an activationKey that is not base64",
            activation_key_not_base64_message(),
        ),
        (
            "an activationKey judged on its encoding",
            activation_key_encoding_violation("not base64 at all!")
                .expect("text outside the base64 alphabet is refused"),
        ),
        (
            "a proposal already accepted",
            proposal_already_accepted_message(connection_id),
        ),
        (
            "a proposal no longer acceptable",
            proposal_not_acceptable_message(connection_id, "deleted"),
        ),
        (
            "an attachPoint that is not a gateway identifier",
            not_a_gateway_identifier_message("not-a-gateway"),
        ),
        (
            "a gateway that is no attach point of the Environment",
            attach_point_not_valid_message(gateway, environment_id),
        ),
        (
            "a tag key in the namespace AWS reserves",
            reserved_tag_key_violation(["aws:internal"])
                .expect("a reserved key is refused"),
        ),
        (
            "a tag key past its modelled length",
            tag_write_violation(None, &over_long_tags)
                .expect("a key longer than the model allows is refused"),
        ),
    ];

    for (case, sentence) in &sentences {
        assert!(
            sentence.starts_with(REFUSAL_ENVELOPE),
            "case {case:?}: expected a refusal opening with {REFUSAL_ENVELOPE:?}, got {sentence:?}"
        );
        assert!(
            sentence.len() > REFUSAL_ENVELOPE.len(),
            "case {case:?}: expected the envelope to be followed by a reason, got {sentence:?}"
        );
    }
    assert!(
        key_min <= key_max,
        "case \"tag key length\": expected the modelled bounds to be ordered, got {key_min}..{key_max}"
    );

    // The ONE exception the service documents: an ARN addressing another scope is
    // refused in the wording an ARN-scope mismatch carries service-wide, which is
    // deliberately neither this service's envelope nor the ARN.
    let foreign = "arn:aws:interconnect:us-east-1:999999999999:connection/mcc-00000000";
    let parsed = parse_connection_arn(foreign).expect("a well-formed connection ARN");
    let mismatch = arn_scope_violation(&parsed, "TagResourceRequest")
        .expect("an ARN of another account does not address the caller's scope");
    assert!(
        !mismatch.starts_with(REFUSAL_ENVELOPE),
        "case \"an ARN of another scope\": expected the documented exception to carry no envelope, got {mismatch:?}"
    );
    assert!(
        !mismatch.contains(foreign),
        "case \"an ARN of another scope\": expected the refusal to reproduce nothing of the ARN, got {mismatch:?}"
    );
}
