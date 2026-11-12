// SPDX-License-Identifier: CC0-1.0

//! An MSK configuration's revision history (`UpdateConfiguration`, read back
//! through `DescribeConfigurationRevision` and `DescribeConfiguration`), driven
//! end to end over the wire through the same `MultiServiceRouter` the emulator
//! serves from (the shared `kafka_harness`).
//!
//! Each suite asserts a RULE over a TABLE of inputs, not one recorded request:
//! the rows differ along whatever the rule generalises over — the size and byte
//! content of the `server.properties` body (ASCII, multi-byte UTF-8, bytes that
//! are not UTF-8 at all, empty, long), whether a description was given, how many
//! updates have been applied, and the signing region. Where the rule is an
//! invariant (an append-only history, a read leaving the store as it found it, a
//! revision number outside the stored range being refused) the input space is
//! walked exhaustively over the stored revisions, which is stronger than
//! sampling it.

use service_com_amazonaws_kafka_kafka::state::{
    MAX_REVISIONS_PER_CONFIGURATION, MAX_SERVER_PROPERTIES_BYTES,
};

use super::kafka_harness::{
    assert_configuration_arn_resolves_to_nothing, reported_server_properties, Emulator, Reply,
    REGION,
};

/// The description a create below stores with revision 1.
const CREATED_DESCRIPTION: &str = "created";

/// `CreateConfiguration`, asserted to succeed; the wire call itself lives on the
/// shared harness.
async fn create_ok(emulator: &Emulator, name: &str, body: &[u8]) -> String {
    emulator
        .create_configuration_ok(name, Some(CREATED_DESCRIPTION), body)
        .await
}

/// `UpdateConfiguration` with this row's description and body.
async fn update(emulator: &Emulator, arn: &str, row: &RevisionRow) -> Reply {
    emulator
        .update_configuration(arn, row.description, &row.server_properties)
        .await
}

/// The same update, as an endpoint for `region` serves it — only the region the
/// configuration lives in holds it.
async fn update_in(emulator: &Emulator, region: &str, arn: &str, row: &RevisionRow) -> Reply {
    emulator
        .update_configuration_in(region, arn, row.description, &row.server_properties)
        .await
}

/// `DescribeConfigurationRevision`.
async fn describe_revision(emulator: &Emulator, arn: &str, revision: i64) -> Reply {
    emulator.describe_configuration_revision(arn, revision).await
}

/// The same read, as an endpoint for `region` serves it.
async fn describe_revision_in(
    emulator: &Emulator,
    region: &str,
    arn: &str,
    revision: i64,
) -> Reply {
    emulator
        .describe_configuration_revision_in(region, arn, revision)
        .await
}

/// `DescribeConfiguration`.
async fn describe(emulator: &Emulator, arn: &str) -> Reply {
    emulator.describe_configuration(arn).await
}

/// One `UpdateConfiguration` request, as a row of a table.
#[derive(Debug, Clone)]
struct RevisionRow {
    case: &'static str,
    description: Option<&'static str>,
    /// The `server.properties` body as raw bytes: the member is a blob, so it
    /// travels base64-encoded and may hold bytes that are not UTF-8 at all.
    server_properties: Vec<u8>,
}

impl RevisionRow {
    /// What `ConfigurationRevision$Description` must report: the description as
    /// given, and NOTHING at all when the request named none or named an empty
    /// one — the member is optional and an empty description is no description,
    /// so it is left out.
    fn expected_description(&self) -> Option<&'static str> {
        self.description.filter(|description| !description.is_empty())
    }
}

/// The table every suite below walks, applied as successive updates to one
/// configuration. The rows differ along what the rules generalise over: the
/// body's size and byte content, and whether a description was given.
fn rows() -> Vec<RevisionRow> {
    vec![
        RevisionRow {
            case: "ascii-properties",
            description: Some("second revision"),
            server_properties: b"auto.create.topics.enable=true\ndelete.topic.enable=false\n"
                .to_vec(),
        },
        RevisionRow {
            case: "no-description",
            description: None,
            server_properties: b"num.partitions=4\n".to_vec(),
        },
        RevisionRow {
            case: "empty-description",
            description: Some(""),
            server_properties: b"log.retention.hours=168\n".to_vec(),
        },
        RevisionRow {
            case: "multi-byte-utf8",
            description: Some("révision — настройка 構成"),
            // Non-ASCII bytes inside the body itself, so a handler that round
            // trips it through a lossy string conversion is caught.
            server_properties: "log.message.format.version=2.8-ß\n# コメント — ✅\n"
                .as_bytes()
                .to_vec(),
        },
        RevisionRow {
            case: "not-utf8-at-all",
            description: Some("raw bytes"),
            // A property whose VALUE carries bytes that are not valid UTF-8
            // anywhere (0xFF/0xFE), a NUL included: the member is a blob, so
            // they must survive the round trip.
            server_properties: [
                b"compression.type=".as_slice(),
                &[0x00, 0xFF, 0xFE, 0x80, 0xC3, b'\n'],
            ]
            .concat(),
        },
        RevisionRow {
            case: "empty-body",
            description: Some("no overrides"),
            server_properties: Vec::new(),
        },
        RevisionRow {
            case: "long-body",
            description: Some("many properties"),
            server_properties: (0..512)
                .map(|index| format!("log.retention.ms={index}\n"))
                .collect::<String>()
                .into_bytes(),
        },
        RevisionRow {
            case: "numeric-precision",
            description: Some("large and fractional numbers"),
            server_properties:
                b"log.retention.bytes=9007199254740993\nreplica.lag.time.max.ms=30000.5\n".to_vec(),
        },
    ]
}

/// RULE: an update of a configuration appends a revision numbered one past its
/// current latest, storing the submitted `server.properties` body and the
/// description of that revision; a revision already stored is never rewritten,
/// so every earlier revision still reports the body it was stored with.
#[tokio::test]
async fn each_update_appends_the_next_revision_and_leaves_earlier_ones_untouched() {
    let emulator = Emulator::new();
    let created_body = b"auto.create.topics.enable=false\n".to_vec();
    let arn = create_ok(&emulator, "appended", &created_body).await;

    // Revision 1 is the body the create stored, with the description it gave.
    let mut stored: Vec<(String, i64, Option<&'static str>, Vec<u8>)> = vec![(
        CREATED_DESCRIPTION.to_string(),
        1,
        Some(CREATED_DESCRIPTION),
        created_body,
    )];

    for (index, row) in rows().iter().enumerate() {
        // One past the latest revision stored so far, whatever the request
        // carries.
        let expected_revision = index as i64 + 2;
        let reply = update(&emulator, &arn, row).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateConfiguration to succeed, got {} {}",
            row.case, reply.status, reply.body
        );
        assert_eq!(
            reply.at("/arn").as_str(),
            Some(arn.as_str()),
            "case {}: the update must report the configuration's own ARN, got {}",
            row.case,
            reply.body
        );
        assert_eq!(
            reply.at("/latestRevision/revision").as_i64(),
            Some(expected_revision),
            "case {}: expected revision {expected_revision}, got {}",
            row.case,
            reply.body
        );
        assert_eq!(
            reply.at("/latestRevision/description").as_str(),
            row.expected_description(),
            "case {}: expected description {:?}, got {}",
            row.case,
            row.expected_description(),
            reply.body
        );
        let revision_time = reply.at("/latestRevision/creationTime").clone();
        assert!(
            revision_time.as_f64().is_some_and(|epoch| epoch > 0.0)
                || revision_time.as_str().is_some_and(|text| text.contains('T')),
            "case {}: expected a creationTime timestamp, got {}",
            row.case,
            reply.body
        );

        stored.push((
            row.case.to_string(),
            expected_revision,
            row.expected_description(),
            row.server_properties.clone(),
        ));

        // The configuration itself now reports this revision as its latest, so
        // the writer and the configuration reader agree on one record.
        let described = describe(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {}: expected DescribeConfiguration to succeed, got {} {}",
            row.case, described.status, described.body
        );
        assert_eq!(
            described.at("/latestRevision/revision").as_i64(),
            Some(expected_revision),
            "case {}: DescribeConfiguration must report the appended revision as latest, got {}",
            row.case,
            described.body
        );
        assert_eq!(
            described.at("/latestRevision/description").as_str(),
            row.expected_description(),
            "case {}: DescribeConfiguration must report the latest revision's description, got {}",
            row.case,
            described.body
        );

        // EVERY revision stored so far, read back: an append-only history means
        // each one still carries its own body and description, byte for byte.
        for (case, revision, description, body) in &stored {
            let read = describe_revision(&emulator, &arn, *revision).await;
            assert_eq!(
                read.status, 200,
                "case {case} revision {revision}: expected the stored revision, got {} {}",
                read.status, read.body
            );
            assert_eq!(
                read.at("/revision").as_i64(),
                Some(*revision),
                "case {case}: expected revision {revision}, got {}",
                read.body
            );
            assert_eq!(
                read.at("/arn").as_str(),
                Some(arn.as_str()),
                "case {case} revision {revision}: expected the configuration ARN, got {}",
                read.body
            );
            assert_eq!(
                read.at("/description").as_str(),
                *description,
                "case {case} revision {revision}: expected description {description:?}, got {}",
                read.body
            );
            let reported = reported_server_properties(&read, case);
            assert_eq!(
                reported,
                *body,
                "case {case} revision {revision}: expected the stored body {:?}, got {:?}",
                String::from_utf8_lossy(body),
                String::from_utf8_lossy(&reported)
            );
        }
    }

    // One revision per call and no more: the history holds exactly the create's
    // revision plus one per update, so the first number past it resolves to
    // nothing and is refused as the family's unresolvable reference.
    let past_the_end = stored.len() as i64 + 1;
    let missing = describe_revision(&emulator, &arn, past_the_end).await;
    assert_configuration_arn_resolves_to_nothing(
        &missing,
        &format!("revision={past_the_end}"),
        &arn,
        REGION,
    );
}

/// RULE: reading a revision reports it without changing it, so repeated reads of
/// the same revision answer identically however many updates followed it.
#[tokio::test]
async fn reading_a_revision_leaves_the_history_as_it_found_it() {
    let emulator = Emulator::new();
    let arn = create_ok(&emulator, "stable-reads", b"num.partitions=1\n").await;
    for row in rows() {
        let reply = update(&emulator, &arn, &row).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateConfiguration to succeed, got {} {}",
            row.case, reply.status, reply.body
        );
    }
    let revisions = rows().len() as i64 + 1;

    for revision in 1..=revisions {
        let first = describe_revision(&emulator, &arn, revision).await;
        let again = describe_revision(&emulator, &arn, revision).await;
        assert_eq!(
            first.status, 200,
            "case revision={revision}: expected the stored revision, got {} {}",
            first.status, first.body
        );
        assert_eq!(
            first.body, again.body,
            "case revision={revision}: a read must not change what a later read sees: {} vs {}",
            first.body, again.body
        );
    }

    // The latest revision is still the last one appended, so no read advanced
    // the history.
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described.at("/latestRevision/revision").as_i64(),
        Some(revisions),
        "case reads-only: expected revision {revisions} to remain the latest, got {}",
        described.body
    );
}

/// RULE: an update names the configuration by ARN, so an `Arn` that resolves to
/// no configuration of this endpoint's region is refused HTTP 400
/// `BadRequestException` — whether it is not an MSK configuration ARN at all or
/// a well-formed one nothing holds — and neither appends anything.
#[tokio::test]
async fn an_unknown_or_malformed_arn_is_refused_by_update_and_by_the_revision_reader() {
    let emulator = Emulator::new();
    let arn = create_ok(&emulator, "present", b"num.partitions=2\n").await;
    let known_prefix = arn
        .rsplit_once('/')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_default();

    // Rows spanning the refusal classes: not an ARN at all, an ARN of another
    // service, an MSK ARN of another resource type, a configuration ARN missing
    // its id segment, and a well-formed configuration ARN nothing holds. (An
    // empty value is not one of them: it addresses no resource URI at all, so
    // the router never reaches this operation with it.)
    let candidates: Vec<(&str, u16, &str)> = vec![
        ("not-an-arn", 400, "BadRequestException"),
        (
            "arn:aws:s3:::configuration/present/abc-1",
            400,
            "BadRequestException",
        ),
        (
            "arn:aws:kafka:us-west-2:000000000000:cluster/present/abc-1",
            400,
            "BadRequestException",
        ),
        (
            "arn:aws:kafka:us-west-2:000000000000:configuration/present",
            400,
            "BadRequestException",
        ),
        (
            "arn:aws:kafka:us-west-2:000000000000:configuration/absent/0e7b1f9a-1",
            400,
            "BadRequestException",
        ),
    ];

    let row = RevisionRow {
        case: "refused",
        description: Some("must not be stored"),
        server_properties: b"num.partitions=9\n".to_vec(),
    };
    for (candidate, status, code) in &candidates {
        let reply = update(&emulator, candidate, &row).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case arn={candidate:?}: expected {status} {code} from UpdateConfiguration, got {} {}",
            reply.status,
            reply.body
        );
        let read = describe_revision(&emulator, candidate, 1).await;
        assert_eq!(
            (read.status, read.error_code()),
            (*status, *code),
            "case arn={candidate:?}: expected {status} {code} from \
             DescribeConfigurationRevision, got {} {}",
            read.status,
            read.body
        );
    }

    // Nothing above appended a revision to the configuration that does exist.
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described.at("/latestRevision/revision").as_i64(),
        Some(1),
        "case refused-updates: a refused update stores nothing, got {}",
        described.body
    );
    assert!(
        !known_prefix.is_empty(),
        "case refused-updates: expected a minted ARN of the modelled form, got {arn:?}"
    );

    // A configuration lives in one region, so another region's endpoint holds
    // neither the configuration nor its revisions.
    let elsewhere = update_in(&emulator, "eu-central-1", &arn, &row).await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (400, "BadRequestException"),
        "case region=eu-central-1: another region's endpoint must not update it, got {} {}",
        elsewhere.status,
        elsewhere.body
    );
    let read_elsewhere = describe_revision_in(&emulator, "eu-central-1", &arn, 1).await;
    assert_eq!(
        (read_elsewhere.status, read_elsewhere.error_code()),
        (400, "BadRequestException"),
        "case region=eu-central-1: another region's endpoint must not read its revisions, \
         got {} {}",
        read_elsewhere.status,
        read_elsewhere.body
    );
}

/// RULE: a revision number the configuration never issued resolves to no
/// revision, so a read of it is refused exactly as an `Arn` that resolves to no
/// configuration is — one 400 `BadRequestException` carrying the family's fixed
/// unresolved-reference sentence — whether the number is above the stored
/// history or below the 1 a create stores.
#[tokio::test]
async fn a_revision_number_outside_the_stored_history_is_refused() {
    let emulator = Emulator::new();
    let arn = create_ok(&emulator, "bounded-history", b"num.partitions=3\n").await;
    let updates = 2;
    for row in rows().iter().take(updates) {
        let reply = update(&emulator, &arn, row).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateConfiguration to succeed, got {} {}",
            row.case, reply.status, reply.body
        );
    }
    let latest = updates as i64 + 1;

    // Each bound of the stored range, and one past each of them: `true` is a
    // revision the history holds, `false` one it does not.
    let candidates: Vec<(i64, bool)> = vec![
        (i64::MIN, false),
        (-1, false),
        (0, false),
        (1, true),
        (latest, true),
        (latest + 1, false),
        (i64::MAX, false),
    ];
    for (revision, held) in &candidates {
        let reply = describe_revision(&emulator, &arn, *revision).await;
        if *held {
            assert_eq!(
                (reply.status, reply.at("/revision").as_i64()),
                (200, Some(*revision)),
                "case revision={revision}: expected the stored revision, got {} {}",
                reply.status,
                reply.body
            );
        } else {
            assert_configuration_arn_resolves_to_nothing(
                &reply,
                &format!("revision={revision}"),
                &arn,
                REGION,
            );
        }
    }
}

/// RULE: this emulator stores a bounded revision history and a bounded
/// `server.properties` body, so an append past either ceiling is refused with
/// the operation's modelled client-input error and changes nothing — no revision
/// is evicted and none is rewritten.
#[tokio::test]
async fn an_append_past_the_stored_ceilings_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let arn = create_ok(&emulator, "bounded", b"num.partitions=1\n").await;

    // A body one byte past the ceiling is refused; the ceiling itself is not.
    // Both bounds come from `MAX_SERVER_PROPERTIES_BYTES`, so the rows follow
    // the constant rather than a literal.
    let filler = b"# padding\n";
    for (case, length, status) in [
        ("at-the-ceiling", MAX_SERVER_PROPERTIES_BYTES, 200u16),
        ("past-the-ceiling", MAX_SERVER_PROPERTIES_BYTES + 1, 400),
    ] {
        let body: Vec<u8> = filler.iter().copied().cycle().take(length).collect();
        let row = RevisionRow {
            case,
            description: Some("sized body"),
            server_properties: body.clone(),
        };
        let reply = update(&emulator, &arn, &row).await;
        assert_eq!(
            reply.status, status,
            "case {case} ({length} bytes): expected {status}, got {} {}",
            reply.status, reply.body
        );
        if status == 400 {
            assert_eq!(
                reply.error_code(),
                "BadRequestException",
                "case {case}: expected the modelled BadRequestException, got {}",
                reply.body
            );
            // The same ceiling applies to the body a create stores, so the
            // bound is a property of a stored revision, not of one operation.
            let created = emulator
                .create_configuration("oversized-create", None, &body)
                .await;
            assert_eq!(
                (created.status, created.error_code()),
                (400, "BadRequestException"),
                "case {case}: a create of the same body must be refused too, got {} {}",
                created.status,
                created.body
            );
        }
    }

    // Revision 2 is the body at the ceiling, and the refused one did not
    // replace it.
    let described = describe(&emulator, &arn).await;
    assert_eq!(
        described.at("/latestRevision/revision").as_i64(),
        Some(2),
        "case sized-bodies: a refused update appends nothing, got {}",
        described.body
    );

    // Fill the history to its ceiling, then one past it.
    for revision in 3..=MAX_REVISIONS_PER_CONFIGURATION as i64 {
        let row = RevisionRow {
            case: "filling",
            description: None,
            server_properties: format!("num.partitions={revision}\n").into_bytes(),
        };
        let reply = update(&emulator, &arn, &row).await;
        assert_eq!(
            reply.at("/latestRevision/revision").as_i64(),
            Some(revision),
            "case filling revision {revision}: expected it to be appended, got {} {}",
            reply.status,
            reply.body
        );
    }
    let past = RevisionRow {
        case: "past-the-history-ceiling",
        description: Some("one too many"),
        server_properties: b"num.partitions=999\n".to_vec(),
    };
    let refused = update(&emulator, &arn, &past).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "BadRequestException"),
        "case past-the-history-ceiling: expected the modelled BadRequestException, got {} {}",
        refused.status,
        refused.body
    );

    // Nothing was evicted to make room, and the latest revision did not move:
    // revision 1 still reports the body the create stored.
    let first = describe_revision(&emulator, &arn, 1).await;
    assert_eq!(
        first.status, 200,
        "case past-the-history-ceiling: revision 1 must not be evicted, got {} {}",
        first.status, first.body
    );
    assert_eq!(
        reported_server_properties(&first, "past-the-history-ceiling"),
        b"num.partitions=1\n".to_vec(),
        "case past-the-history-ceiling: revision 1 must still hold the created body, got {}",
        first.body
    );
    let latest = describe(&emulator, &arn).await;
    assert_eq!(
        latest.at("/latestRevision/revision").as_i64(),
        Some(MAX_REVISIONS_PER_CONFIGURATION as i64),
        "case past-the-history-ceiling: a refused append must not move the latest revision, \
         got {}",
        latest.body
    );
}
