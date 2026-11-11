// SPDX-License-Identifier: CC0-1.0

//! How this service QUOTES a `@pattern` in the refusal of a value outside it, driven end to
//! end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router.
//!
//! The rule under test: a value a member's `@pattern` does not admit is refused HTTP 400
//! `ValidationException` with the boundary's envelope — the rejected value, the member as
//! the wire names it, and the requirement — and the requirement QUOTES the expression AS THE
//! MODEL DECLARES IT, including the `^`/`$` pair the model wraps a whole-value pattern in. It
//! is asserted over a TABLE of this service's `@pattern`-constrained members rather than the
//! one recorded call: ids of four different id spaces, a topic name, a tag resource id and a
//! domain name, which between them span a short character class, a class with an escaped
//! metacharacter, a bracketed single letter, a counted hex run and a grouped repetition.
//!
//! The CHECK is the same expression: every rejected value below CONTAINS a substring the
//! expression matches (a run of admitted characters), so a value is refused exactly when the
//! model's anchored expression refuses it. The last case is the positive control: a value the
//! expression admits whole is not refused this way at all and reaches the handler.

use serde_json::json;

use crate::harness::{call, service};

/// A `DirectoryId` that satisfies the modelled `^d-[0-9a-f]{10}$` and is created in no case
/// here, so a request whose other members are well formed reaches the handler.
const ABSENT_DIRECTORY_ID: &str = "d-0123456789";

/// One row: an operation, a body with exactly ONE member outside its `@pattern`, the member
/// as the wire names it, the value that was rejected, and the expression this service is
/// expected to quote.
struct PatternCase {
    case: &'static str,
    operation: &'static str,
    member: &'static str,
    value: &'static str,
    /// The expression as the refusal quotes it: the shape's `@pattern` itself, anchor pair
    /// included. Each row's modelled pattern is named in the comment beside it, from
    /// `smithy_query(query="shape",shape="<Shape>")`.
    stated: &'static str,
    body: fn(&str) -> serde_json::Value,
}

/// The `@pattern`-constrained members this rule is asserted over, one per expression form
/// this service declares.
fn pattern_rows() -> Vec<PatternCase> {
    vec![
        // `TopicName`: `^[a-zA-Z0-9_-]+$` — the recorded case, a plain character class.
        PatternCase {
            case: "a topic name carrying dots",
            operation: "RegisterEventTopic",
            member: "topicName",
            value: "sim2real.behav.topic",
            stated: r"^[a-zA-Z0-9_-]+$",
            body: |value| json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "TopicName": value }),
        },
        // `DirectoryId`: `^d-[0-9a-f]{10}$` — a literal prefix and a counted hex run.
        PatternCase {
            case: "a directory id of the wrong form",
            operation: "DescribeLDAPSSettings",
            member: "directoryId",
            value: "not-a-directory-id",
            stated: r"^d-[0-9a-f]{10}$",
            body: |value| json!({ "DirectoryId": value }),
        },
        // `SnapshotId`: `^s-[0-9a-f]{10}$` — another id space, so the clause is not one
        // member's.
        PatternCase {
            case: "a snapshot id of the wrong form",
            operation: "DeleteSnapshot",
            member: "snapshotId",
            value: "snap-0123456789",
            stated: r"^s-[0-9a-f]{10}$",
            body: |value| json!({ "SnapshotId": value }),
        },
        // `AssessmentId`: `^da-[0-9a-f]{18}$` — a wider run and a two-letter prefix.
        PatternCase {
            case: "an assessment id too short for its id space",
            operation: "DescribeADAssessment",
            member: "assessmentId",
            value: "da-0123456789",
            stated: r"^da-[0-9a-f]{18}$",
            body: |value| json!({ "AssessmentId": value }),
        },
        // `ResourceId`: `^[d]-[0-9a-f]{10}$` — a bracketed single letter, which the drop
        // must leave alone.
        PatternCase {
            case: "a tag resource id of the wrong form",
            operation: "AddTagsToResource",
            member: "resourceId",
            value: "r-0123456789",
            stated: r"^[d]-[0-9a-f]{10}$",
            body: |value| {
                json!({
                    "ResourceId": value,
                    "Tags": [{ "Key": "owner", "Value": "platform" }]
                })
            },
        },
        // `CertificateId`: `^c-[0-9a-f]{10}$`, refused beside a well-formed `DirectoryId`,
        // so the clause names the member that broke its pattern and not the first one.
        PatternCase {
            case: "a certificate id of the wrong form",
            operation: "DescribeCertificate",
            member: "certificateId",
            value: "cert-0123456789",
            stated: r"^c-[0-9a-f]{10}$",
            body: |value| {
                json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "CertificateId": value })
            },
        },
        // `TrustId`: `^t-[0-9a-f]{10}$`.
        PatternCase {
            case: "a trust id of the wrong form",
            operation: "DeleteTrust",
            member: "trustId",
            value: "trust-0123456789",
            stated: r"^t-[0-9a-f]{10}$",
            body: |value| json!({ "TrustId": value }),
        },
        // `RemoteDomainName`: `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$` — a grouped
        // repetition whose character class carries an escaped metacharacter, which the drop
        // must carry through untouched.
        PatternCase {
            case: "a remote domain name with a space in it",
            operation: "CreateConditionalForwarder",
            member: "remoteDomainName",
            value: "not a domain!",
            stated: r"^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$",
            body: |value| {
                json!({
                    "DirectoryId": ABSENT_DIRECTORY_ID,
                    "RemoteDomainName": value,
                    "DnsIpAddrs": ["10.0.0.10"]
                })
            },
        },
    ]
}

/// The whole clause the boundary states for one rejected value.
fn pattern_clause(value: &str, member: &str, stated: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {stated}"
    )
}

#[tokio::test]
async fn a_pattern_refusal_quotes_the_expression_the_model_declares() {
    let router = service();

    for row in pattern_rows() {
        let (status, parsed) = call(&router, row.operation, (row.body)(row.value)).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the value to be refused, got {status} with {parsed}",
            row.case
        );
        assert_eq!(
            parsed["__type"],
            json!("ValidationException"),
            "case {:?}: expected the boundary's validation envelope, got {parsed}",
            row.case
        );
        let stated = parsed["message"].as_str().unwrap_or_default();
        assert_eq!(
            stated,
            pattern_clause(row.value, row.member, row.stated),
            "case {:?}: expected the expression quoted as the model declares it, got \
             {stated:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_value_its_pattern_admits_is_not_refused_by_it() {
    let router = service();

    // The positive control for the whole table: a topic name the expression admits WHOLE
    // clears the constraint walk and reaches the handler, which resolves the directory the
    // request named and refuses THAT instead — so the clause quotes the expression a value
    // had to satisfy and the walk admits exactly the values it matches.
    for topic_name in ["sim2real-behav-topic", "a", "A_1-b", "0123456789"] {
        let case = format!("topic name {topic_name:?}");
        let (status, parsed) = call(
            &router,
            "RegisterEventTopic",
            json!({ "DirectoryId": ABSENT_DIRECTORY_ID, "TopicName": topic_name }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the absent directory to be refused, got {status} with \
             {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("EntityDoesNotExistException"),
            "case {case:?}: expected the resolution failure rather than a constraint refusal, \
             got {parsed}"
        );
    }
}
