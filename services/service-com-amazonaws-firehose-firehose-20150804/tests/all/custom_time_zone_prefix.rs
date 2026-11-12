// SPDX-License-Identifier: CC0-1.0

//! A destination's `CustomTimeZone` and the digits a delivery writes.
//!
//! The rule under test: every time field of an Amazon S3 delivery — the
//! `!{timestamp:<pattern>}` expansions of `Prefix` and `ErrorOutputPrefix`, the automatic
//! `YYYY/MM/DD/HH` segments a prefix carrying no expression takes ("The \"YYYY/MM/DD/HH\"
//! time format prefix is automatically used for delivered Amazon S3 files" — `Prefix`), and
//! the timestamp a delivered object's own name carries — is the delivery's instant as the
//! zone the destination's `CustomTimeZone` names reads it ("The time zone you prefer. UTC
//! is the default." — `ExtendedS3DestinationConfiguration$CustomTimeZone`), and the
//! documented default when the member is absent or empty.
//!
//! So the digits a key carries sit in a window the WALL CLOCK of the delivery decides: the
//! put's own instant, shifted by the zone's offset. Every case here asserts exactly that
//! window, which is what makes a zone-shifted key distinguishable from a UTC one without
//! any value taken from a recording.

use std::time::{Duration, Instant, SystemTime};

use chrono::{NaiveDateTime, Timelike};
use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// How long a delivery is waited for before a case fails: far longer than the emulator's
/// own delivery window, so a slow machine does not fail a case the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The record every case puts. Its bytes decide nothing here — only where the object
/// holding them lands.
const RECORD: &[u8] = b"{\"dept\":\"sales\"}";

/// The error output type of a record whose partition keys could not be determined, which
/// `!{firehose:error-output-type}` expands to.
const EXTRACTION_FAILED: &str = "metadata-extraction-failed";

/// One zone case: what `CustomTimeZone` carries, and the offset from UTC the zone keeps.
struct ZoneRow {
    case: &'static str,
    /// The member's value; `None` leaves the member out of the configuration altogether.
    configured: Option<&'static str>,
    /// The zone's offset from UTC in seconds, as the IANA time-zone database publishes it.
    offset: i64,
}

/// The zone cases.
///
/// Each name is an IANA zone of the class `CustomTimeZone` admits (`smithy_query
/// query="shape" shape="CustomTimeZone"`: `@length 0..50`, `@pattern ^$|[a-zA-Z/_]+$`, "The
/// time zone you prefer. UTC is the default."), and each names a zone that keeps ONE offset
/// all year — none of them observes daylight saving — so the offset beside it is that zone's
/// offset at every instant and no case depends on when it runs. The rows differ along what
/// the rule generalises over: whether the member is configured at all, the empty value its
/// own pattern admits, the default zone named outright, a whole-hour offset east of UTC, a
/// half-hour one, a name carrying the `_` the pattern admits, and an offset WEST of UTC.
fn zone_rows() -> Vec<ZoneRow> {
    vec![
        ZoneRow {
            case: "no CustomTimeZone at all, the documented default",
            configured: None,
            offset: 0,
        },
        ZoneRow {
            case: "the empty CustomTimeZone the member's own pattern admits",
            configured: Some(""),
            offset: 0,
        },
        ZoneRow {
            case: "the default zone named outright",
            configured: Some("UTC"),
            offset: 0,
        },
        ZoneRow {
            case: "a whole-hour zone east of UTC",
            configured: Some("Asia/Tokyo"),
            offset: 9 * 3_600,
        },
        ZoneRow {
            case: "a zone whose offset is not a whole hour",
            configured: Some("Asia/Kolkata"),
            offset: 5 * 3_600 + 1_800,
        },
        ZoneRow {
            case: "a zone whose name carries the underscore the pattern admits",
            configured: Some("Asia/Ho_Chi_Minh"),
            offset: 7 * 3_600,
        },
        ZoneRow {
            case: "a zone west of UTC",
            configured: Some("America/Phoenix"),
            offset: -7 * 3_600,
        },
    ]
}

/// How a destination states where its objects go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefixForm {
    /// A `Prefix` whose own `!{timestamp:...}` expressions carry the date, the hour and the
    /// minute.
    Expressions,
    /// A `Prefix` carrying no expression at all, which takes the automatic `YYYY/MM/DD/HH`
    /// segments.
    Automatic,
}

impl PrefixForm {
    /// The name this form's streams and prefixes are known by.
    fn name(self) -> &'static str {
        match self {
            Self::Expressions => "expressions",
            Self::Automatic => "automatic",
        }
    }

    /// The `Prefix` a destination of this form configures, under `leg`.
    fn prefix(self, leg: &str) -> String {
        match self {
            Self::Expressions => format!(
                "{leg}d=!{{timestamp:yyyy-MM-dd}}/h=!{{timestamp:HH}}/m=!{{timestamp:mm}}/"
            ),
            Self::Automatic => leg.to_string(),
        }
    }

    /// How precisely this form's key states the delivery's instant.
    fn resolution(self) -> Resolution {
        match self {
            Self::Expressions => Resolution::Minute,
            Self::Automatic => Resolution::Hour,
        }
    }

    /// The timestamp `key` carries right after `leg`.
    fn stamp(self, key: &str, leg: &str, case: &str) -> NaiveDateTime {
        match self {
            Self::Expressions => expression_stamp(key, leg, case),
            Self::Automatic => automatic_stamp(key, leg, case),
        }
    }
}

/// How precisely a time field states an instant, which is how far the window it must fall in
/// is truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Hour,
    Minute,
    Second,
}

impl Resolution {
    /// `at` with every field finer than this resolution dropped, as a key's digits drop it.
    fn truncate(self, at: NaiveDateTime) -> NaiveDateTime {
        let floored = match self {
            Self::Hour => at.with_minute(0).and_then(|at| at.with_second(0)),
            Self::Minute => at.with_second(0),
            Self::Second => Some(at),
        };
        floored.expect("zero is a valid minute and second")
    }
}

/// One stream under test: its destination, and what its delivered key must say.
struct Leg {
    case: String,
    stream: String,
    /// What every key of this stream starts with.
    leg: String,
    form: PrefixForm,
    /// The value `CustomTimeZone` was created with, which `DescribeDeliveryStream` echoes.
    configured: Option<&'static str>,
    offset: i64,
}

#[tokio::test]
async fn a_custom_time_zone_moves_every_time_field_a_delivered_key_carries() {
    let emulator = Emulator::new();
    let bucket = "custom-time-zone-bucket";
    emulator.create_bucket(bucket).await;

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per case.
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in zone_rows().into_iter().enumerate() {
        for form in [PrefixForm::Expressions, PrefixForm::Automatic] {
            let stream = format!("tz-{ordinal}-{}", form.name());
            let leg = format!("{stream}/");
            let mut config = json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": format!("arn:aws:s3:::{bucket}"),
                "Prefix": form.prefix(&leg),
                "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                "CompressionFormat": "UNCOMPRESSED",
            });
            if let Some(zone) = row.configured {
                config["CustomTimeZone"] = json!(zone);
            }
            emulator
                .firehose_ok(
                    "CreateDeliveryStream",
                    json!({
                        "DeliveryStreamName": stream,
                        "DeliveryStreamType": "DirectPut",
                        "ExtendedS3DestinationConfiguration": config,
                    }),
                )
                .await;
            legs.push(Leg {
                case: format!("{} with a {} prefix", row.case, form.name()),
                stream,
                leg,
                form,
                configured: row.configured,
                offset: row.offset,
            });
        }
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The control plane round-trips the member the data plane is about to act on.
    for leg in &legs {
        let description = describe(&emulator, &leg.stream).await;
        assert_eq!(
            description["DeliveryStreamStatus"].as_str(),
            Some("ACTIVE"),
            "case {:?}: the stream must be ACTIVE before a put, got {description:?}",
            leg.case
        );
        let echoed = description["Destinations"][0]["ExtendedS3DestinationDescription"]
            ["CustomTimeZone"]
            .as_str();
        assert_eq!(
            echoed, leg.configured,
            "case {:?}: DescribeDeliveryStream must echo the configured zone, got {echoed:?}",
            leg.case
        );
    }

    // The window every delivered key's digits must fall in opens with the puts.
    let put_at = SystemTime::now();
    for leg in &legs {
        let (status, answered) = emulator.put_record(&leg.stream, RECORD).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must be accepted, got {answered:?}",
            leg.case
        );
    }

    for leg in &legs {
        let key = first_delivered_key(&emulator, &leg.case, bucket, &leg.leg).await;
        // The delivery happened between the put and the moment its object was readable.
        let read_at = SystemTime::now();
        let stamp = leg.form.stamp(&key, &leg.leg, &leg.case);
        assert_in_zone(
            &leg.case,
            "the delivered prefix",
            stamp,
            &key,
            leg.form.resolution(),
            (put_at, read_at),
            leg.offset,
        );
        // The same instant in the same zone, in the object's own name.
        assert_in_zone(
            &leg.case,
            "the delivered object's name",
            object_name_stamp(&key, &leg.stream, &leg.case),
            &key,
            Resolution::Second,
            (put_at, read_at),
            leg.offset,
        );
    }
}

/// The error output a record whose partition keys could not be determined is written under
/// carries the same zone's digits: `ErrorOutputPrefix` is evaluated like any other prefix of
/// the destination ("A prefix that Firehose evaluates and adds to failed records before
/// writing them to S3" — `ErrorOutputPrefix`).
#[tokio::test]
async fn a_custom_time_zone_moves_the_error_output_prefix_of_the_same_destination() {
    let emulator = Emulator::new();
    let bucket = "custom-time-zone-error-bucket";
    emulator.create_bucket(bucket).await;

    let rows: Vec<ZoneRow> = zone_rows()
        .into_iter()
        .filter(|row| row.configured.is_none() || row.offset != 0)
        .collect();
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in rows.into_iter().enumerate() {
        let stream = format!("tz-err-{ordinal}");
        let leg = format!("{stream}-err/{EXTRACTION_FAILED}/");
        let mut config = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{bucket}"),
            // A key only a Lambda data transformation could supply, and this emulator runs
            // none: every record of this destination goes to the error output.
            "Prefix": format!("{stream}/!{{partitionKeyFromLambda:dept}}/"),
            "ErrorOutputPrefix": format!(
                "{stream}-err/!{{firehose:error-output-type}}/\
                 d=!{{timestamp:yyyy-MM-dd}}/h=!{{timestamp:HH}}/m=!{{timestamp:mm}}/"
            ),
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
            "CompressionFormat": "UNCOMPRESSED",
            "DynamicPartitioningConfiguration": { "Enabled": true },
            "ProcessingConfiguration": {
                "Enabled": true,
                "Processors": [{
                    "Type": "MetadataExtraction",
                    "Parameters": [
                        { "ParameterName": "MetadataExtractionQuery", "ParameterValue": "{dept:.dept}" },
                        { "ParameterName": "JsonParsingEngine", "ParameterValue": "JQ-1.6" },
                    ],
                }],
            },
        });
        if let Some(zone) = row.configured {
            config["CustomTimeZone"] = json!(zone);
        }
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": config,
                }),
            )
            .await;
        legs.push(Leg {
            case: format!("{} on an error output prefix", row.case),
            stream,
            leg,
            form: PrefixForm::Expressions,
            configured: row.configured,
            offset: row.offset,
        });
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let put_at = SystemTime::now();
    for leg in &legs {
        let (status, answered) = emulator.put_record(&leg.stream, RECORD).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must be accepted, got {answered:?}",
            leg.case
        );
    }

    for leg in &legs {
        let key = first_delivered_key(&emulator, &leg.case, bucket, &leg.leg).await;
        let read_at = SystemTime::now();
        assert_in_zone(
            &leg.case,
            "the error output prefix",
            expression_stamp(&key, &leg.leg, &leg.case),
            &key,
            Resolution::Minute,
            (put_at, read_at),
            leg.offset,
        );
        assert_in_zone(
            &leg.case,
            "the error object's name",
            object_name_stamp(&key, &leg.stream, &leg.case),
            &key,
            Resolution::Second,
            (put_at, read_at),
            leg.offset,
        );
    }
}

/// The zone in force at the FLUSH decides the digits, so an `UpdateDestination` that names
/// a `CustomTimeZone` moves every later delivery while the one already written stands —
/// "Describes an update for a destination in Amazon S3" (`ExtendedS3DestinationUpdate`).
#[tokio::test]
async fn an_update_naming_a_custom_time_zone_moves_every_later_delivery() {
    /// The zone the update names, and the offset it keeps all year.
    const ZONE: &str = "Asia/Kolkata";
    const OFFSET: i64 = 5 * 3_600 + 1_800;

    let emulator = Emulator::new();
    let bucket = "custom-time-zone-update-bucket";
    let stream = "tz-updated";
    let leg = "tz-updated/";
    emulator.create_bucket(bucket).await;
    emulator
        .firehose_ok(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": stream,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": format!("arn:aws:s3:::{bucket}"),
                    "Prefix": PrefixForm::Expressions.prefix(leg),
                    "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                    "CompressionFormat": "UNCOMPRESSED",
                },
            }),
        )
        .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // Before the update: the destination names no zone, so the default decides.
    let put_at = SystemTime::now();
    let (status, answered) = emulator.put_record(stream, RECORD).await;
    assert_eq!(status, 200, "the first put got {answered:?}");
    let first = first_delivered_key(&emulator, "before the update", bucket, leg).await;
    assert_in_zone(
        "before the update",
        "the delivered prefix",
        expression_stamp(&first, leg, "before the update"),
        &first,
        Resolution::Minute,
        (put_at, SystemTime::now()),
        0,
    );

    let description = describe(&emulator, stream).await;
    let version = description["VersionId"]
        .as_str()
        .expect("the description carries a version id")
        .to_string();
    let destination = description["Destinations"][0]["DestinationId"]
        .as_str()
        .expect("the description carries a destination id")
        .to_string();
    emulator
        .firehose_ok(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": stream,
                "CurrentDeliveryStreamVersionId": version,
                "DestinationId": destination,
                "ExtendedS3DestinationUpdate": { "CustomTimeZone": ZONE },
            }),
        )
        .await;
    let updated = describe(&emulator, stream).await;
    assert_eq!(
        updated["Destinations"][0]["ExtendedS3DestinationDescription"]["CustomTimeZone"].as_str(),
        Some(ZONE),
        "after the update: the description must echo the zone, got {updated:?}"
    );

    // After the update: the later delivery's digits are the same instant in the new zone,
    // and the object already written is untouched.
    let put_at = SystemTime::now();
    let (status, answered) = emulator.put_record(stream, RECORD).await;
    assert_eq!(status, 200, "the second put got {answered:?}");
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let second = loop {
        let keys = emulator.keys_under(bucket, leg).await;
        if let Some(key) = keys.iter().find(|key| **key != first) {
            break key.clone();
        }
        assert!(
            Instant::now() < deadline,
            "after the update: expected a second delivered object under {leg:?}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    let read_at = SystemTime::now();
    assert_in_zone(
        "after the update",
        "the delivered prefix",
        expression_stamp(&second, leg, "after the update"),
        &second,
        Resolution::Minute,
        (put_at, read_at),
        OFFSET,
    );
    assert_in_zone(
        "after the update",
        "the delivered object's name",
        object_name_stamp(&second, stream, "after the update"),
        &second,
        Resolution::Second,
        (put_at, read_at),
        OFFSET,
    );
}

/// One stream's description.
async fn describe(emulator: &Emulator, stream: &str) -> Value {
    emulator
        .firehose_ok(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await["DeliveryStreamDescription"]
        .clone()
}

/// The first key delivered under `prefix`, or a failure naming `case` once
/// [`DELIVERY_DEADLINE`] has passed.
async fn first_delivered_key(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    prefix: &str,
) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = emulator.keys_under(bucket, prefix).await;
        if let Some(key) = keys.into_iter().next() {
            return key;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object under {prefix:?} in {bucket}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Require `stamp` — a time field of `key` — to be the delivery's instant read in a zone
/// `offset` seconds from UTC.
///
/// The delivery happened within `window`, so in that zone the field can only be an instant
/// of that window shifted by the offset, truncated to the field's own `resolution`. For
/// every zone here that offset is hours away from UTC while the window is seconds wide, so
/// a field left in UTC falls outside it.
fn assert_in_zone(
    case: &str,
    field: &str,
    stamp: NaiveDateTime,
    key: &str,
    resolution: Resolution,
    window: (SystemTime, SystemTime),
    offset: i64,
) {
    let (from, to) = window;
    let earliest = resolution.truncate(shifted(from, offset));
    let latest = resolution.truncate(shifted(to, offset));
    assert!(
        stamp >= earliest && stamp <= latest,
        "case {case:?}: {field} must state the delivery's instant in a zone {offset}s from \
         UTC, i.e. between {earliest} and {latest}, got {stamp} in key {key:?}"
    );
}

/// `at` as the wall clock of a zone `offset` seconds from UTC reads it.
fn shifted(at: SystemTime, offset: i64) -> NaiveDateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch");
    let seconds =
        i64::try_from(since_epoch.as_secs()).expect("the instant fits the calendar's range");
    chrono::DateTime::from_timestamp(seconds + offset, 0)
        .expect("the instant is one the calendar places")
        .naive_utc()
}

/// The `d=<date>/h=<hour>/m=<minute>/` digits a key written under an expression prefix
/// carries right after `leg`.
fn expression_stamp(key: &str, leg: &str, case: &str) -> NaiveDateTime {
    let rest = after(key, &format!("{leg}d="), case);
    let (date, rest) = split_at(rest, "/h=", case, key);
    let (hour, rest) = split_at(rest, "/m=", case, key);
    let (minute, _) = split_at(rest, "/", case, key);
    assert!(
        date.len() == 10 && hour.len() == 2 && minute.len() == 2,
        "case {case:?}: the expanded date, hour and minute are as wide as their patterns, \
         got {date:?}, {hour:?} and {minute:?} in key {key:?}"
    );
    parsed(&format!("{date} {hour}:{minute}:00"), case, key)
}

/// The automatic `YYYY/MM/DD/HH/` segments a key carries right after `leg`.
fn automatic_stamp(key: &str, leg: &str, case: &str) -> NaiveDateTime {
    let rest = after(key, leg, case);
    let fields: Vec<&str> = rest.split('/').take(4).collect();
    assert!(
        fields.len() == 4
            && [4, 2, 2, 2]
                .iter()
                .zip(&fields)
                .all(|(width, field)| field.len() == *width
                    && field.bytes().all(|byte| byte.is_ascii_digit())),
        "case {case:?}: the automatic prefix carries YYYY/MM/DD/HH segments, got \
         {fields:?} in key {key:?}"
    );
    parsed(
        &format!(
            "{}-{}-{} {}:00:00",
            fields[0], fields[1], fields[2], fields[3]
        ),
        case,
        key,
    )
}

/// The timestamp a delivered object's own NAME carries:
/// `<stream>-<version>-<yyyy-MM-dd-HH-mm-ss>-<uuid>`.
fn object_name_stamp(key: &str, stream: &str, case: &str) -> NaiveDateTime {
    let name = key.rsplit('/').next().unwrap_or(key);
    let rest = after(name, &format!("{stream}-"), case);
    let fields: Vec<&str> = rest.split('-').collect();
    assert!(
        fields.len() > 7,
        "case {case:?}: a delivered object's name carries its stream, version, timestamp \
         and unique part, got {name:?}"
    );
    // The field after the stream's name is its `VersionId`; the six after that are the
    // calendar fields of the delivery.
    parsed(
        &format!(
            "{}-{}-{} {}:{}:{}",
            fields[1], fields[2], fields[3], fields[4], fields[5], fields[6]
        ),
        case,
        key,
    )
}

/// What is left of `text` after `head`, which it must start with.
fn after<'a>(text: &'a str, head: &str, case: &str) -> &'a str {
    text.strip_prefix(head)
        .unwrap_or_else(|| panic!("case {case:?}: {text:?} must start with {head:?}"))
}

/// `text` split at its first `separator`, which it must carry.
fn split_at<'a>(text: &'a str, separator: &str, case: &str, key: &str) -> (&'a str, &'a str) {
    text.split_once(separator).unwrap_or_else(|| {
        panic!("case {case:?}: {text:?} of key {key:?} must carry {separator:?}")
    })
}

/// `text` as the calendar timestamp its digits state.
fn parsed(text: &str, case: &str, key: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap_or_else(|error| {
        panic!("case {case:?}: {text:?} out of key {key:?} must be a calendar timestamp: {error}")
    })
}
