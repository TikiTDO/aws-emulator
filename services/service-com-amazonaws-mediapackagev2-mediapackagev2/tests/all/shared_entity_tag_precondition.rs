// SPDX-License-Identifier: CC0-1.0

//! The ONE conditional-write precondition every writer of this service resolves through,
//! driven over HTTP through the same router the emulator boots.
//!
//! The rule, asserted over a SET of presented entity tags rather than one recorded
//! request: a conditional write proceeds when the request presents no entity tag at all
//! (it is unconditional) or presents exactly the tag the store holds the addressed
//! resource at, and is refused with `ConflictException` (409) / `CONFLICTING_OPERATION` —
//! writing nothing — when it presents any other tag the member admits.
//!
//! The suite drives the rule through all four operations that bind the `ETag` member to
//! `x-amzn-update-if-match` — `UpdateChannelGroup`, `UpdateChannel`,
//! `UpdateOriginEndpoint` and `CancelHarvestJob` — because each resolved the precondition
//! through its own copy of the comparison: a row here holds every one of them to the same
//! answer, whichever resource it writes.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

// The crate's one emulator client and error-envelope reader, plus the one shared
// `CreateHarvestJob` body, so this suite drives the router exactly as the other suites do
// rather than growing a second copy of the dispatch boilerplate.
use crate::channel_lifecycle::{error_type, Emulator};
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};

/// The header each of these four operations binds its `ETag` member to
/// (`@httpHeader("x-amzn-update-if-match")`).
///
/// `pub(crate)` with [`STALE_ETAG_MESSAGE`], [`read`], [`message`] and [`settle_job`]:
/// they are this crate's ONE description of the shared precondition, so
/// `entity_tag_precondition_precedence` drives it through them rather than carrying a
/// second copy.
pub(crate) const IF_MATCH: &str = "x-amzn-update-if-match";

/// The fixed sentence a presented-but-not-committed entity tag is refused with. It is
/// spelled out here rather than read back from the crate: the service's error text is
/// fixed per rejection class and carries nothing from the request, so the sentence itself
/// is part of the contract (note the typographic apostrophes).
pub(crate) const STALE_ETAG_MESSAGE: &str = "You\u{2019}ve specified an entity tag with your request \
    that does not match the resource\u{2019}s current entity tag. This can occur if the \
    resource has been updated by another request between the time you retrieved the ETag \
    and when you made your update request";

/// The sizes `EntityTag` admits (`smithy_query(query="shape",shape="EntityTag")` —
/// `@length` min 1, max 256, pattern `^[\S]+$`), so the tags presented below span the
/// whole member rather than only strings of a convenient size.
const ENTITY_TAG_MIN_CHARS: usize = 1;
/// See [`ENTITY_TAG_MIN_CHARS`].
const ENTITY_TAG_MAX_CHARS: usize = 256;

/// The channel, origin endpoint and harvest job each row builds inside its own channel
/// group, and the manifest the endpoint publishes and its harvest job harvests.
const CHANNEL: &str = "chan";
/// See [`CHANNEL`].
const ENDPOINT: &str = "end";
/// See [`CHANNEL`].
const JOB: &str = "job";
/// See [`CHANNEL`].
const MANIFEST: &str = "index";

/// The description an accepted write stores, for the operations that govern one.
const DESCRIPTION: &str = "committed under the precondition";

/// The entity tag a row presents in `x-amzn-update-if-match`.
#[derive(Debug)]
enum Presented {
    /// No header at all: the write is unconditional.
    Absent,
    /// The tag the store holds the resource at.
    Committed,
    /// A tag the resource really reported, until another request committed over it.
    Stale,
    /// The committed tag with one more character on the end, which a comparison that
    /// matched a PREFIX rather than the whole tag would accept.
    NearMiss,
    /// A tag of this resource never had, of a size the member admits.
    Other(String),
}

impl Presented {
    /// Whether the conditional write presenting this tag may proceed.
    fn accepted(&self) -> bool {
        matches!(self, Presented::Absent | Presented::Committed)
    }
}

/// One conditional writer: an operation that binds `x-amzn-update-if-match`, with the
/// resource it writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    ChannelGroup,
    Channel,
    OriginEndpoint,
    HarvestJob,
}

impl Writer {
    /// Every operation of this service that binds the precondition header.
    const ALL: [Writer; 4] = [
        Writer::ChannelGroup,
        Writer::Channel,
        Writer::OriginEndpoint,
        Writer::HarvestJob,
    ];

    fn operation(self) -> &'static str {
        match self {
            Writer::ChannelGroup => "UpdateChannelGroup",
            Writer::Channel => "UpdateChannel",
            Writer::OriginEndpoint => "UpdateOriginEndpoint",
            Writer::HarvestJob => "CancelHarvestJob",
        }
    }

    /// Build a fresh resource of this writer's kind inside a channel group named after
    /// `suffix`, and return the request path both its reader and its conditional writer
    /// address (each of these four operations is a `PUT` on the resource's own path, which
    /// its `Get` reads with a `GET`).
    async fn fresh(self, emulator: &Emulator, suffix: &str) -> String {
        let group = format!("grp-{suffix}");
        let (status, body) = emulator.create_group(&group, json!({})).await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateChannelGroup {group} expected 200, got {status} {body}"
        );
        let group_path = format!("/channelGroup/{group}");
        if self == Writer::ChannelGroup {
            return group_path;
        }

        let (status, body) = emulator.create_channel(&group, CHANNEL, json!({})).await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateChannel in {group} expected 200, got {status} {body}"
        );
        // `UpdateChannel`'s URI keeps the trailing slash the model gives it.
        let channel_path = format!("{group_path}/channel/{CHANNEL}/");
        if self == Writer::Channel {
            return channel_path;
        }

        let (status, body) = emulator
            .create_endpoint(
                &group,
                CHANNEL,
                ENDPOINT,
                json!({"ContainerType": "TS", "HlsManifests": [{"ManifestName": MANIFEST}]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateOriginEndpoint in {group} expected 200, got {status} {body}"
        );
        let endpoint_path = format!("{group_path}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}");
        if self == Writer::OriginEndpoint {
            return endpoint_path;
        }

        // A harvest job whose scheduled window is OPEN NOW and ends long after this test
        // does: the queued transition below falls due within the emulator's own span, and
        // nothing else the clock owes it comes due while the row runs.
        let start = epoch_seconds_from_now(0.0);
        let mut request = harvest_body(start, start + 3600.0, &[MANIFEST]);
        request["HarvestJobName"] = json!(JOB);
        let (status, body) = emulator
            .call("POST", &format!("{endpoint_path}/harvestJob"), Some(request))
            .await;
        assert_eq!(
            status, 200,
            "case \"setup\": CreateHarvestJob in {group} expected 200, got {status} {body}"
        );
        let job_path = format!("{endpoint_path}/harvestJob/{JOB}");
        settle_job(emulator, &job_path).await;
        job_path
    }

    /// The body this writer's conditional write sends: the members its input requires,
    /// plus the description the operations that govern one store.
    fn body(self) -> Option<Value> {
        match self {
            Writer::ChannelGroup | Writer::Channel => Some(json!({"Description": DESCRIPTION})),
            // `ContainerType` is a required `UpdateOriginEndpointRequest` member, and an
            // endpoint needs at least one manifest, so the write re-sends the packaging it
            // was created with and changes only the description.
            Writer::OriginEndpoint => Some(json!({
                "ContainerType": "TS",
                "Description": DESCRIPTION,
                "HlsManifests": [{"ManifestName": MANIFEST}],
            })),
            // `CancelHarvestJobRequest` binds no body member at all.
            Writer::HarvestJob => None,
        }
    }

    /// What an ACCEPTED write must leave behind, as a member of the resource and the value
    /// the reader must then report: the three updates store the description they sent, and
    /// the cancellation moves the job's status.
    fn committed(self) -> (&'static str, &'static str) {
        match self {
            Writer::HarvestJob => ("Status", "CANCELLED"),
            _ => ("Description", DESCRIPTION),
        }
    }
}

/// Read the resource at `path` back through its own reader.
pub(crate) async fn read(emulator: &Emulator, case: &str, path: &str) -> Value {
    let (status, body) = emulator.call("GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {path} back expected 200, got {status} {body}"
    );
    body
}

/// Poll a fresh harvest job until the `QUEUED` -> `IN_PROGRESS` transition the clock owes
/// it has been materialised.
///
/// That transition commits a new version of the job, which rotates its entity tag, so a
/// row must present a tag taken AFTER it: once the job is past `QUEUED` the next
/// transition its schedule owes is an hour out, so the tag the row reads stays the
/// committed one.
pub(crate) async fn settle_job(emulator: &Emulator, path: &str) {
    for _ in 0..200u32 {
        let body = read(emulator, "setup", path).await;
        if body["Status"] != json!("QUEUED") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("case \"setup\": the harvest job at {path} never left QUEUED");
}

/// How often, and for how long, [`ended_job`] waits: comfortably past the two transition
/// spans the emulator owes a job whose window had already closed, so one budget is
/// re-scaled in one place when that span changes.
///
/// `pub(crate)` because the suites whose own client type this waiter cannot take
/// (`harvest_job_refusal_text`, which drives the data-plane-capable client) still wait on
/// THIS budget rather than inventing a second one.
pub(crate) const END_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub(crate) const END_POLL_LIMIT: u32 = 600;

/// Poll a harvest job created for a window that had already CLOSED until the clock has
/// ENDED it, which is the status `CancelHarvestJob` refuses as ineligible.
///
/// The counterpart of [`settle_job`] for the other transition the clock owes a job, and
/// `pub(crate)` for the same reason: the suites that need a job in that status wait for it
/// here rather than each carrying its own poll and its own budget. Nobody touches the job,
/// so the status it reaches is the clock's.
pub(crate) async fn ended_job(emulator: &Emulator, case: &str, path: &str) {
    for _ in 0..END_POLL_LIMIT {
        let body = read(emulator, case, path).await;
        if body["Status"] == json!("FAILED") {
            return;
        }
        tokio::time::sleep(END_POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the clock never ended the harvest job at {path}");
}

/// The `Message` member of an error envelope (`""` when absent).
pub(crate) fn message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

#[tokio::test]
async fn every_conditional_writer_resolves_one_entity_tag_precondition() {
    let emulator = Emulator::new();

    // The tags presented, differing in how each relates to the committed one and in its
    // form and size: absent, the committed tag itself, a tag the resource really held
    // until another request committed over it, the committed tag with one character
    // appended, and tags the resource never had at the shortest and the longest size the
    // member admits plus one of a middling hex shape.
    let cases = || {
        vec![
            Presented::Absent,
            Presented::Committed,
            Presented::Stale,
            Presented::NearMiss,
            Presented::Other("x".repeat(ENTITY_TAG_MIN_CHARS)),
            Presented::Other("deadbeef00112233".to_string()),
            Presented::Other("a".repeat(ENTITY_TAG_MAX_CHARS)),
        ]
    };

    for (writer_index, writer) in Writer::ALL.into_iter().enumerate() {
        for (row, presented) in cases().into_iter().enumerate() {
            let case = format!("{} presenting {presented:?}", writer.operation());
            // A fresh resource per row: an accepted write moves the resource on, and an
            // accepted `CancelHarvestJob` leaves its job terminal.
            let path = writer
                .fresh(&emulator, &format!("{writer_index}-{row}"))
                .await;
            let mut before = read(&emulator, &case, &path).await;
            let committed = before["ETag"].as_str().unwrap_or_default().to_string();
            assert!(
                !committed.is_empty(),
                "case {case:?}: the reader must report the resource's committed entity tag, \
                 got {before}"
            );

            let tag: Option<String> = match &presented {
                Presented::Absent => None,
                Presented::Committed => Some(committed.clone()),
                Presented::NearMiss => Some(format!("{committed}0")),
                Presented::Other(tag) => Some(tag.clone()),
                Presented::Stale => {
                    // Somebody else has to commit over the resource for the tag in hand to
                    // go stale, and a tag write is such a commit: the tags are part of
                    // every record's content, so writing one rotates the entity tag.
                    let arn = before["Arn"].as_str().unwrap_or_default().to_string();
                    let tags =
                        BTreeMap::from([("committed-by".to_string(), "another-request".to_string())]);
                    let (status, body) = emulator.tag(&arn, &tags).await;
                    assert_eq!(
                        status, 204,
                        "case {case:?}: TagResource on {arn} expected 204, got {status} {body}"
                    );
                    before = read(&emulator, &case, &path).await;
                    assert_ne!(
                        before["ETag"],
                        json!(committed),
                        "case {case:?}: a commit by another request must rotate the entity \
                         tag, got {before}"
                    );
                    Some(committed.clone())
                }
            };

            let headers: Vec<(&str, &str)> = tag
                .iter()
                .map(|tag| (IF_MATCH, tag.as_str()))
                .collect::<Vec<_>>();
            let (status, answer) = emulator
                .call_with_headers("PUT", &path, writer.body(), &headers)
                .await;

            if presented.accepted() {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the precondition holds, so the write must commit; got \
                     {status} {answer}"
                );
                let (member, expected) = writer.committed();
                let after = read(&emulator, &case, &path).await;
                assert_eq!(
                    after[member],
                    json!(expected),
                    "case {case:?}: an accepted write must be what the read reports for \
                     {member}; got {after}"
                );
            } else {
                assert_eq!(
                    status, 409,
                    "case {case:?}: a tag that is not the committed one must be refused 409, \
                     got {status} {answer}"
                );
                assert_eq!(
                    error_type(&answer),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {answer}"
                );
                assert_eq!(
                    answer["ConflictExceptionType"],
                    json!("CONFLICTING_OPERATION"),
                    "case {case:?}: expected CONFLICTING_OPERATION, got {answer}"
                );
                assert_eq!(
                    message(&answer),
                    STALE_ETAG_MESSAGE,
                    "case {case:?}: expected the fixed sentence for this rejection class, \
                     got {answer}"
                );
                let after = read(&emulator, &case, &path).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused write must leave the resource exactly as it \
                     was; got {after} against {before}"
                );
            }
        }
    }
}
