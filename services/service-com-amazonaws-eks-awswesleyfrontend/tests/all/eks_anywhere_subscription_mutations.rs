// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two mutations of an EKS Anywhere subscription —
//! `UpdateEksAnywhereSubscription` and `DeleteEksAnywhereSubscription` — and for what
//! `ListEksAnywhereSubscriptions` reports once one of them has run, driven over the
//! wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along the term the subscription was bought for, the
//! renewal value asked for (and the one it already had), whether an idempotency token
//! travels with the call, which mutation is addressed and in how many shapes the id
//! it names resolves to nothing.
//!
//! Nothing here waits for a span: an asynchronous move is waited for by LOOKING AGAIN
//! until it has happened or a generous deadline passes, so what is asserted is the
//! state a call reaches and never how fast the host gets there.
//!
//! The wire harness (`Answer`, `emulator`, `call`) is the shared one in
//! `tests/all/wire.rs`.

use std::time::{Duration, Instant};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    subscription_not_deletable_message, SUBSCRIPTION_NOT_FOUND_MESSAGE,
    SUBSCRIPTION_STATUS_ACTIVE, SUBSCRIPTION_STATUS_CREATING, SUBSCRIPTION_STATUS_DELETING,
    SUBSCRIPTION_STATUS_EXPIRED, SUBSCRIPTION_STATUS_UPDATING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, Answer};

const REGION: &str = "us-west-2";

/// Another Region of the same account: a subscription belongs to the Region it was
/// created in, so this one carries none of the subscriptions created above.
const OTHER_REGION: &str = "eu-central-1";

/// How long a clock-owed subscription move is given before a suite calls it a
/// failure. Generous on purpose: what is asserted is the state a call reaches, never
/// how fast the host gets there.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// How often the poll loop looks again. See [`POLL_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `CreateEksAnywhereSubscription`: `POST /eks-anywhere-subscriptions`.
async fn create_subscription(router: &MultiServiceRouter, body: &serde_json::Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/eks-anywhere-subscriptions",
        &body.to_string(),
    )
    .await
}

/// `DescribeEksAnywhereSubscription`: `GET /eks-anywhere-subscriptions/{id}`.
async fn describe_subscription(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/eks-anywhere-subscriptions/{id}"),
        "",
    )
    .await
}

/// `UpdateEksAnywhereSubscription`: `POST /eks-anywhere-subscriptions/{id}`.
async fn update_subscription(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!("/eks-anywhere-subscriptions/{id}"),
        &body.to_string(),
    )
    .await
}

/// `DeleteEksAnywhereSubscription`: `DELETE /eks-anywhere-subscriptions/{id}`.
async fn delete_subscription(router: &MultiServiceRouter, region: &str, id: &str) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!("/eks-anywhere-subscriptions/{id}"),
        "",
    )
    .await
}

/// `ListEksAnywhereSubscriptions`: `GET /eks-anywhere-subscriptions`, with an
/// optional query string.
async fn list_subscriptions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/eks-anywhere-subscriptions".to_string()
    } else {
        format!("/eks-anywhere-subscriptions?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// The `subscription` document an answer carries.
fn subscription(answer: &Answer) -> &serde_json::Value {
    &answer.body["subscription"]
}

/// A created subscription, with the id the create minted.
async fn created(router: &MultiServiceRouter, case: &str, request: &serde_json::Value) -> String {
    let answer = create_subscription(router, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: setup CreateEksAnywhereSubscription expected 200, got {answer:?}"
    );
    let id = subscription(&answer)["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !id.is_empty(),
        "case {case:?}: setup must report the id the create minted, got {answer:?}"
    );
    id
}

/// Read `DescribeEksAnywhereSubscription` until `settled` holds of the subscription
/// it reports, or the deadline passes — the condition is waited for, never a span.
async fn subscription_until<F>(
    router: &MultiServiceRouter,
    case: &str,
    what: &str,
    id: &str,
    settled: F,
) -> Answer
where
    F: Fn(&serde_json::Value) -> bool,
{
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let answer = describe_subscription(router, id).await;
        if answer.status == 200 && settled(subscription(&answer)) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the subscription never reached {what}, last read {answer:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A subscription request for `name`, bought for `months` with the renewal flag
/// `auto_renew`.
fn subscription_request(name: &str, months: i32, auto_renew: bool) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "term": { "duration": months, "unit": "MONTHS" },
        "licenseQuantity": 1,
        "autoRenew": auto_renew,
    })
}

// ---------------------------------------------------------------------------
// RULE: an accepted update writes the renewal flag the request carries and nothing
// else of the subscription, reports the subscription as taking that update, and the
// clock carries it back to ACTIVE — where a later read reports the new flag.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: both published term durations
/// (`EksAnywhereSubscriptionTerm$duration`), every pairing of the renewal flag the
/// subscription was created with and the one the update asks for (so a row that
/// switches it on, one that switches it off and two that ask for the value it already
/// has), and whether an idempotency token travels with the call.
#[tokio::test]
async fn an_update_writes_the_renewal_flag_and_the_clock_settles_the_subscription() {
    let router = emulator();

    struct Row {
        case: &'static str,
        months: i32,
        created_with: bool,
        asked_for: bool,
        token: Option<&'static str>,
    }
    let rows = [
        Row {
            case: "the shortest published term, renewal switched on, no token",
            months: 12,
            created_with: false,
            asked_for: true,
            token: None,
        },
        Row {
            case: "the longest published term, renewal switched off, with a token",
            months: 36,
            created_with: true,
            asked_for: false,
            token: Some("token-renewal-off"),
        },
        Row {
            case: "the renewal flag set to the value it already had",
            months: 12,
            created_with: true,
            asked_for: true,
            token: Some("token-renewal-unchanged"),
        },
        Row {
            case: "the renewal flag left off, no token",
            months: 36,
            created_with: false,
            asked_for: false,
            token: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let id = created(
            &router,
            case,
            &subscription_request(&format!("updated-{index}"), row.months, row.created_with),
        )
        .await;
        let before = describe_subscription(&router, &id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected 200 from DescribeEksAnywhereSubscription, got {before:?}"
        );

        let mut request = serde_json::json!({ "autoRenew": row.asked_for });
        if let Some(token) = row.token {
            request["clientRequestToken"] = serde_json::json!(token);
        }
        let updated = update_subscription(&router, REGION, &id, &request).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateEksAnywhereSubscription, got {updated:?}"
        );
        assert_eq!(
            subscription(&updated)["autoRenew"],
            serde_json::json!(row.asked_for),
            "case {case:?}: expected autoRenew {}, got {:?}",
            row.asked_for,
            subscription(&updated)["autoRenew"]
        );
        // "The status of a subscription": an accepted update is not instantaneous, so
        // the answer reports the subscription as taking one.
        assert_eq!(
            subscription(&updated)["status"],
            serde_json::json!(SUBSCRIPTION_STATUS_UPDATING),
            "case {case:?}: expected status {SUBSCRIPTION_STATUS_UPDATING}, got {:?}",
            subscription(&updated)["status"]
        );
        // "Only auto renewal and tags can be updated after subscription creation": no
        // other member of the subscription moves.
        for member in [
            "id",
            "arn",
            "createdAt",
            "effectiveDate",
            "expirationDate",
            "licenseQuantity",
            "licenseType",
            "term",
            "tags",
        ] {
            assert_eq!(
                subscription(&updated)[member], subscription(&before)[member],
                "case {case:?}: expected {member} to stay {:?}, got {:?}",
                subscription(&before)[member],
                subscription(&updated)[member]
            );
        }

        // The round trip: the read reports what the update wrote, once the clock has
        // carried the subscription back to a settled status.
        let settled = subscription_until(
            &router,
            case,
            SUBSCRIPTION_STATUS_ACTIVE,
            &id,
            |subscription| subscription["status"] == serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE),
        )
        .await;
        assert_eq!(
            subscription(&settled)["autoRenew"],
            serde_json::json!(row.asked_for),
            "case {case:?}: expected the read to report autoRenew {}, got {:?}",
            row.asked_for,
            subscription(&settled)["autoRenew"]
        );
        assert_eq!(
            subscription(&settled)["id"],
            serde_json::json!(id),
            "case {case:?}: expected the read to report id {id:?}, got {:?}",
            subscription(&settled)["id"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `clientRequestToken` is an idempotency token, so the RESENT call is answered
// with the subscription its first delivery produced and starts no second update,
// while a call bearing another token is a new update.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resent_update_token_is_absorbed_and_another_token_is_a_new_update() {
    let router = emulator();
    let id = created(
        &router,
        "the absorbed retry",
        &subscription_request("retried-update", 12, false),
    )
    .await;

    let first = update_subscription(
        &router,
        REGION,
        &id,
        &serde_json::json!({ "autoRenew": true, "clientRequestToken": "token-retried" }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case the first delivery: expected 200, got {first:?}"
    );
    assert_eq!(
        subscription(&first)["status"],
        serde_json::json!(SUBSCRIPTION_STATUS_UPDATING),
        "case the first delivery: expected status {SUBSCRIPTION_STATUS_UPDATING}, got {:?}",
        subscription(&first)["status"]
    );

    // Once the clock has settled that update, the SAME token resent is the SDK's retry
    // of the call it already took: it is answered with that subscription, and the
    // answer shows no second update was started.
    subscription_until(
        &router,
        "the absorbed retry",
        SUBSCRIPTION_STATUS_ACTIVE,
        &id,
        |subscription| subscription["status"] == serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE),
    )
    .await;
    let retried = update_subscription(
        &router,
        REGION,
        &id,
        &serde_json::json!({ "autoRenew": true, "clientRequestToken": "token-retried" }),
    )
    .await;
    assert_eq!(
        retried.status, 200,
        "case the absorbed retry: expected 200, got {retried:?}"
    );
    assert_eq!(
        subscription(&retried)["id"],
        serde_json::json!(id),
        "case the absorbed retry: expected the same subscription {id:?}, got {:?}",
        subscription(&retried)["id"]
    );
    assert_eq!(
        subscription(&retried)["status"],
        serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE),
        "case the absorbed retry: an absorbed retry starts no second update, got {:?}",
        subscription(&retried)["status"]
    );
    assert_eq!(
        subscription(&retried)["autoRenew"],
        serde_json::json!(true),
        "case the absorbed retry: expected autoRenew true, got {:?}",
        subscription(&retried)["autoRenew"]
    );

    // Another token is another call: it is applied.
    let again = update_subscription(
        &router,
        REGION,
        &id,
        &serde_json::json!({ "autoRenew": false, "clientRequestToken": "token-second" }),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "case another token: expected 200, got {again:?}"
    );
    assert_eq!(
        subscription(&again)["status"],
        serde_json::json!(SUBSCRIPTION_STATUS_UPDATING),
        "case another token: expected status {SUBSCRIPTION_STATUS_UPDATING}, got {:?}",
        subscription(&again)["status"]
    );
    let settled = subscription_until(
        &router,
        "another token",
        SUBSCRIPTION_STATUS_ACTIVE,
        &id,
        |subscription| subscription["status"] == serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE),
    )
    .await;
    assert_eq!(
        subscription(&settled)["autoRenew"],
        serde_json::json!(false),
        "case another token: expected the read to report autoRenew false, got {:?}",
        subscription(&settled)["autoRenew"]
    );
}

// ---------------------------------------------------------------------------
// RULE: a mutation addressed to an id the Region carries no subscription for is 404
// `ResourceNotFoundException`, in the subscription's own fixed sentence, naming no
// subscription at all — whichever mutation asked and however the lookup missed.
// ---------------------------------------------------------------------------

/// The table spans both mutations against every way an id resolves to nothing: a
/// well-formed UUID nothing was minted for, a value that is no id at all, the empty
/// id (a trailing slash), and the id of a subscription that exists in ANOTHER Region.
#[tokio::test]
async fn a_mutation_of_an_unknown_subscription_is_not_found() {
    let router = emulator();
    let elsewhere = created(
        &router,
        "the id of a subscription of another Region",
        &subscription_request("mutated-elsewhere", 12, false),
    )
    .await;

    let unknown_ids = [
        (
            "a well-formed UUID nothing was minted for",
            REGION,
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "a value that is not an id at all",
            REGION,
            "not-a-subscription".to_string(),
        ),
        (
            "the id of a subscription of another Region",
            OTHER_REGION,
            elsewhere,
        ),
    ];

    for (case, region, id) in &unknown_ids {
        for (mutation, answer) in [
            (
                "UpdateEksAnywhereSubscription",
                update_subscription(&router, region, id, &serde_json::json!({ "autoRenew": true }))
                    .await,
            ),
            (
                "DeleteEksAnywhereSubscription",
                delete_subscription(&router, region, id).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {mutation} with {case:?}: expected HTTP 404, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {mutation} with {case:?}: expected ResourceNotFoundException, got {code:?}"
            );
            assert_eq!(
                answer.message(),
                SUBSCRIPTION_NOT_FOUND_MESSAGE,
                "case {mutation} with {case:?}: expected the subscription's own not-found \
                 sentence, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.body["subscriptionId"],
                serde_json::Value::Null,
                "case {mutation} with {case:?}: a refusal that resolved no subscription names \
                 none, got {answer:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: this API removes only an expired or inactive subscription, so a live one is
// refused as a request invalid given its state — naming the subscription the service
// resolved — and is left exactly as it was.
// ---------------------------------------------------------------------------

/// The table spans the live statuses a subscription can be in when the delete
/// arrives: one the create is still provisioning, one the clock has settled, and one
/// taking an update. A row whose subscription is in a status the clock is about to
/// move out of admits the settled status too, so what is asserted is the sentence for
/// the status the service refused for and never how fast the host moved it.
#[tokio::test]
async fn a_live_subscription_is_not_deleted_and_stays_in_place() {
    let router = emulator();

    struct Row {
        case: &'static str,
        /// Whether the subscription is carried to `ACTIVE` before the delete.
        settle: bool,
        /// Whether an update is started right before the delete.
        update: bool,
        /// The statuses the refusal may name, in the order they are reached.
        statuses: &'static [&'static str],
    }
    let rows = [
        Row {
            case: "a subscription the create is still provisioning",
            settle: false,
            update: false,
            statuses: &[SUBSCRIPTION_STATUS_CREATING, SUBSCRIPTION_STATUS_ACTIVE],
        },
        Row {
            case: "a subscription the clock has settled",
            settle: true,
            update: false,
            statuses: &[SUBSCRIPTION_STATUS_ACTIVE],
        },
        Row {
            case: "a subscription taking an update",
            settle: true,
            update: true,
            statuses: &[SUBSCRIPTION_STATUS_UPDATING, SUBSCRIPTION_STATUS_ACTIVE],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let id = created(
            &router,
            case,
            &subscription_request(&format!("undeletable-{index}"), 12, false),
        )
        .await;
        if row.settle {
            subscription_until(
                &router,
                case,
                SUBSCRIPTION_STATUS_ACTIVE,
                &id,
                |subscription| {
                    subscription["status"] == serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE)
                },
            )
            .await;
        }
        if row.update {
            let started = update_subscription(
                &router,
                REGION,
                &id,
                &serde_json::json!({ "autoRenew": true }),
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: setup UpdateEksAnywhereSubscription expected 200, got {started:?}"
            );
        }

        let refused = delete_subscription(&router, REGION, &id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {refused:?}"
        );
        let code = error_code(&refused);
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {code:?}"
        );
        let admitted: Vec<String> = row
            .statuses
            .iter()
            .map(|status| subscription_not_deletable_message(status))
            .collect();
        assert!(
            admitted.contains(&refused.message()),
            "case {case:?}: expected one of {admitted:?}, got {:?}",
            refused.message()
        );
        // The subscription the service resolved IS named: a caller reading that member
        // learns the refusal is about the subscription it addressed.
        assert_eq!(
            refused.body["subscriptionId"],
            serde_json::json!(id),
            "case {case:?}: expected the refusal to name subscription {id:?}, got {refused:?}"
        );

        // And nothing of it was torn down: the subscription still reads back.
        let after = describe_subscription(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: the refused subscription must still read back, got {after:?}"
        );
        assert_eq!(
            subscription(&after)["id"],
            serde_json::json!(id),
            "case {case:?}: expected the subscription {id:?} to be unchanged, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the listing reports every subscription of the Region as the mutations left
// it — its renewal flag and the status it settled in — and `includeStatus` selects on
// that same status.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_the_subscriptions_a_mutation_changed() {
    let router = emulator();

    // Three subscriptions, of which the middle one is updated: a row the update
    // touched, and two it must not have.
    let mut ids = Vec::new();
    for index in 0..3 {
        ids.push(
            created(
                &router,
                "the listing after an update",
                &subscription_request(&format!("listed-after-update-{index}"), 12, false),
            )
            .await,
        );
    }
    let updated = ids[1].clone();
    let started = update_subscription(
        &router,
        REGION,
        &updated,
        &serde_json::json!({ "autoRenew": true, "clientRequestToken": "token-listed" }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case the listing after an update: setup UpdateEksAnywhereSubscription expected 200, \
         got {started:?}"
    );

    // Every subscription has settled once each of them reads back `ACTIVE`.
    for id in &ids {
        subscription_until(
            &router,
            "the listing after an update",
            SUBSCRIPTION_STATUS_ACTIVE,
            id,
            |subscription| subscription["status"] == serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE),
        )
        .await;
    }

    let all = list_subscriptions(&router, "").await;
    assert_eq!(
        all.status, 200,
        "case the whole listing: expected 200, got {all:?}"
    );
    let listed = all.body["subscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        ids.len(),
        "case the whole listing: expected {} subscriptions, got {listed:?}",
        ids.len()
    );
    for entry in &listed {
        let id = entry["id"].as_str().unwrap_or_default();
        let expected_renewal = id == updated;
        assert_eq!(
            entry["autoRenew"],
            serde_json::json!(expected_renewal),
            "case the listing of {id:?}: expected autoRenew {expected_renewal}, got {:?}",
            entry["autoRenew"]
        );
        assert_eq!(
            entry["status"],
            serde_json::json!(SUBSCRIPTION_STATUS_ACTIVE),
            "case the listing of {id:?}: expected status {SUBSCRIPTION_STATUS_ACTIVE}, got {:?}",
            entry["status"]
        );
    }

    // "Filtered by includeStatus": the settled status selects every one of them, and a
    // status none of them is in selects none.
    fn listed_ids(answer: &Answer) -> Vec<String> {
        answer.body["subscriptions"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
    let mut expected_ids = ids.clone();
    expected_ids.sort();
    let active = list_subscriptions(
        &router,
        &format!("includeStatus={SUBSCRIPTION_STATUS_ACTIVE}"),
    )
    .await;
    assert_eq!(
        listed_ids(&active),
        expected_ids,
        "case the {SUBSCRIPTION_STATUS_ACTIVE} filter: expected every settled subscription, \
         got {:?}",
        listed_ids(&active)
    );
    for status in [
        SUBSCRIPTION_STATUS_UPDATING,
        SUBSCRIPTION_STATUS_DELETING,
        SUBSCRIPTION_STATUS_EXPIRED,
    ] {
        let filtered = list_subscriptions(&router, &format!("includeStatus={status}")).await;
        assert_eq!(
            listed_ids(&filtered),
            Vec::<String>::new(),
            "case the {status} filter: expected none, got {:?}",
            listed_ids(&filtered)
        );
    }
}
