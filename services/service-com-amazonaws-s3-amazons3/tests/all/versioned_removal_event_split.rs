// SPDX-License-Identifier: CC0-1.0

//! What a bucket's notification configuration does when a VERSIONED delete removes
//! something: which queue hears about it, and what the record it receives says.
//!
//! Two rules are under test, both over the `ObjectRemoved` class a versioned bucket
//! produces two different events in:
//!
//! * a delete with no `versionId` on a bucket that carries a versioning state "inserts a
//!   delete marker, which becomes the current version of the object" (`DeleteObject`'s own
//!   documentation), which is the `s3:ObjectRemoved:DeleteMarkerCreated` event, while a
//!   delete that names a `versionId` permanently removes that version, which is
//!   `s3:ObjectRemoved:Delete` — and each is delivered to the destination configurations
//!   whose `Events` list names THAT event and to no other, carrying the version id the
//!   call acted on and no object size, because the change left no object at the key;
//! * every record a notification publishes names the CALL it came of — not only by the
//!   request-correlation pair in its `responseElements` but by the address the call was
//!   made from, which the Amazon S3 event message structure states as
//!   `requestParameters.sourceIPAddress` ("IP address where request came from"). It is
//!   read off the request's own connection, so a call forwarded by a proxy is reported as
//!   coming from the client that proxy names.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 beside Amazon
//! SQS, and reads what a consumer really received off the queue, so what is pinned is the
//! wire behaviour rather than any handler's internals. The harness is the one the
//! single-destination half of this rule uses ([`super::object_event_notifications`]) —
//! this suite adds the destination SPLIT, which a configuration with one destination
//! cannot show.
//!
//! The rows span what the rules generalise over: the bucket's versioning state (enabled,
//! suspended, none), whether the delete names a version, how the configuration names the
//! event (the value itself or the `s3:ObjectRemoved:*` wildcard), which of the two queues
//! is the matching one, and — for the second rule — whether the call was forwarded.

use serde_json::Value;

use super::object_event_notifications::{queue_arn, Answer, Emulator, XMLNS};

/// The key every row writes and then deletes.
const KEY: &str = "ledger/entry.bin";

/// The body that write carries, so a record reporting a size would report THIS many bytes
/// — which is what makes "no size at all" an assertable fact rather than a default.
const BODY: &[u8] = b"versioned-removal";

/// The address a record reports when nothing forwarded the call: this emulator serves its
/// API over the loopback interface, so a connection that reached it came from the local
/// host.
const LOOPBACK: &str = "127.0.0.1";

/// A client address a proxy states it forwarded for, from the documentation range
/// RFC 5737 reserves for it.
const FORWARDED_CLIENT: &str = "203.0.113.7";

/// One queue destination configuration of the container a row installs.
struct QueueTarget<'a> {
    /// The configuration's `Id`, which every delivered record reports as its
    /// `configurationId`.
    id: &'a str,
    /// The queue's NAME; its ARN and url are resolved from it.
    queue: &'a str,
    /// The `Event` values this configuration is notified about.
    events: &'a [&'a str],
}

/// The versioning state a row's bucket carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Versioning {
    /// No `VersioningConfiguration` at all: a delete removes the object.
    None,
    /// `Status=Enabled`: a delete with no `versionId` lays a delete marker.
    Enabled,
    /// `Status=Suspended`: a delete still lays a marker, under the `null` version id.
    Suspended,
}

impl Versioning {
    /// The `Status` a `PutBucketVersioning` carries for this state, or `None` when the
    /// bucket is left with no versioning configuration.
    fn status(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Enabled => Some("Enabled"),
            Self::Suspended => Some("Suspended"),
        }
    }
}

/// `PUT /<bucket>?versioning`, when the row's bucket carries a versioning state.
async fn set_versioning(emulator: &Emulator, case: &str, bucket: &str, versioning: Versioning) {
    let Some(status) = versioning.status() else {
        return;
    };
    let body = format!(
        "<VersioningConfiguration xmlns=\"{XMLNS}\"><Status>{status}</Status>\
         </VersioningConfiguration>"
    );
    let answer = emulator
        .s3_xml("PUT", &format!("/{bucket}?versioning"), &body)
        .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: putting versioning {status} on {bucket} must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// `PUT /<bucket>?notification` with ONE `QueueConfiguration` per entry of `targets`.
///
/// The MULTI-destination form, which the harness's own single-destination
/// [`Emulator::put_notification`] cannot express — and the split across two destinations
/// is the whole of what this suite asserts, so the container is built here.
async fn put_notification(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    targets: &[QueueTarget<'_>],
) {
    let mut body = format!("<NotificationConfiguration xmlns=\"{XMLNS}\">");
    for target in targets {
        body.push_str("<QueueConfiguration>");
        body.push_str(&format!("<Id>{}</Id>", target.id));
        body.push_str(&format!("<Queue>{}</Queue>", queue_arn(target.queue)));
        for event in target.events {
            body.push_str(&format!("<Event>{event}</Event>"));
        }
        body.push_str("</QueueConfiguration>");
    }
    body.push_str("</NotificationConfiguration>");
    let answer = emulator
        .s3_xml("PUT", &format!("/{bucket}?notification"), &body)
        .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: configuring notifications on {bucket} must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// `DELETE /<bucket>/<key>`, naming `version_id` when the row's delete addresses one and
/// carrying `forwarded_for` as the address a proxy states it forwarded the call for.
async fn delete_object(
    emulator: &Emulator,
    case: &str,
    bucket: &str,
    version_id: Option<&str>,
    forwarded_for: Option<&str>,
) -> Answer {
    let uri = match version_id {
        Some(version_id) => format!("/{bucket}/{KEY}?versionId={version_id}"),
        None => format!("/{bucket}/{KEY}"),
    };
    let headers: Vec<(&str, &str)> = forwarded_for
        .map(|address| vec![("x-forwarded-for", address)])
        .unwrap_or_default();
    let answer = emulator
        .send("s3", "DELETE", &uri, &headers, Vec::new())
        .await;
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: deleting {KEY:?} must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
    answer
}

/// The `Records` envelope the queue at `url` received, or `None` when nothing arrived.
async fn record_on(emulator: &Emulator, url: &str) -> Option<Value> {
    emulator
        .delivered(url)
        .await
        .map(|message| message["Records"][0].clone())
}

/// Take the `s3:TestEvent` a validated queue destination receives off the queue, so the
/// rows below read only what their own delete delivered.
async fn take_test_event(emulator: &Emulator, case: &str, url: &str) {
    let taken = emulator.delivered(url).await.unwrap_or_else(|| {
        panic!("case {case:?}: a validated queue destination must receive the test event")
    });
    assert_eq!(
        taken["Event"].as_str(),
        Some("s3:TestEvent"),
        "case {case:?}: the first message a validated destination receives must be the test \
         notification, got {taken}"
    );
}

/// Which delete the row makes, and so which `ObjectRemoved` event it owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// `DeleteObject` with no `versionId`.
    Current,
    /// `DeleteObject` naming the version the write created.
    Version,
}

/// What the record must say about the version the removal acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportedVersion {
    /// It names the version the delete's own response named.
    TheCallsOwn,
    /// It names none, because what the removal took away carried none.
    Nothing,
    /// Left unasserted: the `null` delete marker a versioning-SUSPENDED bucket lays
    /// carries no version id of its own, and the event message structure's note ("the
    /// object version if bucket is versioning-enabled, otherwise null") does not say
    /// whether the member is then absent or carries the `null` id. The row still pins the
    /// event and the destination, which is what the split turns on.
    Unasserted,
}

/// One row of the table.
struct Row {
    case: &'static str,
    versioning: Versioning,
    /// The `Events` list of the FIRST queue configuration.
    first_events: &'static [&'static str],
    /// The `Events` list of the SECOND one.
    second_events: &'static [&'static str],
    removal: Removal,
    /// The `eventName` of the record the matching queue must receive.
    expected_event_name: &'static str,
    /// What that record must say about the version acted on.
    reported_version: ReportedVersion,
    /// Which configuration must receive it — `true` for the first, `false` for the second.
    expects_first: bool,
}

#[tokio::test]
async fn each_removal_reaches_only_the_configuration_whose_events_name_it() {
    let rows: &[Row] = &[
        Row {
            case: "a delete with no versionId on a versioning-enabled bucket",
            versioning: Versioning::Enabled,
            first_events: &["s3:ObjectRemoved:DeleteMarkerCreated"],
            second_events: &["s3:ObjectRemoved:Delete"],
            removal: Removal::Current,
            expected_event_name: "ObjectRemoved:DeleteMarkerCreated",
            reported_version: ReportedVersion::TheCallsOwn,
            expects_first: true,
        },
        Row {
            case: "a delete naming a versionId on a versioning-enabled bucket",
            versioning: Versioning::Enabled,
            first_events: &["s3:ObjectRemoved:DeleteMarkerCreated"],
            second_events: &["s3:ObjectRemoved:Delete"],
            removal: Removal::Version,
            expected_event_name: "ObjectRemoved:Delete",
            reported_version: ReportedVersion::TheCallsOwn,
            expects_first: false,
        },
        Row {
            case: "the same split with the two configurations in the other order",
            versioning: Versioning::Enabled,
            first_events: &["s3:ObjectRemoved:Delete"],
            second_events: &["s3:ObjectRemoved:DeleteMarkerCreated"],
            removal: Removal::Current,
            expected_event_name: "ObjectRemoved:DeleteMarkerCreated",
            reported_version: ReportedVersion::TheCallsOwn,
            expects_first: false,
        },
        Row {
            case: "a delete with no versionId on a versioning-suspended bucket",
            versioning: Versioning::Suspended,
            first_events: &["s3:ObjectRemoved:DeleteMarkerCreated"],
            second_events: &["s3:ObjectRemoved:Delete"],
            removal: Removal::Current,
            expected_event_name: "ObjectRemoved:DeleteMarkerCreated",
            reported_version: ReportedVersion::Unasserted,
            expects_first: true,
        },
        Row {
            case: "a delete on a bucket with no versioning state, which removes the object",
            versioning: Versioning::None,
            first_events: &["s3:ObjectRemoved:DeleteMarkerCreated"],
            second_events: &["s3:ObjectRemoved:Delete"],
            removal: Removal::Current,
            expected_event_name: "ObjectRemoved:Delete",
            reported_version: ReportedVersion::Nothing,
            expects_first: false,
        },
        Row {
            case: "the removal wildcard, which stands for every event of the type",
            versioning: Versioning::Enabled,
            first_events: &["s3:ObjectRemoved:*"],
            second_events: &["s3:ObjectTagging:Put"],
            removal: Removal::Current,
            expected_event_name: "ObjectRemoved:DeleteMarkerCreated",
            reported_version: ReportedVersion::TheCallsOwn,
            expects_first: true,
        },
    ];

    let emulator = Emulator::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let bucket = format!("removal-split-{index}");
        let first = format!("removal-first-{index}");
        let second = format!("removal-second-{index}");
        emulator.create_bucket(&bucket).await;
        set_versioning(&emulator, case, &bucket, row.versioning).await;
        let first_url = emulator.create_queue(&first).await;
        let second_url = emulator.create_queue(&second).await;
        put_notification(
            &emulator,
            case,
            &bucket,
            &[
                QueueTarget {
                    id: "first",
                    queue: &first,
                    events: row.first_events,
                },
                QueueTarget {
                    id: "second",
                    queue: &second,
                    events: row.second_events,
                },
            ],
        )
        .await;
        take_test_event(&emulator, case, &first_url).await;
        take_test_event(&emulator, case, &second_url).await;

        // The write the row's delete then removes. On a bucket that keeps versions the
        // response names the version it created, which is the version the `Version` row
        // deletes by id.
        let written = emulator.put_object(&bucket, KEY, BODY).await;
        let written_version = written.header("x-amz-version-id");

        let removed = match row.removal {
            Removal::Current => delete_object(&emulator, case, &bucket, None, None).await,
            Removal::Version => {
                let version_id = written_version.clone().unwrap_or_else(|| {
                    panic!("case {case:?}: a write to a versioned bucket must answer a version id")
                });
                delete_object(&emulator, case, &bucket, Some(&version_id), None).await
            }
        };
        // "Returns the version ID of the delete marker created as a result of the DELETE
        // operation" / the version a delete that named one removed: the record has to
        // report the SAME version the response named, so a consumer can address it.
        let reported_version = removed.header("x-amz-version-id");

        let (matching_url, quiet_url) = if row.expects_first {
            (&first_url, &second_url)
        } else {
            (&second_url, &first_url)
        };
        let record = record_on(&emulator, matching_url).await.unwrap_or_else(|| {
            panic!("case {case:?}: the configuration naming this event must receive a record")
        });
        assert_eq!(
            record["eventName"].as_str(),
            Some(row.expected_event_name),
            "case {case:?}: expected eventName {:?}, got {record}",
            row.expected_event_name
        );
        match row.reported_version {
            // The version the delete's own response named — "Returns the version ID of the
            // delete marker created as a result of the DELETE operation" / the version a
            // delete that named one removed — so a consumer can address exactly what the
            // record says happened.
            ReportedVersion::TheCallsOwn => assert_eq!(
                record["s3"]["object"]["versionId"]
                    .as_str()
                    .map(str::to_string),
                reported_version,
                "case {case:?}: the record must name the version the response named, got \
                 {record}"
            ),
            ReportedVersion::Nothing => assert!(
                record["s3"]["object"]["versionId"].is_null(),
                "case {case:?}: a removal of an object that carried no version id names \
                 none, got {record}"
            ),
            ReportedVersion::Unasserted => {}
        }
        // A removal leaves no object at the key, so the record reports neither its size
        // nor its entity tag — the two members an `ObjectCreated` record does carry.
        assert!(
            record["s3"]["object"]["size"].is_null(),
            "case {case:?}: a removal record reports no size, got {record}"
        );
        assert!(
            record["s3"]["object"]["eTag"].is_null(),
            "case {case:?}: a removal record reports no entity tag, got {record}"
        );
        // And the sibling configuration, which names the OTHER event, hears nothing: the
        // split is only observable because this queue stays quiet.
        let quiet = emulator.nothing_delivered(quiet_url).await;
        assert!(
            quiet.is_empty(),
            "case {case:?}: the configuration naming the other event must receive nothing, \
             got {quiet:?}"
        );

        // What the bucket HOLDS afterwards agrees with what the record said happened: a
        // delete marker is listed as one under the version id the record named, and a
        // version the delete removed permanently is listed no more.
        let listed = emulator
            .s3_xml("GET", &format!("/{bucket}?versions"), "")
            .await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: listing the versions must succeed, got {} with {}",
            listed.status,
            listed.text()
        );
        let listing = listed.text();
        match row.removal {
            Removal::Current => {
                if let Some(marker) = reported_version.as_deref() {
                    assert!(
                        listing.contains("<DeleteMarker>")
                            && listing.contains(&format!("<VersionId>{marker}</VersionId>")),
                        "case {case:?}: the marker the record named must be listed as a \
                         delete marker, got {listing}"
                    );
                }
            }
            Removal::Version => {
                let removed = written_version.clone().unwrap_or_default();
                assert!(
                    !listing.contains(&format!("<VersionId>{removed}</VersionId>")),
                    "case {case:?}: the version the record said was removed must be listed \
                     no more, got {listing}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_record_names_the_address_its_call_was_made_from() {
    // (case, the address a proxy states it forwarded for, the address the record must
    // report)
    let rows: &[(&str, Option<&str>, &str)] = &[
        ("a call nothing forwarded", None, LOOPBACK),
        (
            "a call a proxy forwarded for one client",
            Some(FORWARDED_CLIENT),
            FORWARDED_CLIENT,
        ),
        (
            "a call forwarded through two proxies, whose first entry is the client",
            Some("198.51.100.23, 192.0.2.44"),
            "198.51.100.23",
        ),
        (
            // The member is an ADDRESS, so a forwarded entry that is not one names no
            // address the record could report: it is answered with the loopback host
            // rather than published verbatim.
            "a call forwarded with a value that is not an address at all",
            Some("not-an-address"),
            LOOPBACK,
        ),
        (
            "a call forwarded with an IPv6 client address",
            Some("2001:db8::7"),
            "2001:db8::7",
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, forwarded_for, expected)) in rows.iter().enumerate() {
        let bucket = format!("removal-source-{index}");
        let queue = format!("removal-source-queue-{index}");
        emulator.create_bucket(&bucket).await;
        set_versioning(&emulator, case, &bucket, Versioning::Enabled).await;
        let url = emulator.create_queue(&queue).await;
        put_notification(
            &emulator,
            case,
            &bucket,
            &[QueueTarget {
                id: "removals",
                queue: &queue,
                events: &["s3:ObjectRemoved:*"],
            }],
        )
        .await;
        take_test_event(&emulator, case, &url).await;
        emulator.put_object(&bucket, KEY, BODY).await;

        delete_object(&emulator, case, &bucket, None, *forwarded_for).await;
        let record = record_on(&emulator, &url).await.unwrap_or_else(|| {
            panic!("case {case:?}: the configured destination must receive a record")
        });
        assert_eq!(
            record["requestParameters"]["sourceIPAddress"].as_str(),
            Some(*expected),
            "case {case:?}: expected the record to name {expected:?} as the address the call \
             was made from, got {record}"
        );
    }
}
