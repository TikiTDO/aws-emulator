// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TRANSITION rules of a lifecycle configuration: which chains
//! `PutBucketLifecycleConfiguration` stores, which it refuses, and the sentence each refusal
//! carries.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] owns), so what is pinned is the wire behaviour — status, error
//! code and message, and what a later read reports — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * A transition's day count is bounded BELOW by nothing: "the value can be `0` or any
//!   positive integer. Be aware that some storage classes have a minimum storage duration
//!   and that you're charged for transitioning objects before their minimum storage
//!   duration" (`Transition$Days`) — the duration is a charge on the object, not a bound on
//!   the configuration, so a transition to any target class on any day from zero up is
//!   stored and reads back.
//! * What the minimum storage duration DOES bound is the SPACING between one transition and
//!   the next of the same rule: a chain may not move an object out of a class before that
//!   class's duration has run (Amazon S3 User Guide, *Transitioning objects*), and it may
//!   not move it to a WARMER class than one it has already been moved to.
//! * The TARGET of either transition action is a `TransitionStorageClass`
//!   (`smithy_query(query="shape",shape="TransitionStorageClass")` publishes `[GLACIER,
//!   STANDARD_IA, ONEZONE_IA, INTELLIGENT_TIERING, DEEP_ARCHIVE, GLACIER_IR]`), so
//!   `STANDARD` — the class an object is already in — is not one, and naming it is refused
//!   for the VALUE rather than as a document that does not validate.
//! * Each refusal carries S3's own sentence, whose variable parts come from the document:
//!   the member under constraint, the action carrying it, the two storage classes, the
//!   number of days the duration requires and the SCOPE of the rule, which S3 describes as
//!   `(prefix=<prefix>)` with one `, tag=<key>=<value>` per tag the filter names.

use service_com_amazonaws_s3_amazons3::ops::types::TRANSITION_STORAGE_CLASS_VALUES;
use service_com_amazonaws_s3_amazons3::state::{
    minimum_storage_duration_days, NONCURRENT_VERSION_TRANSITION_ACTION, TRANSITION_ACTION,
    TRANSITION_STORAGE_CLASS_CHAIN,
};

use super::bucket_lifecycle::router;
use super::bucket_lifecycle_configuration::{configuration, put_lifecycle, rule};
use super::object_and_tagging_roundtrip::{bucket, text};

/// One row of the ACCEPTED table: the case, the rule body the put carries, and a fragment a
/// later read of the stored configuration must report back.
struct AcceptedRow {
    case: &'static str,
    body: String,
    reads_back: String,
}

/// The chains S3 STORES.
///
/// The rows differ along what the rule generalises over: the target class (every value of
/// `TransitionStorageClass`), the day count (zero, one, and a count inside the class's own
/// minimum storage duration), the ACTION carrying it (current-version and noncurrent-version
/// transitions), the number of transitions in the chain, and the form of the filter that
/// scopes it.
fn accepted_rows() -> Vec<AcceptedRow> {
    let mut rows: Vec<AcceptedRow> = Vec::new();

    // Every published target class, one day after the object is created — the case the
    // minimum storage duration does NOT bound.
    for storage_class in TRANSITION_STORAGE_CLASS_VALUES.iter().copied() {
        rows.push(AcceptedRow {
            case: storage_class,
            body: format!(
                "<ID>day-one-{storage_class}</ID><Filter><Prefix>tiered/</Prefix></Filter>\
                 <Status>Enabled</Status><Transition><Days>1</Days>\
                 <StorageClass>{storage_class}</StorageClass></Transition>"
            ),
            reads_back: format!("<StorageClass>{storage_class}</StorageClass>"),
        });
    }

    rows.push(AcceptedRow {
        case: "a transition on the day the object is created",
        body: "<ID>day-zero</ID><Filter><Prefix>tiered/</Prefix></Filter><Status>Enabled</Status>\
               <Transition><Days>0</Days><StorageClass>STANDARD_IA</StorageClass></Transition>"
            .to_string(),
        reads_back: "<Days>0</Days>".to_string(),
    });
    rows.push(AcceptedRow {
        case: "a chain spaced by exactly the warmer class's minimum storage duration",
        body: "<ID>spaced</ID><Filter><Prefix>tiered/</Prefix></Filter><Status>Enabled</Status>\
               <Transition><Days>1</Days><StorageClass>STANDARD_IA</StorageClass></Transition>\
               <Transition><Days>31</Days><StorageClass>GLACIER</StorageClass></Transition>"
            .to_string(),
        reads_back: "<Days>31</Days>".to_string(),
    });
    rows.push(AcceptedRow {
        case: "a tag-filtered transition one day after creation",
        body: "<ID>tagged</ID><Filter><Tag><Key>team</Key><Value>data</Value></Tag></Filter>\
               <Status>Enabled</Status><Transition><Days>1</Days>\
               <StorageClass>ONEZONE_IA</StorageClass></Transition>"
            .to_string(),
        reads_back: "<Key>team</Key>".to_string(),
    });
    rows.push(AcceptedRow {
        case: "a noncurrent-version transition one day after the version became noncurrent",
        body: "<ID>noncurrent-day-one</ID><Filter><Prefix>tiered/</Prefix></Filter>\
               <Status>Enabled</Status><NoncurrentVersionTransition>\
               <NoncurrentDays>1</NoncurrentDays><StorageClass>STANDARD_IA</StorageClass>\
               </NoncurrentVersionTransition>"
            .to_string(),
        reads_back: "<NoncurrentDays>1</NoncurrentDays>".to_string(),
    });
    rows
}

#[tokio::test]
async fn a_transition_is_stored_whatever_day_it_falls_on() {
    let router = router();
    bucket(&router, "transition-chains").await;

    for row in accepted_rows() {
        let case = row.case;
        let document = configuration(&[rule(&row.body)]);
        let put = put_lifecycle(&router, "transition-chains", &[], &document).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: the configuration must be stored, got {} with body {:?}",
            put.status,
            text(&put)
        );

        // Round-tripped: what the bucket carries afterwards is what the put named.
        let read = super::bucket_lifecycle::send(
            &router,
            "GET",
            "/transition-chains?lifecycle",
            &[],
            "",
        )
        .await;
        let stored = text(&read);
        assert!(
            stored.contains(&row.reads_back),
            "case {case:?}: the stored configuration must report {:?}, got {stored:?}",
            row.reads_back
        );
    }
}

/// One row of the REFUSED table: the case, the rule body, and the exact code and sentence.
struct RefusedRow {
    case: &'static str,
    body: String,
    code: &'static str,
    message: String,
}

/// The sentence S3 refuses a transition chain that moves an object on before its class's
/// minimum storage duration has run with — built here the way S3 builds it, from the
/// document.
///
/// Its second clause names the EARLIER transition by the rule's filter alone: no repeated
/// day member, and no "for filter" clause after the storage class.
fn too_soon(action: (&str, &str), filter: &str, earlier: &str, later: &str) -> String {
    let (action, days) = action;
    let minimum = minimum_storage_duration_days(earlier).unwrap_or_default();
    format!(
        "'{days}' in the '{action}' action for StorageClass '{later}' for filter '{filter}' \
         must be {minimum} days more than 'filter '{filter}'' in the '{action}' action for \
         StorageClass '{earlier}'"
    )
}

/// The sentence S3 refuses a chain running from a colder class to a warmer one with.
fn backwards(action: (&str, &str), filter: &str, colder: &str, warmer: &str) -> String {
    let (action, days) = action;
    format!(
        "'{days}' in the '{action}' action for StorageClass '{colder}' for filter '{filter}' \
         must be greater than '{days}' in the '{action}' action for StorageClass '{warmer}' \
         for filter '{filter}'"
    )
}

/// The chains S3 REFUSES.
///
/// The rows differ along the rule that is broken (the spacing a minimum storage duration
/// requires, the direction of the chain, and a target class that is not a transition target
/// at all), the ACTION the broken rule belongs to, and the form of the filter the sentence
/// has to describe.
fn refused_rows() -> Vec<RefusedRow> {
    vec![
        RefusedRow {
            case: "Glacier ten days after Standard-IA, inside its thirty-day duration",
            body: "<ID>gap</ID><Filter><Prefix>tiered/</Prefix></Filter><Status>Enabled</Status>\
                   <Transition><Days>1</Days><StorageClass>STANDARD_IA</StorageClass>\
                   </Transition><Transition><Days>10</Days><StorageClass>GLACIER</StorageClass>\
                   </Transition>"
                .to_string(),
            code: "InvalidArgument",
            message: too_soon(
                TRANSITION_ACTION,
                "(prefix=tiered/)",
                "STANDARD_IA",
                "GLACIER",
            ),
        },
        RefusedRow {
            case: "Deep Archive thirty days after Glacier, inside its ninety-day duration",
            body: "<ID>gap-deep</ID><Filter><Prefix>cold/</Prefix></Filter>\
                   <Status>Enabled</Status><Transition><Days>10</Days>\
                   <StorageClass>GLACIER</StorageClass></Transition><Transition><Days>40</Days>\
                   <StorageClass>DEEP_ARCHIVE</StorageClass></Transition>"
                .to_string(),
            code: "InvalidArgument",
            message: too_soon(
                TRANSITION_ACTION,
                "(prefix=cold/)",
                "GLACIER",
                "DEEP_ARCHIVE",
            ),
        },
        RefusedRow {
            case: "a tag-filtered chain inside the warmer class's duration",
            body: "<ID>gap-tagged</ID><Filter><And><Prefix>tiered/</Prefix>\
                   <Tag><Key>team</Key><Value>data</Value></Tag></And></Filter>\
                   <Status>Enabled</Status><Transition><Days>1</Days>\
                   <StorageClass>ONEZONE_IA</StorageClass></Transition>\
                   <Transition><Days>10</Days><StorageClass>GLACIER_IR</StorageClass>\
                   </Transition>"
                .to_string(),
            code: "InvalidArgument",
            message: too_soon(
                TRANSITION_ACTION,
                "(prefix=tiered/, tag=team=data)",
                "ONEZONE_IA",
                "GLACIER_IR",
            ),
        },
        RefusedRow {
            case: "a noncurrent chain inside the warmer class's duration",
            body: "<ID>gap-noncurrent</ID><Filter><Prefix>tiered/</Prefix></Filter>\
                   <Status>Enabled</Status><NoncurrentVersionTransition>\
                   <NoncurrentDays>1</NoncurrentDays><StorageClass>STANDARD_IA</StorageClass>\
                   </NoncurrentVersionTransition><NoncurrentVersionTransition>\
                   <NoncurrentDays>10</NoncurrentDays><StorageClass>GLACIER</StorageClass>\
                   </NoncurrentVersionTransition>"
                .to_string(),
            code: "InvalidArgument",
            message: too_soon(
                NONCURRENT_VERSION_TRANSITION_ACTION,
                "(prefix=tiered/)",
                "STANDARD_IA",
                "GLACIER",
            ),
        },
        RefusedRow {
            case: "Glacier before Standard-IA, a chain running to a warmer class",
            body: "<ID>backwards</ID><Filter><Prefix>tiered/</Prefix></Filter>\
                   <Status>Enabled</Status><Transition><Days>30</Days>\
                   <StorageClass>GLACIER</StorageClass></Transition><Transition><Days>60</Days>\
                   <StorageClass>STANDARD_IA</StorageClass></Transition>"
                .to_string(),
            code: "InvalidArgument",
            message: backwards(
                TRANSITION_ACTION,
                "(prefix=tiered/)",
                "GLACIER",
                "STANDARD_IA",
            ),
        },
        RefusedRow {
            case: "a noncurrent chain running to a warmer class",
            body: "<ID>backwards-noncurrent</ID><Filter><Prefix>tiered/</Prefix></Filter>\
                   <Status>Enabled</Status><NoncurrentVersionTransition>\
                   <NoncurrentDays>30</NoncurrentDays><StorageClass>DEEP_ARCHIVE</StorageClass>\
                   </NoncurrentVersionTransition><NoncurrentVersionTransition>\
                   <NoncurrentDays>300</NoncurrentDays><StorageClass>GLACIER</StorageClass>\
                   </NoncurrentVersionTransition>"
                .to_string(),
            code: "InvalidArgument",
            message: backwards(
                NONCURRENT_VERSION_TRANSITION_ACTION,
                "(prefix=tiered/)",
                "DEEP_ARCHIVE",
                "GLACIER",
            ),
        },
        RefusedRow {
            case: "a transition whose target is the class the object is already in",
            body: "<ID>standard-target</ID><Filter><Prefix>tiered/</Prefix></Filter>\
                   <Status>Enabled</Status><Transition><Days>30</Days>\
                   <StorageClass>STANDARD</StorageClass></Transition>"
                .to_string(),
            code: "InvalidArgument",
            message: format!(
                "Invalid target Storage class for {} action",
                TRANSITION_ACTION.0
            ),
        },
        RefusedRow {
            case: "a noncurrent transition whose target is STANDARD",
            body: "<ID>standard-target-noncurrent</ID><Filter><Prefix>tiered/</Prefix></Filter>\
                   <Status>Enabled</Status><NoncurrentVersionTransition>\
                   <NoncurrentDays>30</NoncurrentDays><StorageClass>STANDARD</StorageClass>\
                   </NoncurrentVersionTransition>"
                .to_string(),
            code: "InvalidArgument",
            message: format!(
                "Invalid target Storage class for {} action",
                NONCURRENT_VERSION_TRANSITION_ACTION.0
            ),
        },
    ]
}

#[tokio::test]
async fn a_transition_chain_outside_the_published_rules_is_refused_in_aws_words() {
    let router = router();
    bucket(&router, "transition-refusals").await;

    for row in refused_rows() {
        let case = row.case;
        let document = configuration(&[rule(&row.body)]);
        let refused = put_lifecycle(&router, "transition-refusals", &[], &document).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            row.code,
            "case {case:?}: expected the code {:?}, got {:?}",
            row.code,
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            row.message,
            "case {case:?}: expected the message {:?}, got {:?}",
            row.message,
            refused.error_message()
        );
    }

    // Nothing was stored: every chain above was refused before the bucket was touched.
    let read =
        super::bucket_lifecycle::send(&router, "GET", "/transition-refusals?lifecycle", &[], "")
            .await;
    assert_eq!(
        read.status,
        404,
        "the refused chains must leave the bucket with no lifecycle configuration, got {} \
         with body {:?}",
        read.status,
        text(&read)
    );
}

#[tokio::test]
async fn every_published_transition_class_carries_its_own_minimum_duration() {
    // The catalog the spacing rule is judged by, pinned against the model's own enum: every
    // class a transition may name has a place in the chain and a duration, and the chain is
    // ordered warmest first.
    for storage_class in TRANSITION_STORAGE_CLASS_VALUES.iter().copied() {
        assert!(
            TRANSITION_STORAGE_CLASS_CHAIN
                .iter()
                .any(|(named, _)| *named == storage_class),
            "case {storage_class:?}: every published transition class must have a place in \
             the chain, got {TRANSITION_STORAGE_CLASS_CHAIN:?}"
        );
    }
    for (storage_class, published) in TRANSITION_STORAGE_CLASS_CHAIN {
        assert_eq!(
            minimum_storage_duration_days(storage_class),
            Some(published),
            "case {storage_class:?}: the duration the chain publishes must be the one the \
             validator reads, got {:?}",
            minimum_storage_duration_days(storage_class)
        );
    }
    assert_eq!(
        minimum_storage_duration_days("STANDARD"),
        None,
        "case \"STANDARD\": a class no transition may target has no duration in the chain, \
         got {:?}",
        minimum_storage_duration_days("STANDARD")
    );
}
