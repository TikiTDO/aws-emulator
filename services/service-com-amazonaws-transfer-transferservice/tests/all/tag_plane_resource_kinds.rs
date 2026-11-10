// SPDX-License-Identifier: CC0-1.0

//! Which KIND of resource the tag plane reports as missing.
//!
//! RULE under test: `TagResource` and `UntagResource` address their target by ARN, so
//! the kind they report in a not-found refusal is the one the ARN names — the kind's own
//! `ResourceType`, the resource's own IDENTIFIER in `Resource` (not the whole ARN), and
//! the fixed `Message` that kind is reported with. Which is to say: the tag plane reports
//! a missing resource exactly as the operations of that kind report it, so a caller
//! tagging several kinds can tell them apart and anything keying on `Resource` as an
//! identifier gets one.
//!
//! The expectation is taken from the KIND's OWN reader rather than written here: each row
//! asks that kind's `Describe*` for the same resource and asserts the tag plane answered
//! the same three members, so neither answer can drift from the other.
//!
//! The rows span every resource kind this service's ARNs name — the account-level ones
//! and the ones a server owns, whose ARN carries the server before the child's
//! identifier — plus an ARN of another service, whose kind this service cannot name at
//! all.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{member_of, Answer, Emulator, ACCOUNT, REGION};

/// The three members `ResourceNotFoundException` models as required, as one answer
/// carries them.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    message: String,
    resource: String,
    resource_type: String,
}

impl Reported {
    fn of(answer: &Answer) -> Self {
        let member = |name: &str| {
            member_of(&answer.body, name)
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        Self {
            message: member("Message"),
            resource: member("Resource"),
            resource_type: member("ResourceType"),
        }
    }
}

/// Assert `answer` is this service's not-found refusal and hand back what it reported.
fn not_found(case: &str, op: &str, answer: &Answer) -> Reported {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected {op} to answer HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected {op} to answer ResourceNotFoundException, got {}",
        answer.body
    );
    let reported = Reported::of(answer);
    for (member, value) in [
        ("Message", &reported.message),
        ("Resource", &reported.resource),
        ("ResourceType", &reported.resource_type),
    ] {
        assert!(
            !value.is_empty(),
            "case {case:?}: {op} must answer {member}, which the model requires, got {}",
            answer.body
        );
    }
    reported
}

/// One row: the kind, an ARN of that kind nothing answers to, the identifier that ARN
/// addresses the resource by, and the request that kind's own `Describe*` resolves the
/// same resource with.
struct Row {
    kind: &'static str,
    arn: String,
    identifier: String,
    describe: &'static str,
    describe_request: Value,
}

/// RULE: the tag plane's not-found names the kind the ARN names, with that kind's own
/// `ResourceType`, the resource's identifier in `Resource`, and the same fixed `Message`
/// the kind's own reader answers.
#[tokio::test]
async fn a_tag_write_reports_the_resource_kind_its_arn_names() {
    let emulator = Emulator::new();
    // A server that EXISTS, so the rows for the resources a server owns are refused for
    // the child they name rather than for their parent.
    let server_id = emulator.create_server(json!({})).await;

    let transfer_arn = |resource: &str| format!("arn:aws:transfer:{REGION}:{ACCOUNT}:{resource}");

    // Identifiers of the modelled form that no create ever minted: `^s-([0-9a-f]{17})$`,
    // `^p-([0-9a-f]{17})$`, `^c-([0-9a-f]{17})$`, `^w-([a-z0-9]{17})$`,
    // `^a-([0-9a-f]{17})$`, `^cert-([0-9a-f]{17})$`, `^webapp-[0-9a-f]{17}$`,
    // `^hostkey-[0-9a-f]{17}$` and `UserName`'s `^[\w][\w@.-]{2,99}$`.
    let rows: Vec<Row> = vec![
        Row {
            kind: "server",
            arn: transfer_arn("server/s-00000000000000000"),
            identifier: "s-00000000000000000".to_string(),
            describe: "DescribeServer",
            describe_request: json!({"ServerId": "s-00000000000000000"}),
        },
        Row {
            kind: "user",
            arn: transfer_arn(&format!("user/{server_id}/absent-user")),
            identifier: "absent-user".to_string(),
            describe: "DescribeUser",
            describe_request: json!({"ServerId": &server_id, "UserName": "absent-user"}),
        },
        Row {
            kind: "host key",
            arn: transfer_arn(&format!("host-key/{server_id}/hostkey-00000000000000000")),
            identifier: "hostkey-00000000000000000".to_string(),
            describe: "DescribeHostKey",
            describe_request: json!({
                "ServerId": &server_id,
                "HostKeyId": "hostkey-00000000000000000",
            }),
        },
        Row {
            kind: "web app",
            arn: transfer_arn("webapp/webapp-00000000000000000"),
            identifier: "webapp-00000000000000000".to_string(),
            describe: "DescribeWebApp",
            describe_request: json!({"WebAppId": "webapp-00000000000000000"}),
        },
        Row {
            kind: "profile",
            arn: transfer_arn("profile/p-00000000000000000"),
            identifier: "p-00000000000000000".to_string(),
            describe: "DescribeProfile",
            describe_request: json!({"ProfileId": "p-00000000000000000"}),
        },
        Row {
            kind: "connector",
            arn: transfer_arn("connector/c-00000000000000000"),
            identifier: "c-00000000000000000".to_string(),
            describe: "DescribeConnector",
            describe_request: json!({"ConnectorId": "c-00000000000000000"}),
        },
        Row {
            kind: "workflow",
            arn: transfer_arn("workflow/w-00000000000000000"),
            identifier: "w-00000000000000000".to_string(),
            describe: "DescribeWorkflow",
            describe_request: json!({"WorkflowId": "w-00000000000000000"}),
        },
        Row {
            kind: "agreement",
            arn: transfer_arn(&format!("agreement/{server_id}/a-00000000000000000")),
            identifier: "a-00000000000000000".to_string(),
            describe: "DescribeAgreement",
            describe_request: json!({
                "ServerId": &server_id,
                "AgreementId": "a-00000000000000000",
            }),
        },
        Row {
            kind: "certificate",
            arn: transfer_arn("certificate/cert-00000000000000000"),
            identifier: "cert-00000000000000000".to_string(),
            describe: "DescribeCertificate",
            describe_request: json!({"CertificateId": "cert-00000000000000000"}),
        },
    ];

    let mut per_kind: Vec<(String, Reported)> = Vec::new();

    for row in &rows {
        // What the kind's OWN reader reports for the same resource: the expectation.
        let read = emulator
            .call(row.describe, row.describe_request.clone())
            .await;
        let expected = not_found(row.kind, row.describe, &read);
        assert_eq!(
            expected.resource, row.identifier,
            "case {:?}: {} must report the identifier it was asked for, got {:?}",
            row.kind, row.describe, expected.resource
        );

        for (op, request) in [
            (
                "TagResource",
                json!({"Arn": &row.arn, "Tags": [{"Key": "env", "Value": "prod"}]}),
            ),
            (
                "UntagResource",
                json!({"Arn": &row.arn, "TagKeys": ["env"]}),
            ),
        ] {
            let answer = emulator.call(op, request).await;
            let reported = not_found(row.kind, op, &answer);
            assert_eq!(
                reported, expected,
                "case {:?}: expected {op} to report the kind its ARN names exactly as {} \
                 reports it ({expected:?}), got {reported:?}",
                row.kind, row.describe
            );
            assert!(
                !reported.message.contains(&row.arn) && !reported.message.contains(&row.identifier),
                "case {:?}: expected {op}'s Message to carry neither the ARN nor the \
                 identifier, got {:?}",
                row.kind,
                reported.message
            );
        }
        per_kind.push((row.kind.to_string(), expected));
    }

    // The kinds are TOLD APART: no two of them report the same triple, which is what a
    // single generic answer for every ARN would do.
    for (index, (kind, reported)) in per_kind.iter().enumerate() {
        for (other_kind, other) in per_kind.iter().skip(index + 1) {
            assert_ne!(
                reported, other,
                "case {kind:?} beside {other_kind:?}: expected each kind to be reported as \
                 itself, both answered {reported:?}"
            );
        }
    }
}

/// RULE (the other half): an ARN whose KIND this service does not hold has no identifier
/// to decode, so the whole ARN is what the refusal reports.
///
/// The rows differ along why no kind can be read: another service's ARN, a Transfer ARN
/// naming a resource kind this service has no resource of, and a value whose resource
/// field names no kind at all.
#[tokio::test]
async fn a_tag_write_reports_the_whole_arn_when_it_names_no_kind_of_this_service() {
    let emulator = Emulator::new();

    let unknown_kinds: Vec<(&str, String)> = vec![
        (
            "another service's ARN",
            "arn:aws:s3:::tag-plane-bucket".to_string(),
        ),
        (
            "an IAM role ARN",
            format!("arn:aws:iam::{ACCOUNT}:role/transfer-s3"),
        ),
        (
            "a Transfer ARN of a kind this service holds no resource of",
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:instance/i-00000000000000000"),
        ),
        (
            "a Transfer ARN whose resource field names no kind",
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:/s-00000000000000000"),
        ),
    ];

    let mut generic: Vec<(String, Reported)> = Vec::new();
    for (case, arn) in &unknown_kinds {
        let answer = emulator
            .call(
                "TagResource",
                json!({"Arn": arn, "Tags": [{"Key": "env", "Value": "prod"}]}),
            )
            .await;
        let reported = not_found(case, "TagResource", &answer);
        assert_eq!(
            &reported.resource, arn,
            "case {case:?}: expected the whole ARN in Resource, got {:?}",
            reported.resource
        );
        assert!(
            !reported.message.contains(arn.as_str()),
            "case {case:?}: expected a Message carrying no ARN, got {:?}",
            reported.message
        );
        generic.push((case.to_string(), reported));
    }

    // One generic answer for every ARN this service can name no kind for.
    let (first_case, first) = &generic[0];
    for (case, reported) in &generic {
        assert_eq!(
            reported.message, first.message,
            "case {case:?}: expected the same generic Message as case {first_case:?} \
             ({:?}), got {:?}",
            first.message, reported.message
        );
        assert_eq!(
            reported.resource_type, first.resource_type,
            "case {case:?}: expected the same generic ResourceType as case {first_case:?} \
             ({:?}), got {:?}",
            first.resource_type, reported.resource_type
        );
    }
}
