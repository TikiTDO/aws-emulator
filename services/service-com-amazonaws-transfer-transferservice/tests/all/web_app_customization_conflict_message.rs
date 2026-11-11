// SPDX-License-Identifier: CC0-1.0

//! What a web app customization write that LOSES the race is told, end to end over HTTP.
//!
//! RULE under test: `UpdateWebAppCustomization` and `DeleteWebAppCustomization` both write the
//! single customization object a web app holds, and the service applies one such write at a
//! time, so a write that overlaps another is refused with the modelled `ConflictException`
//! (HTTP 409 for each,
//! `smithy_query(query="operation",operation="UpdateWebAppCustomization,DeleteWebAppCustomization")`).
//! That refusal carries the service's own FIXED sentence for the write it could not carry
//! out — "Unable to update WebApp customization" for an update, "Unable to delete WebApp
//! customization" for a delete — so the sentence is decided by the operation that lost and by
//! nothing else: no web app id, no title, no logo, nothing from the request or from the store
//! reaches it, and two web apps' losers read the same.
//!
//! (The guard itself — exactly one winner, the surviving customization being the winner's,
//! non-overlapping writes all applied — is [`crate::web_app_customization_write_guard`]'s
//! rule; this suite is about the WORDING every refusal it produces carries.)
//!
//! The rows differ along what the rule generalises over: which operation loses, which pair of
//! operations race (two updates, two deletes, an update against a delete in both orders, and
//! the two kinds mixed), how many writers race at once, what the losing request carried
//! (a title, a logo, nothing at all), whether the web app held a customization already, and
//! which of two web apps the race is on.
//!
//! Overlap is produced the way the guard's own suite produces it, with the helpers it owns.

use serde_json::{json, Value};

use crate::harness::{member_of, Answer, Emulator};
use crate::web_app_customization_write_guard::{create_web_app, futures_of};

/// The sentence a losing `UpdateWebAppCustomization` is refused with, spelled out as a
/// literal rather than read off the crate's producer, so the WORDING is what these rows are
/// held to.
const UPDATE_CONFLICT: &str = "Unable to update WebApp customization";

/// The sentence a losing `DeleteWebAppCustomization` is refused with, for the same reason.
const DELETE_CONFLICT: &str = "Unable to delete WebApp customization";

/// The sentence the service refuses a lost write of `op` with.
fn conflict_sentence(op: &str) -> &'static str {
    match op {
        "UpdateWebAppCustomization" => UPDATE_CONFLICT,
        "DeleteWebAppCustomization" => DELETE_CONFLICT,
        other => panic!("{other} does not write a web app's customization"),
    }
}

/// Assert `answer` is the modelled conflict for a lost write of `op`, carrying that
/// operation's own sentence and nothing the request submitted.
fn assert_conflict_wording(case: &str, op: &str, answer: &Answer, submitted: &[&str]) {
    assert_eq!(
        answer.status, 409,
        "case {case:?} ({op} lost): expected HTTP 409, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ConflictException",
        "case {case:?} ({op} lost): expected ConflictException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expected = conflict_sentence(op);
    assert_eq!(
        message, expected,
        "case {case:?} ({op} lost): expected {expected:?}, got {message:?}"
    );
    for value in submitted {
        assert!(
            !message.contains(value),
            "case {case:?} ({op} lost): the refusal must carry nothing the request \
             submitted, got {message:?} for {value:?}"
        );
    }
}

/// One racing writer: the operation and the request members it carries.
type Call = (&'static str, Value);

/// RULE: of writes of one web app's customization that overlap, every loser is refused with
/// the sentence for the operation IT could not carry out.
#[tokio::test]
async fn every_losing_customization_write_carries_its_own_operation_sentence() {
    // (case, whether a customization is seeded first, the writers that race)
    let rows: Vec<(&str, bool, Vec<Call>)> = vec![
        (
            "two updates with different titles",
            true,
            vec![
                ("UpdateWebAppCustomization", json!({"Title": "first"})),
                ("UpdateWebAppCustomization", json!({"Title": "second"})),
            ],
        ),
        (
            "two deletes",
            true,
            vec![
                ("DeleteWebAppCustomization", json!({})),
                ("DeleteWebAppCustomization", json!({})),
            ],
        ),
        (
            "an update against a delete",
            true,
            vec![
                ("UpdateWebAppCustomization", json!({"Title": "assigned"})),
                ("DeleteWebAppCustomization", json!({})),
            ],
        ),
        (
            "a delete against an update",
            true,
            vec![
                ("DeleteWebAppCustomization", json!({})),
                ("UpdateWebAppCustomization", json!({"Title": "assigned"})),
            ],
        ),
        (
            "a logo update against a delete, on a web app with no customization yet",
            false,
            vec![
                (
                    "UpdateWebAppCustomization",
                    json!({"LogoFile": aws_smithy_types::base64::encode(b"logo")}),
                ),
                ("DeleteWebAppCustomization", json!({})),
            ],
        ),
        (
            "five overlapping updates",
            true,
            vec![
                ("UpdateWebAppCustomization", json!({"Title": "one"})),
                ("UpdateWebAppCustomization", json!({"Title": "two"})),
                ("UpdateWebAppCustomization", json!({"Title": "three"})),
                ("UpdateWebAppCustomization", json!({"Title": "four"})),
                ("UpdateWebAppCustomization", json!({"Title": "five"})),
            ],
        ),
        (
            "three overlapping deletes",
            true,
            vec![
                ("DeleteWebAppCustomization", json!({})),
                ("DeleteWebAppCustomization", json!({})),
                ("DeleteWebAppCustomization", json!({})),
            ],
        ),
        (
            "two updates and two deletes at once",
            true,
            vec![
                ("UpdateWebAppCustomization", json!({"Title": "mixed-one"})),
                ("DeleteWebAppCustomization", json!({})),
                ("UpdateWebAppCustomization", json!({"Title": "mixed-two"})),
                ("DeleteWebAppCustomization", json!({})),
            ],
        ),
    ];

    for (case, seed, writers) in rows {
        let emulator = Emulator::new();
        // A SECOND web app, created first, so the id under test is not the only one the
        // store holds: a refusal that named the web app it was about would differ between
        // the two, and these rows would catch it.
        let other = create_web_app(&emulator).await;
        let web_app_id = create_web_app(&emulator).await;
        assert_ne!(
            other, web_app_id,
            "case {case:?}: the two web apps must have different ids"
        );
        if seed {
            let seeded = emulator
                .call(
                    "UpdateWebAppCustomization",
                    json!({"WebAppId": &web_app_id, "Title": "seeded"}),
                )
                .await;
            assert_eq!(
                seeded.status, 200,
                "case {case:?}: expected the seeding update to be served, got {} with body {}",
                seeded.status, seeded.body
            );
        }

        let calls = writers.iter().map(|(op, members)| {
            let mut request = members.clone();
            request["WebAppId"] = json!(&web_app_id);
            emulator.call(op, request)
        });
        let answers = futures_of(calls).await;

        let served = answers
            .iter()
            .filter(|answer| answer.status == 200)
            .count();
        assert_eq!(
            served,
            1,
            "case {case:?}: expected exactly one winning write, got the statuses {:?}",
            answers.iter().map(|answer| answer.status).collect::<Vec<_>>()
        );

        // Every value the losing requests submitted, which no refusal may carry.
        let mut submitted: Vec<&str> = vec![web_app_id.as_str()];
        for (_, members) in &writers {
            if let Some(title) = member_of(members, "Title").as_str() {
                submitted.push(title);
            }
        }
        let losers = answers
            .iter()
            .zip(&writers)
            .filter(|(answer, _)| answer.status != 200);
        let mut refused = 0;
        for (answer, (op, _)) in losers {
            assert_conflict_wording(case, op, answer, &submitted);
            refused += 1;
        }
        assert_eq!(
            refused,
            writers.len() - 1,
            "case {case:?}: every writer but the winner must be refused"
        );
    }
}
