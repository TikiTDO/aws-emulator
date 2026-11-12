// SPDX-License-Identifier: CC0-1.0

//! The AMBIGUITY of a notification document whose configurations overlap through an
//! `s3:<Type>:*` EVENT: a type's wildcard stands for every event of that type, so a
//! configuration naming it and one naming an event of that type would both be matched by a
//! single object event, and S3 refuses the whole document rather than storing a
//! configuration whose delivery nothing could decide.
//!
//! The rule under test: **two destination configurations of one `NotificationConfiguration`
//! may not both be matched by a single object event — an `Events` entry covers the event
//! value itself and, in the `s3:<Type>:*` form, every published event of that type, and two
//! configurations are both kept only when their events are disjoint or their key-name
//! filters are — and a document that breaks it is refused whole with HTTP 400
//! `InvalidArgument` and leaves the bucket's stored configuration exactly as it was.**
//!
//! WHICH sentence it is refused with follows from why the two cannot both be in force: a
//! pair that names one event value between them is told apart by key filters alone, so a
//! pair whose filters also overlap is ambiguous ([`AMBIGUOUS`]); a pair that does NOT name
//! the same value and still shares an event — one names the type wildcard — overlaps in the
//! event ([`OVERLAPPING_EVENTS`]), and that refusal names the `Event` argument and the two
//! values.
//!
//! Where it comes from:
//!
//! * `FilterRule$Name` publishes the constraint itself: "overlapping prefixes and suffixes
//!   are not supported", and S3's own sentence for a pair that nothing in its filters tells
//!   apart is the fixed [`AMBIGUOUS`] text.
//! * The wildcard form is the model's: `Event`
//!   (`smithy_query(query="shape",shape="Event")`) publishes `s3:ObjectTagging:*` beside
//!   `s3:ObjectTagging:Put` and `s3:ObjectTagging:Delete`, and the wildcard is "every event
//!   of that type" — which is exactly how a DELIVERY is matched, so it is how an overlap
//!   has to be judged too. S3's sentence for that pair is the fixed [`OVERLAPPING_EVENTS`]
//!   text: two configurations of one bucket cannot share an event type.
//! * "The PUT notification is an atomic operation … If the message fails, the entire PUT
//!   action will fail, and Amazon S3 will not add the configuration to your bucket"
//!   (`PutBucketNotificationConfiguration`'s own documentation): a refused document changes
//!   nothing.
//!
//! The rows span what the rule generalises over: which type the wildcard belongs to, which
//! side of the document carries it, whether the other configuration names one event or
//! several, and what tells two configurations apart when something does (disjoint prefixes,
//! disjoint suffixes, disjoint event types). Beyond the written table, EVERY wildcard the
//! published `Event` catalog carries is paired with EVERY event of its own type, so a value
//! no row names is covered as well as the ones that are.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour — status, error code, message — and the configuration a later read
//! reports.

use service_com_amazonaws_s3_amazons3::state::EVENT_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace a client's S3 payload carries.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The two SQS queue ARNs the configurations below publish to. A destination is not what
/// makes two configurations distinguishable — "which destination a configuration publishes
/// to does not make two rules tell an object apart" — so the rows use two queues of this
/// account to keep the documents realistic.
const FIRST_QUEUE_ARN: &str = "arn:aws:sqs:us-east-1:123456789012:object-events-one";
const SECOND_QUEUE_ARN: &str = "arn:aws:sqs:us-east-1:123456789012:object-events-two";

/// The fixed `<Message>` of each class a conflicting document falls in, and the `<Code>`
/// both carry: two configurations that share an event TYPE without naming the same value
/// (one names the `s3:<Type>:*` form), and two that name ONE event value and are told apart
/// by nothing in their key filters.
const OVERLAPPING_EVENTS: &str = "Configurations overlap. Configurations on the same bucket \
                                  cannot share a common event type.";
const AMBIGUOUS: &str = "Configuration is ambiguously defined. Cannot have overlapping \
                         suffixes in two rules if the prefixes are overlapping for the same \
                         event type.";
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The `<ArgumentName>` the shared-event-type class names: the request field that overlap is
/// IN. S3's `InvalidArgument` document carries an `<ArgumentName>`/`<ArgumentValue>` pair
/// beside its code and message (Amazon S3 API Reference, *Error responses*), and that class's
/// sentence says nothing about which events overlapped — so the pair is the whole of what a
/// client reads to find that out. The ambiguous-filters class names no argument.
const EVENT_ARGUMENT: &str = "Event";

/// What a refused document's `InvalidArgument` document must carry.
#[derive(Debug, Clone, Copy)]
struct Refused {
    message: &'static str,
    /// The `<ArgumentName>`, or `None` for a refusal whose document names no argument.
    argument: Option<&'static str>,
    /// The `<ArgumentValue>`, or `None` where the refusal names no argument at all.
    value: Option<ArgumentValue>,
}

impl Refused {
    /// The two configurations share an event type through the wildcard: the two `Event`
    /// values they overlap in — each configuration's own, in document order — are what the
    /// refusal reports.
    fn events(first: &'static str, second: &'static str) -> Self {
        Self {
            message: OVERLAPPING_EVENTS,
            argument: Some(EVENT_ARGUMENT),
            value: Some(ArgumentValue::Pair(first, second)),
        }
    }

    /// The two configurations name ONE event value and nothing in their key filters tells
    /// them apart.
    fn filters() -> Self {
        Self {
            message: AMBIGUOUS,
            argument: None,
            value: None,
        }
    }
}

/// The `<ArgumentValue>` a refusal carries, as the two halves it is written from.
#[derive(Debug, Clone, Copy)]
enum ArgumentValue {
    /// The two `Event` values of the overlapping pair, which the document carries in this
    /// order.
    Pair(&'static str, &'static str),
}

impl ArgumentValue {
    /// The value as the refusal spells it: the pair separated by `, `.
    fn spelled(self) -> String {
        match self {
            Self::Pair(first, second) => format!("{first}, {second}"),
        }
    }
}

/// The separator between an `Event` value's TYPE and its detail, and the detail that stands
/// for every event of a type — the two halves of the `s3:<Type>:*` form the model publishes.
const DETAIL_SEPARATOR: char = ':';
const WILDCARD_SUFFIX: &str = ":*";

/// One `QueueConfiguration` of a document: the id it is stored under, the queue it names,
/// the events it is notified of, and the key-name filter rules it carries as
/// `(Name, Value)`.
#[derive(Debug, Clone)]
struct Configuration {
    id: &'static str,
    arn: &'static str,
    events: Vec<String>,
    filter_rules: Vec<(&'static str, &'static str)>,
}

impl Configuration {
    /// A configuration of the first queue publishing `events` and filtering on nothing.
    fn first(events: &[&str]) -> Self {
        Self {
            id: "one",
            arn: FIRST_QUEUE_ARN,
            events: events.iter().map(|event| (*event).to_string()).collect(),
            filter_rules: Vec::new(),
        }
    }

    /// A configuration of the second queue publishing `events` and filtering on nothing.
    fn second(events: &[&str]) -> Self {
        Self {
            id: "two",
            arn: SECOND_QUEUE_ARN,
            events: events.iter().map(|event| (*event).to_string()).collect(),
            filter_rules: Vec::new(),
        }
    }

    fn filtering(mut self, rules: &[(&'static str, &'static str)]) -> Self {
        self.filter_rules = rules.to_vec();
        self
    }

    /// This configuration as the XML element a client sends.
    fn xml(&self) -> String {
        let mut body = format!(
            "<QueueConfiguration><Id>{}</Id><Queue>{}</Queue>",
            self.id, self.arn
        );
        for event in &self.events {
            body.push_str(&format!("<Event>{event}</Event>"));
        }
        if !self.filter_rules.is_empty() {
            body.push_str("<Filter><S3Key>");
            for (name, value) in &self.filter_rules {
                body.push_str(&format!(
                    "<FilterRule><Name>{name}</Name><Value>{value}</Value></FilterRule>"
                ));
            }
            body.push_str("</S3Key></Filter>");
        }
        body.push_str("</QueueConfiguration>");
        body
    }
}

/// The `NotificationConfiguration` payload carrying `configurations`.
fn payload(configurations: &[Configuration]) -> String {
    let mut body = format!("<NotificationConfiguration xmlns=\"{XMLNS}\">");
    for configuration in configurations {
        body.push_str(&configuration.xml());
    }
    body.push_str("</NotificationConfiguration>");
    body
}

/// `PUT /<bucket>?notification` — `PutBucketNotificationConfiguration`.
async fn put_notification(
    router: &MultiServiceRouter,
    bucket: &str,
    configurations: &[Configuration],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?notification"),
        &[],
        &payload(configurations),
    )
    .await
}

/// `GET /<bucket>?notification` — `GetBucketNotificationConfiguration`.
async fn get_notification(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?notification"), &[], "").await
}

/// The configuration a bucket carries while the rows below are refused: one queue
/// configuration whose event no row names, so a row that stored its own document would be
/// visible in the read-back.
fn seeded() -> Vec<Configuration> {
    vec![Configuration::first(&["s3:ObjectRestore:Delete"])]
}

/// The event the seeded configuration names, which is what a read-back must still report.
const SEEDED_EVENT: &str = "s3:ObjectRestore:Delete";

/// The type part of `event` — everything before the detail separator — or `None` for a
/// value that carries no detail part at all (`s3:ReducedRedundancyLostObject`,
/// `s3:IntelligentTiering`, `s3:LifecycleTransition`), which stands for nothing but itself.
fn event_type(event: &str) -> Option<&str> {
    event
        .rsplit_once(DETAIL_SEPARATOR)
        .map(|(kind, _)| kind)
        .filter(|kind| kind.contains(DETAIL_SEPARATOR))
}

#[tokio::test]
async fn a_wildcard_event_and_an_event_of_its_type_cannot_both_be_configured() {
    // (case, the two configurations, what the refusal must carry — `None` for a document S3
    // applies as given). A wildcard overlap names the `Event` of the LATER configuration
    // that the earlier one is notified of too; a pair naming one event value names the
    // `FilterRule` nothing tells them apart by.
    let rows: Vec<(&str, Vec<Configuration>, Option<Refused>)> = vec![
        (
            // The class the recorded case belongs to: the tagging wildcard beside one
            // tagging event, neither filtering, so every key is matched by both.
            "the tagging wildcard beside the tagging write",
            vec![
                Configuration::first(&["s3:ObjectTagging:*"]),
                Configuration::second(&["s3:ObjectTagging:Put"]),
            ],
            Some(Refused::events("s3:ObjectTagging:*", "s3:ObjectTagging:Put")),
        ),
        (
            "the tagging wildcard beside the tagging removal, in the other order",
            vec![
                Configuration::first(&["s3:ObjectTagging:Delete"]),
                Configuration::second(&["s3:ObjectTagging:*"]),
            ],
            Some(Refused::events("s3:ObjectTagging:Delete", "s3:ObjectTagging:*")),
        ),
        (
            // Another type, and an event the issue's own case does not name: the rule is
            // about the FORM of the value, not about one type.
            "the creation wildcard beside a multipart completion",
            vec![
                Configuration::first(&["s3:ObjectCreated:*"]),
                Configuration::second(&["s3:ObjectCreated:CompleteMultipartUpload"]),
            ],
            Some(Refused::events(
                "s3:ObjectCreated:*",
                "s3:ObjectCreated:CompleteMultipartUpload",
            )),
        ),
        (
            // The filters do not tell these apart: one prefix contains the other, so a key
            // under the longer one is matched by both configurations.
            "the removal wildcard beside a delete marker over nested prefixes",
            vec![
                Configuration::first(&["s3:ObjectRemoved:*"]).filtering(&[("prefix", "v/")]),
                Configuration::second(&["s3:ObjectRemoved:DeleteMarkerCreated"])
                    .filtering(&[("prefix", "v/sub/")]),
            ],
            Some(Refused::events(
                "s3:ObjectRemoved:*",
                "s3:ObjectRemoved:DeleteMarkerCreated",
            )),
        ),
        (
            // The two name the SAME value, so what they are not told apart by is their key
            // filters, and that is the field this refusal names.
            "one type's wildcard twice",
            vec![
                Configuration::first(&["s3:ObjectRestore:*"]),
                Configuration::second(&["s3:ObjectRestore:*"]),
            ],
            Some(Refused::filters()),
        ),
        (
            "one literal event twice over nested prefixes",
            vec![
                Configuration::first(&["s3:ObjectCreated:Put"]).filtering(&[("prefix", "a/")]),
                Configuration::second(&["s3:ObjectCreated:Put"])
                    .filtering(&[("prefix", "a/deep/")]),
            ],
            Some(Refused::filters()),
        ),
        (
            // The wildcard is one entry of a longer `Events` list, and the overlap is with
            // that entry alone.
            "a wildcard among several events beside one event of its type",
            vec![
                Configuration::first(&["s3:ObjectAcl:Put", "s3:ObjectRestore:*"]),
                Configuration::second(&["s3:ObjectRestore:Completed", "s3:ObjectRetention:Put"]),
            ],
            Some(Refused::events(
                "s3:ObjectRestore:*",
                "s3:ObjectRestore:Completed",
            )),
        ),
        (
            // ...and the key filters are what tell two configurations apart, exactly as
            // they do for two configurations naming one literal value.
            "a wildcard and an event of its type over disjoint prefixes",
            vec![
                Configuration::first(&["s3:ObjectTagging:*"]).filtering(&[("prefix", "wild/")]),
                Configuration::second(&["s3:ObjectTagging:Put"])
                    .filtering(&[("prefix", "literal/")]),
            ],
            None,
        ),
        (
            "a wildcard and an event of its type over overlapping prefixes and disjoint \
             suffixes",
            vec![
                Configuration::first(&["s3:ObjectCreated:*"])
                    .filtering(&[("prefix", "incoming/"), ("suffix", ".csv")]),
                Configuration::second(&["s3:ObjectCreated:Put"])
                    .filtering(&[("prefix", "incoming/deep/"), ("suffix", ".json")]),
            ],
            None,
        ),
        (
            // The wildcard spans ONE type: an event of another type is a different event,
            // whatever the filters say.
            "a wildcard and an event of another type on one prefix",
            vec![
                Configuration::first(&["s3:ObjectCreated:*"]).filtering(&[("prefix", "shared/")]),
                Configuration::second(&["s3:ObjectRemoved:Delete"])
                    .filtering(&[("prefix", "shared/")]),
            ],
            None,
        ),
        (
            "two different types' wildcards",
            vec![
                Configuration::first(&["s3:ObjectCreated:*"]),
                Configuration::second(&["s3:ObjectRemoved:*"]),
            ],
            None,
        ),
        (
            // A published value with no detail part stands for nothing but itself, so no
            // wildcard covers it.
            "a detail-less event beside a wildcard",
            vec![
                Configuration::first(&["s3:ReducedRedundancyLostObject"]),
                Configuration::second(&["s3:ObjectCreated:*"]),
            ],
            None,
        ),
    ];

    let router = router();
    for (index, (case, configurations, refused_event)) in rows.into_iter().enumerate() {
        let bucket = format!("wildcard-overlap-{index}");
        create_bucket(&router, &bucket, None, None).await;
        // Seeded first, so a document that must be refused can be seen not to have replaced
        // what the bucket already carried.
        let seeding = put_notification(&router, &bucket, &seeded()).await;
        assert_eq!(
            seeding.status,
            200,
            "case {case:?}: the seeded configuration must be accepted, got {} with {:?}",
            seeding.status,
            String::from_utf8_lossy(&seeding.body)
        );

        let answer = put_notification(&router, &bucket, &configurations).await;
        match refused_event {
            None => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: the document must be applied as given, got {} with {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let read = get_notification(&router, &bucket).await;
                let events = read.elements("Event");
                for configuration in &configurations {
                    for event in &configuration.events {
                        assert!(
                            events.contains(event),
                            "case {case:?}: the stored configuration must report {event:?}, \
                             got {events:?}"
                        );
                    }
                }
            }
            Some(refused) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: the document must be refused with HTTP 400, got {} with \
                     {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    INVALID_ARGUMENT,
                    "case {case:?}: expected the refusal to carry {INVALID_ARGUMENT}, got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.error_message(),
                    refused.message,
                    "case {case:?}: expected the refusal's message to be {:?}, got {:?}",
                    refused.message,
                    answer.error_message()
                );
                // Each class's sentence is the same for every document in it, so where a
                // class names a field that pair is what says which part of this one to
                // change.
                assert_eq!(
                    answer.element("ArgumentName").as_deref(),
                    refused.argument,
                    "case {case:?}: expected the refusal to name the argument {:?}, got {:?}",
                    refused.argument,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.element("ArgumentValue"),
                    refused.value.map(ArgumentValue::spelled),
                    "case {case:?}: expected the refusal to name the value {:?}, got {:?}",
                    refused.value.map(ArgumentValue::spelled),
                    String::from_utf8_lossy(&answer.body)
                );
                // "Amazon S3 will not add the configuration to your bucket": the bucket
                // still carries exactly what it was seeded with.
                let read = get_notification(&router, &bucket).await;
                let events = read.elements("Event");
                assert_eq!(
                    events,
                    vec![SEEDED_EVENT.to_string()],
                    "case {case:?}: the refused document must leave the stored configuration \
                     untouched, got {events:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_published_wildcard_conflicts_with_every_event_of_its_own_type() {
    // The catalog is the model's own `Event` value list
    // (`smithy_query(query="shape",shape="Event")`), so a value added to the model is
    // covered here without this suite naming it.
    let wildcards: Vec<&&str> = EVENT_VALUES
        .iter()
        .filter(|event| event.ends_with(WILDCARD_SUFFIX))
        .collect();
    assert!(
        !wildcards.is_empty(),
        "case \"the catalog's wildcards\": the published Event catalog must carry the \
         s3:<Type>:* form, got {EVENT_VALUES:?}"
    );

    let router = router();
    let mut index = 0usize;
    for wildcard in wildcards {
        let kind = event_type(wildcard).unwrap_or_else(|| {
            panic!("case {wildcard:?}: a wildcard value carries the type it spans")
        });
        // Every published event of that type, which is what the wildcard stands for.
        let covered: Vec<&&str> = EVENT_VALUES
            .iter()
            .filter(|event| **event != *wildcard && event_type(event) == Some(kind))
            .collect();
        assert!(
            !covered.is_empty(),
            "case {wildcard:?}: the catalog must publish at least one event of the type a \
             wildcard spans, got {EVENT_VALUES:?}"
        );
        for event in covered {
            let case = format!("{wildcard} beside {event}");
            let bucket = format!("wildcard-catalog-{index}");
            index += 1;
            create_bucket(&router, &bucket, None, None).await;
            let answer = put_notification(
                &router,
                &bucket,
                &[
                    Configuration::first(&[wildcard]),
                    Configuration::second(&[event]),
                ],
            )
            .await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: the document must be refused with HTTP 400, got {} with {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_message(),
                OVERLAPPING_EVENTS,
                "case {case:?}: expected the refusal's message to be {OVERLAPPING_EVENTS:?}, \
                 got {:?}",
                answer.error_message()
            );
            assert_eq!(
                answer.element("ArgumentName").as_deref(),
                Some(EVENT_ARGUMENT),
                "case {case:?}: expected the refusal to name the {EVENT_ARGUMENT:?} argument, \
                 got {:?}",
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.element("ArgumentValue"),
                Some(format!("{wildcard}, {event}")),
                "case {case:?}: expected the refusal to name the pair {wildcard}, {event}, \
                 got {:?}",
                String::from_utf8_lossy(&answer.body)
            );

            // ...and the same pair over DISJOINT prefixes is applied, so the refusal above
            // is about the overlap and not about the pair of values.
            let distinguished = put_notification(
                &router,
                &bucket,
                &[
                    Configuration::first(&[wildcard]).filtering(&[("prefix", "wild/")]),
                    Configuration::second(&[event]).filtering(&[("prefix", "literal/")]),
                ],
            )
            .await;
            assert_eq!(
                distinguished.status,
                200,
                "case {case:?}: over disjoint prefixes the same pair must be applied, got {} \
                 with {:?}",
                distinguished.status,
                String::from_utf8_lossy(&distinguished.body)
            );
        }
    }
}
