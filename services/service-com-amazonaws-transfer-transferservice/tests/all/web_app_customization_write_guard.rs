// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the guard that admits ONE writer of a web app's customization.
//!
//! RULE under test: `UpdateWebAppCustomization` and `DeleteWebAppCustomization` both
//! write the single customization object a web app holds, and both model
//! `ConflictException` (HTTP 409). The service applies one such write per web app at a
//! time, so of two writes of ONE web app's customization that are IN FLIGHT AT THE SAME
//! TIME exactly one commits and the other is refused with that conflict — the surviving
//! customization is one submitter's values and never a mixture. Writes that do not
//! overlap are each applied, however many of them there are and however fast they
//! follow one another, and writes of DIFFERENT web apps never contend.
//!
//! The rows differ along what the rule generalises over: which pair of operations race
//! (two updates, two deletes, an update against a delete, in both orders), how many
//! writers race, whether they address the same web app or different ones, and — for the
//! non-overlapping half — how many writes run in a row and in which order.
//!
//! Two calls are made to overlap by driving their futures TOGETHER (`tokio::join!`), so
//! the second reaches the service while the first is still being applied; that is what a
//! client doing the same from two threads produces. Every request goes over the real wire
//! path (`POST /` with `X-Amz-Target: TransferService.<Op>`) through the shared
//! [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{member_of, Answer, Emulator, ROLE};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s
/// modelled pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// Create one web app and hand back its `WebAppId`.
///
/// `pub(crate)` so a sibling suite about the same guard creates its web app the same way
/// rather than growing a copy of this.
pub(crate) async fn create_web_app(emulator: &Emulator) -> String {
    let answer = emulator
        .call(
            "CreateWebApp",
            json!({"IdentityProviderDetails": {
                "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE},
            }}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "CreateWebApp: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WebAppId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateWebApp returned no WebAppId: {}", answer.body))
        .to_string()
}

/// The `Title` the web app's customization currently holds, or `None` when it holds no
/// customization at all.
async fn stored_title(emulator: &Emulator, web_app_id: &str) -> Option<String> {
    let answer = emulator
        .call("DescribeWebAppCustomization", json!({"WebAppId": web_app_id}))
        .await;
    if answer.status != 200 {
        return None;
    }
    member_of(answer.get("WebAppCustomization"), "Title")
        .as_str()
        .map(str::to_string)
}

/// Assert `answer` is the modelled conflict a racing writer is refused with.
fn assert_conflict(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409 for the racing writer, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ConflictException",
        "case {case:?}: expected ConflictException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty(),
        "case {case:?}: expected a non-empty conflict message, got {message:?}"
    );
}

/// One racing pair: the case, and each writer's operation and request members.
type Call = (&'static str, Value);

/// RULE: of two writes of one web app's customization that overlap, exactly one commits
/// and the other is refused with `ConflictException` — for every pair of the two
/// operations that write the object.
#[tokio::test]
async fn two_overlapping_customization_writes_have_exactly_one_winner() {
    // (case, the first call, the second call) — the request members of each, with the
    // WebAppId filled in per row.
    let pairs: Vec<(&str, Call, Call)> = vec![
        (
            "two updates with different titles",
            ("UpdateWebAppCustomization", json!({"Title": "first"})),
            ("UpdateWebAppCustomization", json!({"Title": "second"})),
        ),
        (
            "two updates assigning different members",
            ("UpdateWebAppCustomization", json!({"Title": "only-title"})),
            (
                "UpdateWebAppCustomization",
                json!({"LogoFile": aws_smithy_types::base64::encode(b"logo")}),
            ),
        ),
        (
            "an update against a delete",
            ("UpdateWebAppCustomization", json!({"Title": "assigned"})),
            ("DeleteWebAppCustomization", json!({})),
        ),
        (
            "a delete against an update",
            ("DeleteWebAppCustomization", json!({})),
            ("UpdateWebAppCustomization", json!({"Title": "assigned"})),
        ),
        (
            "two deletes",
            ("DeleteWebAppCustomization", json!({})),
            ("DeleteWebAppCustomization", json!({})),
        ),
    ];

    for (case, (first_op, first_members), (second_op, second_members)) in pairs {
        let emulator = Emulator::new();
        let web_app_id = create_web_app(&emulator).await;
        // A customization is already in place, so a delete has something to remove and
        // the losing writer cannot be mistaken for "there was nothing to write".
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

        let mut first = first_members.clone();
        first["WebAppId"] = json!(&web_app_id);
        let mut second = second_members.clone();
        second["WebAppId"] = json!(&web_app_id);

        let (first_answer, second_answer) = tokio::join!(
            emulator.call(first_op, first),
            emulator.call(second_op, second)
        );

        let answers = [
            (first_op, &first_answer, &first_members),
            (second_op, &second_answer, &second_members),
        ];
        let winners = answers
            .iter()
            .filter(|(_, answer, _)| answer.status == 200)
            .count();
        assert_eq!(
            winners, 1,
            "case {case:?}: expected exactly one winning write, got {} and {} with bodies {} / {}",
            first_answer.status, second_answer.status, first_answer.body, second_answer.body
        );
        for (op, answer, _) in &answers {
            if answer.status != 200 {
                assert_conflict(&format!("{case} ({op} lost)"), answer);
            }
        }

        // The surviving customization is the WINNER's: the loser's values reached
        // nothing.
        let (winning_op, _, winning_members) = answers
            .iter()
            .find(|(_, answer, _)| answer.status == 200)
            .expect("one write won");
        let title = stored_title(&emulator, &web_app_id).await;
        let expected = if *winning_op == "DeleteWebAppCustomization" {
            None
        } else {
            Some(
                member_of(winning_members, "Title")
                    .as_str()
                    .unwrap_or("seeded")
                    .to_string(),
            )
        };
        assert_eq!(
            title, expected,
            "case {case:?}: expected the stored title to be the winner's ({winning_op}), \
             got {title:?}"
        );
    }
}

/// RULE (the same rule, at scale): however many writers overlap, exactly one of them
/// commits and every other is refused with the conflict.
#[tokio::test]
async fn only_one_of_many_overlapping_writers_commits() {
    // The rows differ along how many writers race at once.
    for writers in [2usize, 3, 5] {
        let case = format!("{writers} overlapping updates");
        let emulator = Emulator::new();
        let web_app_id = create_web_app(&emulator).await;

        let calls = (0..writers).map(|index| {
            emulator.call(
                "UpdateWebAppCustomization",
                json!({"WebAppId": &web_app_id, "Title": format!("writer-{index}")}),
            )
        });
        let answers = futures_of(calls).await;

        let served: Vec<usize> = answers
            .iter()
            .enumerate()
            .filter(|(_, answer)| answer.status == 200)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            served.len(),
            1,
            "case {case:?}: expected exactly one winner, got the statuses {:?}",
            answers.iter().map(|answer| answer.status).collect::<Vec<_>>()
        );
        for (index, answer) in answers.iter().enumerate() {
            if answer.status != 200 {
                assert_conflict(&format!("{case} (writer {index})"), answer);
            }
        }
        let winner = served[0];
        assert_eq!(
            stored_title(&emulator, &web_app_id).await,
            Some(format!("writer-{winner}")),
            "case {case:?}: expected the stored title to be writer {winner}'s"
        );
    }
}

/// Drive every call TOGETHER and collect the answers in the order the calls were made.
///
/// `pub(crate)` for the reason [`create_web_app`] is: overlapping calls are made to overlap
/// in ONE way in this crate's tests.
pub(crate) async fn futures_of<F>(calls: impl Iterator<Item = F>) -> Vec<Answer>
where
    F: std::future::Future<Output = Answer>,
{
    // `join_all` over the service's own futures: they are polled on one task, so each one
    // reaches the service while the others are still in flight.
    let mut pending: Vec<std::pin::Pin<Box<F>>> = calls.map(Box::pin).collect();
    let mut answers: Vec<Option<Answer>> = (0..pending.len()).map(|_| None).collect();
    let mut remaining = pending.len();
    while remaining > 0 {
        for (index, call) in pending.iter_mut().enumerate() {
            if answers[index].is_some() {
                continue;
            }
            if let std::task::Poll::Ready(answer) = poll_once(call.as_mut()).await {
                answers[index] = Some(answer);
                remaining -= 1;
            }
        }
    }
    answers
        .into_iter()
        .map(|answer| answer.expect("every call answered"))
        .collect()
}

/// Poll `future` exactly once, so a caller can interleave several by hand.
async fn poll_once<F: std::future::Future>(
    mut future: std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    std::future::poll_fn(move |cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await
}

/// RULE (the other half): writes that do NOT overlap are each applied — the guard is
/// mutual exclusion between calls in flight at the same time, not a window the web app
/// sits in — so any number of writes made one after another all commit.
///
/// The rows are one sequence over a single web app, differing along the operation and
/// along what each step assigns.
#[tokio::test]
async fn writes_that_do_not_overlap_are_all_applied() {
    let emulator = Emulator::new();
    let web_app_id = create_web_app(&emulator).await;

    // (step, operation, members, the title the customization holds afterwards)
    let steps: Vec<(&str, &str, Value, Option<&str>)> = vec![
        (
            "an update",
            "UpdateWebAppCustomization",
            json!({"Title": "one"}),
            Some("one"),
        ),
        (
            "a second update straight after it",
            "UpdateWebAppCustomization",
            json!({"Title": "two"}),
            Some("two"),
        ),
        (
            "a third update straight after that",
            "UpdateWebAppCustomization",
            json!({"Title": "three"}),
            Some("three"),
        ),
        (
            "a delete",
            "DeleteWebAppCustomization",
            json!({}),
            None,
        ),
        (
            "a repeat of the delete",
            "DeleteWebAppCustomization",
            json!({}),
            None,
        ),
        (
            "an update that rebuilds the object",
            "UpdateWebAppCustomization",
            json!({"Title": "rebuilt"}),
            Some("rebuilt"),
        ),
        (
            "a delete again",
            "DeleteWebAppCustomization",
            json!({}),
            None,
        ),
    ];

    for (step, operation, members, expected) in steps {
        let mut request = members.clone();
        request["WebAppId"] = json!(&web_app_id);
        let answer = emulator.call(operation, request).await;
        assert_eq!(
            answer.status, 200,
            "case {step:?}: a write that overlaps nothing must be applied, got {} with body {}",
            answer.status, answer.body
        );
        let title = stored_title(&emulator, &web_app_id).await;
        assert_eq!(
            title.as_deref(),
            expected,
            "case {step:?}: expected the stored title {expected:?}, got {title:?}"
        );
    }
}

/// RULE (the claim is the request's, not the resource's): a write that never completes
/// — a caller that goes away while its call is in flight — leaves no claim behind, so
/// the next write of that web app's customization is served rather than refused.
///
/// The rows differ along which operation is abandoned mid-flight. Each row first shows
/// the claim IS held while the abandoned call is suspended (a concurrent write is
/// refused), then drops that call and shows the next write is served.
#[tokio::test]
async fn an_abandoned_write_leaves_no_claim_behind() {
    let emulator = Emulator::new();

    for (case, operation, members) in [
        (
            "an abandoned update",
            "UpdateWebAppCustomization",
            json!({"Title": "abandoned"}),
        ),
        ("an abandoned delete", "DeleteWebAppCustomization", json!({})),
    ] {
        let web_app_id = create_web_app(&emulator).await;
        let mut request = members.clone();
        request["WebAppId"] = json!(&web_app_id);

        {
            let mut abandoned = Box::pin(emulator.call(operation, request));
            let polled = poll_once(abandoned.as_mut()).await;
            assert!(
                polled.is_pending(),
                "case {case:?}: the abandoned call must still be in flight, or the row \
                 abandons no claim"
            );

            // It really holds the claim: a write that arrives now is the racing one.
            let racing = emulator
                .call(
                    "UpdateWebAppCustomization",
                    json!({"WebAppId": &web_app_id, "Title": "racing"}),
                )
                .await;
            assert_conflict(&format!("{case} (a write while it is in flight)"), &racing);
        }

        // The abandoned call is gone, and with it its claim.
        let after = emulator
            .call(
                "UpdateWebAppCustomization",
                json!({"WebAppId": &web_app_id, "Title": "after"}),
            )
            .await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the write after the abandoned one must be served, got {} with \
             body {}",
            after.status, after.body
        );
        assert_eq!(
            stored_title(&emulator, &web_app_id).await,
            Some("after".to_string()),
            "case {case:?}: expected the served write's title to be stored"
        );
    }
}

/// RULE (the guard's scope): the claim is per WEB APP, so writes of different web apps
/// that overlap all commit.
///
/// The rows differ along how many web apps are written at once.
#[tokio::test]
async fn overlapping_writes_of_different_web_apps_all_commit() {
    for apps in [2usize, 4] {
        let case = format!("{apps} web apps written at once");
        let emulator = Emulator::new();
        let mut ids = Vec::new();
        for _ in 0..apps {
            ids.push(create_web_app(&emulator).await);
        }

        let calls = ids.iter().enumerate().map(|(index, web_app_id)| {
            emulator.call(
                "UpdateWebAppCustomization",
                json!({"WebAppId": web_app_id, "Title": format!("app-{index}")}),
            )
        });
        let answers = futures_of(calls).await;

        for (index, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: web app {index}'s own write must be served, got {} with body {}",
                answer.status, answer.body
            );
        }
        for (index, web_app_id) in ids.iter().enumerate() {
            assert_eq!(
                stored_title(&emulator, web_app_id).await,
                Some(format!("app-{index}")),
                "case {case:?}: expected web app {index} to hold its own title"
            );
        }
    }
}
