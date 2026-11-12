// SPDX-License-Identifier: CC0-1.0

//! How this service STATES an event destination a configuration set does not hold,
//! driven end to end over the awsQuery wire: form-urlencoded body in, XML out, one
//! shared engine behind every call.
//!
//! One rule, asserted over a SET of inputs and over EVERY operation that resolves an
//! event destination on a configuration set the account DOES hold: the refusal is HTTP
//! 400 `EventDestinationDoesNotExist` whose message is the fixed text
//! `No EventDestination found for ` followed by the request's `ConfigurationSetName`
//! stated bare — no destination name, no brackets and no trailing period — so two calls
//! differing only in the destination they asked for are worded identically.
//!
//! The rows differ along what the rule generalises over: the FORM of the configuration
//! set's name and of the destination name asked for (both over the character set
//! `ConfigurationSetName`/`EventDestination$Name` document — ASCII letters, digits,
//! underscores and dashes, at most 64 characters), whether that destination never
//! existed or was deleted, whether its name differs from a held one only in letter case,
//! and WHICH operation (the update or the delete) read it. The positive control on the
//! same store — the destination the set DOES hold is updated and deleted — is what keeps
//! this a statement about the refusal class rather than about every call.

use crate::harness::{
    call, described_event_destinations, elements, emulator, error_envelope,
    create_event_destination, field, update_event_destination, xml_text, Destination,
};
use smithy_server_runtime::MultiServiceRouter;

/// The code SES answers for an event destination a configuration set does not hold.
const DOES_NOT_EXIST: &str = "EventDestinationDoesNotExist";

/// The sentence the missing-destination refusal states, with the configuration set the
/// request named as its one variable part.
fn missing_message(configuration_set: &str) -> String {
    format!("No EventDestination found for {configuration_set}")
}

/// `DeleteConfigurationSetEventDestination`.
async fn delete_event_destination(
    router: &MultiServiceRouter,
    configuration_set: &str,
    name: &str,
) -> (u16, String) {
    call(
        router,
        "DeleteConfigurationSetEventDestination",
        &[
            field("ConfigurationSetName", configuration_set),
            field("EventDestinationName", name),
        ],
    )
    .await
}

/// A configuration set the account holds, so each row starts from a set that resolves
/// and the only thing missing is the destination.
async fn given_configuration_set(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateConfigurationSet",
        &[field("ConfigurationSet.Name", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding set {name:?}: expected 200, got {status} with {body}"
    );
}

/// The names `DescribeConfigurationSet` reports for `set`'s event destinations, in the
/// order it reports them.
async fn destination_names(router: &MultiServiceRouter, set: &str) -> Vec<String> {
    let reported = described_event_destinations(router, set).await;
    elements(&reported, "Name")
        .iter()
        .map(|name| xml_text(name))
        .collect()
}

/// A `CloudWatchDestination` categorizing on `dimension`, which is the destination body
/// both writers carry in these rows.
fn cloud_watch(dimension: &str) -> Destination {
    Destination::CloudWatch(vec![(
        dimension.to_string(),
        "messageTag".to_string(),
        "none".to_string(),
    )])
}

/// One row of the table the rule is asserted over.
struct Row {
    /// What this row is, for the assertion messages.
    case: &'static str,
    /// The configuration set the request names — one the account holds.
    set: String,
    /// The destination the set holds before the refused call, if any: a row that
    /// DELETED its destination first starts from one.
    held: Option<String>,
    /// The destination name the refused call asks for, which the set holds none of.
    asked_for: String,
}

/// The rows, spanning the forms both names arrive in and how the destination came to be
/// absent.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a destination that never existed, on an empty set",
            set: "wording-empty-set".to_string(),
            held: None,
            asked_for: "absent".to_string(),
        },
        Row {
            case: "a destination deleted first, so the set once held it",
            set: "wording-deleted".to_string(),
            held: Some("gone-dest".to_string()),
            asked_for: "gone-dest".to_string(),
        },
        Row {
            case: "a name differing from a held one only in letter case",
            set: "wording-letter-case".to_string(),
            held: Some("held".to_string()),
            asked_for: "Held".to_string(),
        },
        Row {
            case: "an underscored and dashed destination name",
            set: "Wording_Set-2".to_string(),
            held: Some("held".to_string()),
            asked_for: "Absent_Dest-2".to_string(),
        },
        Row {
            case: "the shortest name the form admits, on a numeric set name",
            set: "0".to_string(),
            held: None,
            asked_for: "z".to_string(),
        },
        Row {
            // Both names at the 64-character ceiling the model documents for
            // `ConfigurationSetName` and `EventDestination$Name`.
            case: "both names at the 64-character ceiling",
            set: "s".repeat(64),
            held: None,
            asked_for: "d".repeat(64),
        },
    ]
}

/// Seed `row`'s set, and the destination it holds before the refused call.
async fn given_row(router: &MultiServiceRouter, row: &Row) {
    given_configuration_set(router, &row.set).await;
    let Some(held) = row.held.as_deref() else {
        return;
    };
    let (status, body) = create_event_destination(
        router,
        &row.set,
        held,
        true,
        &["send"],
        &Destination::Sns(crate::harness::topic_arn("wording-topic")),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: seeding destination {held:?} expected 200, got {status} with {body}",
        row.case
    );
    // A row whose destination was DELETED starts from one that was there and is not:
    // the delete itself is the positive control for that row.
    if held == row.asked_for {
        let (status, body) = delete_event_destination(router, &row.set, held).await;
        assert_eq!(
            status, 200,
            "case {:?}: deleting {held:?} expected 200, got {status} with {body}",
            row.case
        );
    }
}

/// The destinations `row`'s set holds after the refused call, which a refusal leaves
/// exactly as the seeding left them.
fn expected_remaining(row: &Row) -> Vec<String> {
    match row.held.as_deref() {
        Some(held) if held != row.asked_for => vec![held.to_string()],
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn the_update_states_the_missing_destination_by_naming_only_the_configuration_set() {
    let router = emulator();
    for row in rows() {
        given_row(&router, &row).await;
        let (status, body) = update_event_destination(
            &router,
            &row.set,
            &row.asked_for,
            false,
            &["bounce"],
            &cloud_watch("campaign"),
        )
        .await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(DOES_NOT_EXIST),
                Some(missing_message(&row.set).as_str())
            ),
            "case {:?}: expected the missing-destination refusal naming only {:?}, got {status} \
             with {body}",
            row.case,
            row.set
        );
        let remaining = destination_names(&router, &row.set).await;
        assert_eq!(
            remaining,
            expected_remaining(&row),
            "case {:?}: a refused update must record nothing, got {remaining:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_delete_states_the_missing_destination_in_the_same_sentence() {
    let router = emulator();
    for row in rows() {
        given_row(&router, &row).await;
        let (status, body) = delete_event_destination(&router, &row.set, &row.asked_for).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref(), message.as_deref()),
            (
                400,
                Some(DOES_NOT_EXIST),
                Some(missing_message(&row.set).as_str())
            ),
            "case {:?}: expected the missing-destination refusal naming only {:?}, got {status} \
             with {body}",
            row.case,
            row.set
        );
        let remaining = destination_names(&router, &row.set).await;
        assert_eq!(
            remaining,
            expected_remaining(&row),
            "case {:?}: a refused delete must remove nothing, got {remaining:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn two_calls_differing_only_in_the_destination_asked_for_are_worded_identically() {
    let router = emulator();
    let set = "wording-one-sentence";
    given_configuration_set(&router, set).await;

    // Several destination names, none of them held: the sentence does not vary with the
    // destination asked for, because it names none.
    let mut sentences = Vec::new();
    for asked_for in ["one", "two-dest", "THREE_dest", &"x".repeat(64)] {
        let (status, body) = delete_event_destination(&router, set, asked_for).await;
        let (code, message) = error_envelope(&body);
        assert_eq!(
            (status, code.as_deref()),
            (400, Some(DOES_NOT_EXIST)),
            "case {asked_for:?}: expected the missing-destination refusal, got {status} with \
             {body}"
        );
        sentences.push(message.unwrap_or_default());
    }
    let expected = vec![missing_message(set); sentences.len()];
    assert_eq!(
        sentences, expected,
        "every missing-destination refusal on one set must read alike, got {sentences:?}"
    );
}

#[tokio::test]
async fn the_destination_the_set_does_hold_is_updated_and_deleted() {
    let router = emulator();
    let set = "wording-control";
    given_configuration_set(&router, set).await;
    let (status, body) = create_event_destination(
        &router,
        set,
        "held",
        true,
        &["send"],
        &Destination::Sns(crate::harness::topic_arn("wording-topic")),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the held destination: creating it expected 200, got {status} with {body}"
    );

    let (status, body) = update_event_destination(
        &router,
        set,
        "held",
        false,
        &["bounce"],
        &cloud_watch("campaign"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the held destination: updating it expected 200, got {status} with {body}"
    );
    let names = destination_names(&router, set).await;
    assert_eq!(
        names,
        vec!["held".to_string()],
        "case the held destination: expected the set to keep it, got {names:?}"
    );

    let (status, body) = delete_event_destination(&router, set, "held").await;
    assert_eq!(
        status, 200,
        "case the held destination: deleting it expected 200, got {status} with {body}"
    );
    let names = destination_names(&router, set).await;
    assert!(
        names.is_empty(),
        "case the held destination: expected the set to hold none, got {names:?}"
    );
}
