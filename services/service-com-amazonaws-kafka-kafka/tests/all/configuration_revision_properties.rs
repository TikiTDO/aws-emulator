// SPDX-License-Identifier: CC0-1.0

//! What `DescribeConfigurationRevision` reports for a stored revision's
//! `server.properties` body, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! RULE under test: `ConfigurationRevision$ServerProperties` is modelled as a
//! blob (`smithy_query(query="operation",operation="DescribeConfigurationRevision")`
//! reports `ServerProperties (__blob)`), so a revision reader reports the bytes
//! that were stored verbatim — it never parses, normalises or re-renders what a
//! property's value looks like. A numeric property value therefore keeps its
//! exact textual form whatever its magnitude, precision or notation, including
//! values no IEEE-754 double can hold exactly.
//!
//! The table's rows differ along what that rule generalises over: the magnitude
//! of the value (inside a double's exact integer range, one past it, past i64,
//! past u64), its notation (plain, fractional, exponent, signed, zero-padded)
//! and whether the body carries one property or many. The suite in
//! `configuration_revisions.rs` walks the body's size and byte content; this one
//! walks the shape of the values inside it.

use super::kafka_harness::{reported_server_properties, Emulator};

/// One row of the table: a `server.properties` body built from a single
/// property, named by the value's form.
#[derive(Debug, Clone, Copy)]
struct PropertyRow {
    case: &'static str,
    key: &'static str,
    /// The value exactly as the body carries it — the text the reader must hand
    /// back unchanged.
    value: &'static str,
}

impl PropertyRow {
    /// The body this row submits: one `key=value` line, as a
    /// `server.properties` file carries it.
    fn body(&self) -> Vec<u8> {
        format!("{}={}\n", self.key, self.value).into_bytes()
    }
}

/// The property names below are Amazon MSK configuration properties, and each
/// row's value is chosen for its numeric form rather than for being a value the
/// broker would accept.
fn rows() -> Vec<PropertyRow> {
    vec![
        PropertyRow {
            case: "small-integer",
            key: "num.partitions",
            value: "4",
        },
        PropertyRow {
            case: "inside-double-exact-range",
            key: "log.retention.bytes",
            value: "9007199254740992",
        },
        PropertyRow {
            // 2^53 + 1: the first integer a double cannot represent, so a
            // handler that round trips the body through a JSON number loses it.
            case: "one-past-double-exact-range",
            key: "log.retention.bytes",
            value: "9007199254740993",
        },
        PropertyRow {
            case: "past-i64",
            key: "log.retention.bytes",
            value: "9223372036854775808",
        },
        PropertyRow {
            case: "past-u64",
            key: "log.retention.bytes",
            value: "184467440737095516160",
        },
        PropertyRow {
            case: "fractional",
            key: "replica.lag.time.max.ms",
            value: "30000.5",
        },
        PropertyRow {
            case: "long-fraction",
            key: "log.cleaner.min.cleanable.ratio",
            value: "0.50000000000000000001",
        },
        PropertyRow {
            case: "trailing-zeros-after-point",
            key: "log.cleaner.min.cleanable.ratio",
            value: "0.5000",
        },
        PropertyRow {
            case: "exponent-notation",
            key: "log.segment.bytes",
            value: "1.073741824E9",
        },
        PropertyRow {
            case: "leading-zeros",
            key: "num.io.threads",
            value: "0008",
        },
        PropertyRow {
            case: "signed-negative",
            key: "log.retention.ms",
            value: "-1",
        },
        PropertyRow {
            case: "signed-positive",
            key: "num.network.threads",
            value: "+5",
        },
        PropertyRow {
            case: "whitespace-padded",
            key: "default.replication.factor",
            value: "  3  ",
        },
    ]
}

/// `UpdateConfiguration`, asserted to succeed; hands back the revision number it
/// appended.
async fn update_ok(emulator: &Emulator, case: &str, arn: &str, body: &[u8]) -> i64 {
    let reply = emulator
        .update_configuration(arn, Some("updated"), body)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected UpdateConfiguration to succeed, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/latestRevision/revision")
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: the update reported no revision: {}", reply.body))
}

/// RULE: the reader reports a stored revision's body byte for byte, so each
/// property value comes back in exactly the textual form it was stored in —
/// whether it reached the store through the create (revision 1) or through an
/// update (a later revision).
#[tokio::test]
async fn a_revision_reports_every_numeric_property_value_in_its_stored_form() {
    let emulator = Emulator::new();

    for row in rows() {
        let body = row.body();

        // The value as the create stored it: revision 1 of its own
        // configuration, so each row is read back in isolation.
        let arn = emulator
            .create_configuration_ok(&format!("created-{}", row.case), Some("created"), &body)
            .await;
        let created = emulator.describe_configuration_revision(&arn, 1).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected the created revision to be reported, got {} {}",
            row.case, created.status, created.body
        );
        let reported = reported_server_properties(&created, row.case);
        assert_eq!(
            reported,
            body,
            "case {}: expected the created body {:?}, got {:?}",
            row.case,
            String::from_utf8_lossy(&body),
            String::from_utf8_lossy(&reported)
        );

        // The same value as an update stored it, read back at the revision the
        // update appended: the reader is indifferent to which writer stored it.
        let revision = update_ok(&emulator, row.case, &arn, &body).await;
        let updated = emulator
            .describe_configuration_revision(&arn, revision)
            .await;
        assert_eq!(
            updated.status, 200,
            "case {} revision {revision}: expected the appended revision to be reported, \
             got {} {}",
            row.case, updated.status, updated.body
        );
        let reported = reported_server_properties(&updated, row.case);
        assert_eq!(
            reported,
            body,
            "case {} revision {revision}: expected the updated body {:?}, got {:?}",
            row.case,
            String::from_utf8_lossy(&body),
            String::from_utf8_lossy(&reported)
        );
        assert_eq!(
            updated.at("/revision").as_i64(),
            Some(revision),
            "case {}: expected revision {revision}, got {}",
            row.case,
            updated.body
        );
        assert_eq!(
            updated.at("/arn").as_str(),
            Some(arn.as_str()),
            "case {}: expected the configuration's own ARN, got {}",
            row.case,
            updated.body
        );
    }
}

/// RULE: one revision holds one whole body, so a body carrying many numeric
/// properties comes back with every line in its stored order and form — the
/// reader concatenates nothing and re-orders nothing.
#[tokio::test]
async fn a_revision_reports_a_whole_multi_property_body_in_order() {
    let emulator = Emulator::new();
    let table = rows();

    // Every row's property in one body, in the table's order.
    let body: Vec<u8> = table.iter().flat_map(|row| row.body()).collect();
    let arn = emulator
        .create_configuration_ok("many-properties", Some("created"), &body)
        .await;

    let read = emulator.describe_configuration_revision(&arn, 1).await;
    assert_eq!(
        read.status, 200,
        "case many-properties: expected the stored revision, got {} {}",
        read.status, read.body
    );
    let reported = reported_server_properties(&read, "many-properties");
    assert_eq!(
        reported,
        body,
        "case many-properties: expected the whole stored body {:?}, got {:?}",
        String::from_utf8_lossy(&body),
        String::from_utf8_lossy(&reported)
    );

    // And each individual value is present as its own line, so no row was
    // merged into its neighbour.
    let text = String::from_utf8(reported).expect("the body above is ASCII");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        table.len(),
        "case many-properties: expected {} lines, got {:?}",
        table.len(),
        lines
    );
    for (row, line) in table.iter().zip(lines) {
        assert_eq!(
            line,
            format!("{}={}", row.key, row.value),
            "case {}: expected the line stored for it, got {line:?}",
            row.case
        );
    }
}

/// RULE: the revision reader is served from stored state, so a request it can
/// answer carries the revision's own body and one it cannot carries the
/// operation's modelled error code — never the scaffold channel's
/// `NotImplemented`.
#[tokio::test]
async fn the_revision_reader_is_served_rather_than_refused_as_unimplemented() {
    let emulator = Emulator::new();
    let arn = emulator
        .create_configuration_ok("served", Some("created"), b"num.partitions=1\n")
        .await;

    // The revision the store holds, and one past it: each is answered by this
    // operation's own channel, so neither error code is the scaffold's.
    let candidates: Vec<(&str, i64, &str)> =
        vec![("stored", 1, ""), ("absent", 2, "BadRequestException")];
    for (case, revision, code) in candidates {
        let reply = emulator.describe_configuration_revision(&arn, revision).await;
        assert_eq!(
            reply.error_code(),
            code,
            "case {case}: expected the error code {code:?}, got {} {}",
            reply.status,
            reply.body
        );
    }
}
