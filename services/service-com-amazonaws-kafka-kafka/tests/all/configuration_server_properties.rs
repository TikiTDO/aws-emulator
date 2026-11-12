// SPDX-License-Identifier: CC0-1.0

//! Which `server.properties` bodies the configuration writers admit, driven end to
//! end over the same `MultiServiceRouter` the emulator serves from (the shared
//! `kafka_harness`).
//!
//! RULE under test: an MSK configuration IS a `server.properties` file, and MSK
//! configures a published set of broker properties, so a body is admitted exactly
//! when every property it declares is one of those — a declaration whose NAME is
//! outside the set (a name that is no broker property, or one MSK manages itself)
//! is refused HTTP 400 `BadRequestException` per offending declaration, naming it
//! and the value the body gave it, and nothing is stored. A property's VALUE is
//! never judged: it is stored as submitted.
//!
//! The rows differ along what that rule generalises over: which separator a line
//! uses (`=`, `:`, whitespace) and whether it carries one at all, how many
//! properties a body declares and which of them offends, whether the offending
//! name is unknown to Apache Kafka or a real property MSK reserves, what a value
//! looks like (nonsense, empty, bytes that are not UTF-8), and which writer
//! submitted the body — the create's first revision or the update's next one, both
//! of which store one.

use serde_json::Value;

use service_com_amazonaws_kafka_kafka::state::{
    catalogued_kafka_versions,
    server_properties::{Declaration, SERVER_PROPERTIES_MEMBER},
};

use super::kafka_harness::{reported_server_properties, Emulator, Reply};

/// One row of the table: a submitted body, and the declarations of it MSK does
/// not configure — empty for a body it admits.
struct Row {
    case: &'static str,
    body: Vec<u8>,
    offending: Vec<(&'static str, &'static str)>,
}

impl Row {
    /// A row the writers must admit.
    fn admitted(case: &'static str, body: &[u8]) -> Self {
        Self {
            case,
            body: body.to_vec(),
            offending: Vec::new(),
        }
    }

    /// A row the writers must refuse, naming `offending` in that order.
    fn refused(case: &'static str, body: Vec<u8>, offending: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            case,
            body,
            offending,
        }
    }

    /// What the refusal's message must be: the per-property envelope for an
    /// offending row, rendered through the service's own wording.
    fn expected_message(&self) -> String {
        let rejected: Vec<Declaration> = self
            .offending
            .iter()
            .map(|(name, value)| Declaration {
                name: (*name).to_string(),
                value: (*value).to_string(),
            })
            .collect();
        let versions: Vec<String> = catalogued_kafka_versions()
            .iter()
            .map(|version| (*version).to_string())
            .collect();
        service_com_amazonaws_kafka_kafka::state::server_properties::unsupported_properties_message(
            &rejected, &versions,
        )
    }
}

/// The table both writers walk. The property names it admits are Amazon MSK
/// configuration properties, and the ones it refuses are a name Apache Kafka does
/// not have, a real Apache Kafka property MSK manages itself, and the first token
/// of a line that declares no property with a separator at all.
fn rows() -> Vec<Row> {
    vec![
        Row::admitted("one-property", b"num.partitions=1\n"),
        Row::admitted("colon-separator", b"num.partitions:1\n"),
        Row::admitted("whitespace-separator", b"num.partitions 1\n"),
        Row::admitted("no-trailing-newline", b"auto.create.topics.enable=true"),
        Row::admitted("empty-body", b""),
        Row::admitted(
            "comments-and-blanks-only",
            b"# nothing is declared here\n\n! nor here\n",
        ),
        Row::admitted(
            "many-properties",
            b"auto.create.topics.enable=true\ndelete.topic.enable=false\nlog.retention.hours=168\n",
        ),
        Row::admitted(
            // A property the published set carries that this issue never cited,
            // with an override value of the form the broker takes.
            "an-uncited-published-property",
            b"max.connections.per.ip.overrides=10.0.0.1:100\n",
        ),
        Row::admitted("empty-value", b"compression.type=\n"),
        // The VALUE is never judged, so a value no broker would accept is stored.
        Row::admitted("nonsense-value", b"num.partitions=not-a-number\n"),
        Row::admitted("negative-value", b"num.partitions=-5\n"),
        Row::admitted("non-boolean-value", b"auto.create.topics.enable=maybe\n"),
        Row::refused(
            "unknown-name",
            b"totally.not.a.kafka.property=1\n".to_vec(),
            vec![("totally.not.a.kafka.property", "1")],
        ),
        Row::refused(
            // A real Apache Kafka property, which MSK sets itself.
            "reserved-name",
            b"broker.id=7\n".to_vec(),
            vec![("broker.id", "7")],
        ),
        Row::refused(
            "a-line-that-declares-no-property-with-a-separator",
            b"this line has no equals sign and is not valid java properties syntax".to_vec(),
            vec![(
                "this",
                "line has no equals sign and is not valid java properties syntax",
            )],
        ),
        Row::refused(
            "an-offending-name-after-admitted-ones",
            b"num.partitions=1\ncompression.type=gzip\nnot.a.property=x\n".to_vec(),
            vec![("not.a.property", "x")],
        ),
        Row::refused(
            "every-offending-name-in-order",
            b"first.not.a.property=1\nnum.partitions=2\nsecond.not.a.property=3\n".to_vec(),
            vec![("first.not.a.property", "1"), ("second.not.a.property", "3")],
        ),
    ]
}

/// A name no two rows share, so each row is created in isolation.
fn name_for(case: &str) -> String {
    format!("properties-{case}")
}

/// Assert `reply` is the refusal a body declaring unconfigurable properties
/// answers: HTTP 400 `BadRequestException`, the per-property envelope as the whole
/// message, and the submitted member named as the parameter at fault.
fn assert_refused(reply: &Reply, row: &Row, operation: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {} ({operation}): expected 400 BadRequestException, got {} {}",
        row.case,
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        row.expected_message(),
        "case {} ({operation}): expected the per-property refusal {:?}, got {:?}",
        row.case,
        row.expected_message(),
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(SERVER_PROPERTIES_MEMBER),
        "case {} ({operation}): expected the refusal to name {SERVER_PROPERTIES_MEMBER}, got {}",
        row.case,
        reply.body
    );
}

/// RULE: `CreateConfiguration` stores the submitted body as revision 1 exactly
/// when every property it declares is one MSK configures — and a refused create
/// registers NOTHING: the name stays absent from `ListConfigurations` and free for
/// a later create.
#[tokio::test]
async fn a_create_stores_a_body_of_configurable_properties_and_refuses_any_other() {
    let emulator = Emulator::new();

    for row in rows() {
        let name = name_for(row.case);
        let reply = emulator
            .create_configuration(&name, Some("submitted"), &row.body)
            .await;
        if row.offending.is_empty() {
            assert_eq!(
                reply.status, 200,
                "case {}: expected CreateConfiguration to succeed, got {} {}",
                row.case, reply.status, reply.body
            );
            assert_eq!(
                reply.at("/state").as_str(),
                Some("ACTIVE"),
                "case {}: expected the created configuration to be ACTIVE, got {}",
                row.case,
                reply.body
            );
            assert_eq!(
                reply.at("/latestRevision/revision").as_i64(),
                Some(1),
                "case {}: expected revision 1, got {}",
                row.case,
                reply.body
            );

            // Read back through the revision reader: the body stored is the body
            // submitted, byte for byte.
            let arn = reply
                .at("/arn")
                .as_str()
                .unwrap_or_else(|| panic!("case {}: the create reported no ARN", row.case))
                .to_string();
            let read = emulator.describe_configuration_revision(&arn, 1).await;
            assert_eq!(
                read.status, 200,
                "case {}: expected revision 1 to be reported, got {} {}",
                row.case, read.status, read.body
            );
            assert_eq!(
                reported_server_properties(&read, row.case),
                row.body,
                "case {}: expected the stored body {:?} back",
                row.case,
                String::from_utf8_lossy(&row.body)
            );
            continue;
        }

        assert_refused(&reply, &row, "CreateConfiguration");

        // Nothing was registered, so the name is absent from the listing and a
        // create with an admitted body under the SAME name succeeds.
        assert!(
            !listed_names(&emulator, row.case).await.contains(&name),
            "case {}: a refused create must register nothing, but {name:?} is listed",
            row.case
        );
        let again = emulator
            .create_configuration(&name, Some("submitted"), b"num.partitions=1\n")
            .await;
        assert_eq!(
            again.status, 200,
            "case {}: a refused create must leave the name free, got {} {}",
            row.case, again.status, again.body
        );
    }
}

/// RULE: `UpdateConfiguration` appends the submitted body as the next revision
/// under exactly the same admission — a declaration MSK does not configure is
/// refused with the same envelope, and the configuration keeps the revision it
/// already reported as its latest.
#[tokio::test]
async fn an_update_appends_a_body_of_configurable_properties_and_refuses_any_other() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_ok("properties-updated", Some("created"), b"num.partitions=1\n")
        .await;

    // Revision 1 is the create's; each admitted row appends the next one, and each
    // refused row appends nothing.
    let mut latest = 1i64;
    for row in rows() {
        let reply = emulator
            .update_configuration(&arn, Some("appended"), &row.body)
            .await;
        if row.offending.is_empty() {
            latest += 1;
            assert_eq!(
                reply.status, 200,
                "case {}: expected UpdateConfiguration to succeed, got {} {}",
                row.case, reply.status, reply.body
            );
            assert_eq!(
                reply.at("/latestRevision/revision").as_i64(),
                Some(latest),
                "case {}: expected revision {latest}, got {}",
                row.case,
                reply.body
            );
            let read = emulator.describe_configuration_revision(&arn, latest).await;
            assert_eq!(
                reported_server_properties(&read, row.case),
                row.body,
                "case {}: expected the appended body {:?} back",
                row.case,
                String::from_utf8_lossy(&row.body)
            );
        } else {
            assert_refused(&reply, &row, "UpdateConfiguration");
        }

        // Either way, the configuration's latest revision is the last one an
        // ADMITTED body appended.
        let described = emulator.describe_configuration(&arn).await;
        assert_eq!(
            described.at("/latestRevision/revision").as_i64(),
            Some(latest),
            "case {}: expected revision {latest} to be the latest, got {}",
            row.case,
            described.body
        );
    }
}

/// RULE: the admission is over the bytes submitted, so a value carrying bytes that
/// are no UTF-8 text at all is stored verbatim under a configurable name, while a
/// body whose DECLARATION is those bytes names no property MSK configures and is
/// refused.
#[tokio::test]
async fn bytes_that_are_not_text_are_judged_by_the_name_they_declare() {
    let emulator = Emulator::new();

    // A value of raw bytes under a published name: admitted, and reported back
    // byte for byte.
    let raw = [b"compression.type=".as_slice(), &[0x00, 0xff, 0xfe, 0x80]].concat();
    let arn = emulator
        .create_configuration_ok("properties-raw-value", None, &raw)
        .await;
    let read = emulator.describe_configuration_revision(&arn, 1).await;
    assert_eq!(
        reported_server_properties(&read, "raw-value"),
        raw,
        "case \"raw-value\": expected the stored bytes back, got {}",
        read.body
    );

    // The same bytes as the NAME: no property MSK configures, so the create is
    // refused and nothing is stored.
    let refused = emulator
        .create_configuration(
            "properties-raw-name",
            None,
            &[&[0x00, 0xff, 0xfe, 0x80][..], b"=1\n"].concat(),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "BadRequestException"),
        "case \"raw-name\": expected 400 BadRequestException, got {} {}",
        refused.status,
        refused.body
    );
    assert!(
        !listed_names(&emulator, "raw-name")
            .await
            .contains(&"properties-raw-name".to_string()),
        "case \"raw-name\": a refused create must register nothing"
    );
}

/// RULE: the admission is the body's alone, so it is answered whatever else the
/// request carries — and the refusals of the other members keep their own
/// wordings.
#[tokio::test]
async fn the_body_is_judged_independently_of_the_other_members() {
    let emulator = Emulator::new();

    // An unconfigurable body with an unsupported Kafka version: the request is
    // refused, and the version refusal is not the body's envelope.
    let reply = emulator
        .call(
            "POST",
            "/v1/configurations",
            Some(serde_json::json!({
                "name": "properties-and-version",
                "kafkaVersions": ["0.0.1-does-not-exist"],
                "serverProperties": ::aws_smithy_types::base64::encode(b"not.a.property=1\n"),
            })),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case \"body and version\": expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("kafkaVersions"),
        "case \"body and version\": expected the version refusal to come first, got {}",
        reply.body
    );

    // A body MSK configures, with a name outside the service's character set: the
    // name refusal, which carries the service's fixed parameter sentence.
    let reply = emulator
        .create_configuration("properties name!", None, b"num.partitions=1\n")
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case \"illegal name\": expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some("name"),
        "case \"illegal name\": expected the refusal to name the name member, got {}",
        reply.body
    );
}

/// Every configuration name `ListConfigurations` reports for this endpoint.
async fn listed_names(emulator: &Emulator, case: &str) -> Vec<String> {
    let reply = emulator.call("GET", "/v1/configurations", None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListConfigurations to succeed, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/configurations")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
