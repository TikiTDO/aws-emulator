// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteApplication`: what a delete leaves behind and what the
//! read paths report once the clock has taken the record away.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the tagging paths from [`super::resource_tags`];
//! this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a delete of an application this account holds answers with the modelled empty
//!   response, leaves the record resting in `DELETING` — which is what a read of it and
//!   the listing then report — and the clock takes the record away one span later,
//!   after which no read path reports the application at all. Asserted over a table of
//!   seven creates differing in the members they carry (data sources, configuration
//!   settings, tags, a customer-managed key, a client token, explicitly empty lists)
//!   and in the length of the name (both published bounds);
//! * an id that names no application of this account — one never minted, and one whose
//!   removal has already landed — is refused with the modelled
//!   `ResourceNotFoundException` and deletes nothing;
//! * asking twice while the removal is in flight is the same delete: it answers as the
//!   first did and the application is still gone once the span has passed;
//! * the record going releases what it held: its name can be created again, its tags go
//!   with it, and the slot it occupied in this account's bound is free for another
//!   create (one slot, and no more).

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::APPLICATION_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    APPLICATION_DOES_NOT_EXIST, APPLICATION_QUOTA, APPLICATION_STATUS_ACTIVE,
    APPLICATION_STATUS_CREATING, APPLICATION_STATUS_DELETING,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::resource_tags::{add_tags, list_tags, reported_tags, tag, wire_tags};

/// A customer-managed key ARN satisfying `KmsKeyArn`'s own pattern, for a create that
/// names one.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// One of the two values `AppConfigType` declares
/// (`smithy_query(query="shape",shape="AppConfigType")`).
const APP_CONFIG_GROUPS: &str = "opensearchDashboards.dashboardAdmin.groups";

/// `CreateApplication`: `POST /2021-01-01/opensearch/application`.
async fn create_application(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(body),
        )
        .await
}

/// `GetApplication`: `GET /2021-01-01/opensearch/application/{id}`.
async fn get_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/application/{id}"), None)
        .await
}

/// `DeleteApplication`: `DELETE /2021-01-01/opensearch/application/{id}`.
pub(crate) async fn delete_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await
}

/// `ListApplications`: `GET /2021-01-01/opensearch/list-applications`, with `query`
/// carrying whatever the case asks for (`&statuses=…`).
async fn list_applications(emulator: &Emulator, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/list-applications")
    } else {
        format!("{PREFIX}/opensearch/list-applications?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `id`s a listing answer reports.
fn listed_ids(answer: &Answer, case: &str) -> Vec<String> {
    answer.body["ApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an ApplicationSummaries list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|row| string_member(row, "id", case))
        .collect()
}

/// The `status` a listing reports for `id`, or `None` when it reports no such row.
fn listed_status(answer: &Answer, id: &str, case: &str) -> Option<String> {
    answer.body["ApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an ApplicationSummaries list, got {:?}",
                answer.body
            )
        })
        .iter()
        .find(|row| row["id"].as_str() == Some(id))
        .map(|row| string_member(row, "status", case))
}

/// The application `id` names, asserted to be one no read path reports any more.
async fn assert_gone(emulator: &Emulator, id: &str, case: &str) {
    let read = get_application(emulator, id).await;
    assert_eq!(
        read.status, RECORDED_NOT_FOUND_STATUS,
        "case {case}: a removed application resolves for nobody, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        error_code(&read),
        "ResourceNotFoundException",
        "case {case}: the refusal is the modelled not-found, got {:?}",
        read.body
    );
    assert_eq!(
        read.body["message"],
        json!(APPLICATION_DOES_NOT_EXIST),
        "case {case}: the refusal states that the account holds no such application, got {:?}",
        read.body
    );

    let listing = list_applications(emulator, "").await;
    assert!(
        !listed_ids(&listing, case).contains(&id.to_string()),
        "case {case}: the listing reports no removed application, got {:?}",
        listing.body
    );
}

/// The table the deletion rule is asserted over: one row per way a create can differ —
/// the members it carries, the lists' sizes, and the name at each published length
/// bound — because what a delete does is the same whatever the record holds.
fn creates() -> Vec<(&'static str, Value)> {
    vec![
        (
            "only the required name, at the 3-character lower bound",
            json!({ "name": "ui1" }),
        ),
        (
            // `ApplicationName`'s upper bound is 30 characters; this name is exactly 30.
            "a 30-character name at the upper bound",
            json!({ "name": "abcdefghij-klmnopqrst-uvwxyz12" }),
        ),
        (
            "two data sources",
            json!({
                "name": "sources-ui",
                "dataSources": [
                    { "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/logs" },
                    {
                        "dataSourceArn": "arn:aws:s3:::my-catalog",
                        "dataSourceDescription": "an S3 Glue catalog",
                    },
                ],
            }),
        ),
        (
            "a configuration setting",
            json!({
                "name": "admin-ui",
                "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "operators" }],
            }),
        ),
        (
            "tags",
            json!({
                "name": "tagged-ui",
                "tagList": [
                    { "Key": "team", "Value": "search" },
                    { "Key": "cost-center", "Value": "" },
                ],
            }),
        ),
        (
            "a customer-managed key and IAM Identity Center",
            json!({
                "name": "federated-ui",
                "kmsKeyArn": KMS_KEY,
                "iamIdentityCenterOptions": {
                    "enabled": true,
                    "iamIdentityCenterInstanceArn":
                        "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                },
            }),
        ),
        (
            "a client token and explicitly empty lists",
            json!({
                "name": "token-ui",
                "clientToken": "delete-me-once",
                "dataSources": [],
                "appConfigs": [],
                "tagList": [],
            }),
        ),
    ]
}

/// RULE: a delete of a resource this account holds answers with the modelled response,
/// leaves the record resting in its deleting status, and the clock takes the record away
/// one span later — after which no read path reports the resource.
#[tokio::test]
async fn a_deleted_application_rests_in_deleting_until_the_clock_removes_it() {
    let emulator = Emulator::new();
    let mut deleted: Vec<(&str, String)> = Vec::new();

    for (case, request) in creates() {
        let created = create_application(&emulator, request).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the create is served, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);

        let dropped = delete_application(&emulator, &id).await;
        assert_eq!(
            dropped.status, 200,
            "case {case}: the delete is served, got {} with {:?}",
            dropped.status, dropped.body
        );
        assert_eq!(
            dropped.body,
            json!({}),
            "case {case}: `DeleteApplicationResponse` declares no members, got {:?}",
            dropped.body
        );

        // The record is not gone where it stands: the application is being deleted, and
        // that is the status both read paths report until the removal falls due.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: the application being deleted still resolves, got {} with {:?}",
            read.status, read.body
        );
        let status = string_member(&read.body, "status", case);
        assert_eq!(
            status, APPLICATION_STATUS_DELETING,
            "case {case}: a deleted application rests in its deleting status, got {status:?}"
        );
        assert!(
            APPLICATION_STATUS_VALUES.contains(&status.as_str()),
            "case {case}: the status is one the model declares, got {status:?}"
        );

        let listing = list_applications(&emulator, "").await;
        assert_eq!(
            listed_status(&listing, &id, case),
            Some(APPLICATION_STATUS_DELETING.to_string()),
            "case {case}: the listing reports the row from the same record, got {:?}",
            listing.body
        );
        let selected = list_applications(
            &emulator,
            &format!("statuses={APPLICATION_STATUS_DELETING}"),
        )
        .await;
        assert!(
            listed_ids(&selected, case).contains(&id),
            "case {case}: the status filter selects the application being deleted, got {:?}",
            selected.body
        );
        for status in [APPLICATION_STATUS_CREATING, APPLICATION_STATUS_ACTIVE] {
            let narrowed = list_applications(&emulator, &format!("statuses={status}")).await;
            assert!(
                !listed_ids(&narrowed, case).contains(&id),
                "case {case}: a {status} filter does not select it, got {:?}",
                narrowed.body
            );
        }

        deleted.push((case, id));
    }

    // Nothing below asks for the removal: the span each delete was answered with falls
    // due on its own, and the next call materialises it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, id) in &deleted {
        assert_gone(&emulator, id, case).await;
    }
}

/// RULE: an id that names no resource of this account is already in the state a delete
/// asks for, so the call is ANSWERED rather than refused — and it removes nothing else.
#[tokio::test]
async fn an_id_that_names_no_application_is_answered_and_removes_nothing() {
    let emulator = Emulator::new();

    // The account holds one application throughout, so what each row below exercises is
    // the id rather than an empty store.
    let created = create_application(&emulator, json!({ "name": "held-ui" })).await;
    let held = string_member(&created.body, "id", "held-ui");

    // An application whose removal has already landed: the id was minted by this
    // account, and names nothing by the time the row runs.
    let removed = create_application(&emulator, json!({ "name": "removed-ui" })).await;
    let removed = string_member(&removed.body, "id", "removed-ui");
    assert_eq!(
        delete_application(&emulator, &removed).await.status,
        200,
        "case removed-ui: the delete is served"
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    // One row per shape of well-formed id the account holds nothing under: the shortest
    // the pattern allows, the longest, a digits-only one, the held id with one character
    // changed, and the id of the application the clock has already removed.
    let mut absent = vec![
        ("the 3-character lower bound".to_string(), "abc".to_string()),
        ("the 30-character upper bound".to_string(), "a".repeat(30)),
        ("digits only".to_string(), "0123456789".to_string()),
        (
            "the held id with one character changed".to_string(),
            format!("{}z", &held[..held.len() - 1]),
        ),
        (
            "an application already removed".to_string(),
            removed.clone(),
        ),
    ];
    absent.retain(|(_, id)| *id != held);

    for (case, id) in absent {
        let dropped = delete_application(&emulator, &id).await;
        assert_eq!(
            dropped.status, 200,
            "case {case}: an id naming no application is answered, got {} with {:?}",
            dropped.status, dropped.body
        );

        // And the read of that id still reports nothing: the answered delete minted no
        // record and the listing holds none under it.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            (read.status, error_code(&read).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {case}: the id still names no application, got {} with {:?}",
            read.status,
            read.body
        );
        assert_eq!(
            read.body["message"],
            json!(APPLICATION_DOES_NOT_EXIST),
            "case {case}: the read states that the account holds no such application, got {:?}",
            read.body
        );
    }

    // The application the account does hold is untouched by every delete above.
    let read = get_application(&emulator, &held).await;
    assert_eq!(
        read.status, 200,
        "case held-ui: the held application still resolves, got {} with {:?}",
        read.status, read.body
    );
    let status = string_member(&read.body, "status", "held-ui");
    assert_ne!(
        status, APPLICATION_STATUS_DELETING,
        "case held-ui: a delete of another id does not start this resource's removal, got \
         {status:?}"
    );
}

/// RULE: a second delete asks for the removal the clock already owes, so it answers as
/// the first did and the resource is gone once that removal falls due.
#[tokio::test]
async fn deleting_twice_while_the_removal_is_in_flight_is_the_same_delete() {
    let emulator = Emulator::new();

    // Two rows, differing in what the record holds, so what is asserted is the repeat
    // rather than one application.
    let rows = [
        ("a bare application", json!({ "name": "twice-bare" })),
        (
            "an application carrying tags and a data source",
            json!({
                "name": "twice-loaded",
                "tagList": [{ "Key": "env", "Value": "prod" }],
                "dataSources": [{ "dataSourceArn": "arn:aws:s3:::twice-over-catalog" }],
            }),
        ),
    ];

    let mut ids: Vec<(&str, String)> = Vec::new();
    for (case, request) in rows {
        let created = create_application(&emulator, request).await;
        let id = string_member(&created.body, "id", case);

        for attempt in 1..=3 {
            let dropped = delete_application(&emulator, &id).await;
            assert_eq!(
                dropped.status, 200,
                "case {case} (delete {attempt}): the removal already owed is answered the same \
                 way, got {} with {:?}",
                dropped.status, dropped.body
            );
            let read = get_application(&emulator, &id).await;
            assert_eq!(
                string_member(&read.body, "status", case),
                APPLICATION_STATUS_DELETING,
                "case {case} (delete {attempt}): the record still rests in its deleting status, \
                 got {:?}",
                read.body
            );
        }

        ids.push((case, id));
    }

    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, id) in &ids {
        assert_gone(&emulator, id, case).await;

        // A delete of an application the clock has removed asks for the state the
        // account is already in, so it is answered as the first delete was.
        let dropped = delete_application(&emulator, id).await;
        assert_eq!(
            dropped.status, 200,
            "case {case}: a delete after the removal landed is answered, got {} with {:?}",
            dropped.status, dropped.body
        );
    }
}

/// One row of [`a_removed_application_releases_its_name_and_its_tags`]: the create the
/// application is made with, and the tags a later `AddTags` puts on it.
struct ReleaseRow {
    case: &'static str,
    request: Value,
    added: Vec<(String, String)>,
}

/// RULE: what the record held goes with the record: the name it occupied can be used
/// again, and its tag set is no longer a tag set of this account.
#[tokio::test]
async fn a_removed_application_releases_its_name_and_its_tags() {
    let emulator = Emulator::new();

    // Rows differ in the name's shape and in whether the create or a later `AddTags`
    // put the tags on the record: the release is the same either way.
    let rows = vec![
        ReleaseRow {
            case: "tags named by the create",
            request: json!({
                "name": "reuse-created",
                "tagList": [{ "Key": "env", "Value": "prod" }],
            }),
            added: vec![],
        },
        ReleaseRow {
            case: "tags added after the create",
            request: json!({ "name": "reuse-added" }),
            added: vec![tag("owner", "search"), tag("stage", "beta")],
        },
        ReleaseRow {
            case: "a 3-character name at the lower bound, with no tags at all",
            request: json!({ "name": "rsu" }),
            added: vec![],
        },
    ];

    for ReleaseRow {
        case,
        request,
        added,
    } in rows
    {
        let name = request["name"].as_str().unwrap_or_default().to_string();
        let created = create_application(&emulator, request.clone()).await;
        let id = string_member(&created.body, "id", case);
        let arn = string_member(&created.body, "arn", case);

        if !added.is_empty() {
            let written = add_tags(&emulator, &arn, wire_tags(&added)).await;
            assert_eq!(
                written.status, 200,
                "case {case}: the tags are written, got {} with {:?}",
                written.status, written.body
            );
            let read = list_tags(&emulator, &arn).await;
            assert_eq!(
                reported_tags(&read, case),
                added,
                "case {case}: the tags are the set just written, got {:?}",
                read.body
            );
        }

        // The name is this account's for as long as it holds the application: a second
        // create of it is the modelled conflict rather than a second record.
        let clash = create_application(&emulator, request).await;
        assert_eq!(
            error_code(&clash),
            "ConflictException",
            "case {case}: the name is taken while the application is held, got {:?}",
            clash.body
        );

        assert_eq!(
            delete_application(&emulator, &id).await.status,
            200,
            "case {case}: the delete is served"
        );
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_gone(&emulator, &id, case).await;

        // The tag set went with the record, so its ARN names no resource of this account
        // for the tagging paths either.
        let orphaned = list_tags(&emulator, &arn).await;
        assert_eq!(
            orphaned.status, 400,
            "case {case}: the ARN of a removed application resolves for nobody, got {} with {:?}",
            orphaned.status, orphaned.body
        );
        assert_eq!(
            error_code(&orphaned),
            "ValidationException",
            "case {case}: the tagging paths refuse an unresolvable ARN, got {:?}",
            orphaned.body
        );

        // The name the removed application held is free, and the create that takes it is
        // a NEW application rather than the one that was deleted.
        let again = create_application(&emulator, json!({ "name": name })).await;
        assert_eq!(
            again.status, 200,
            "case {case}: the name a removed application held can be used again, got {} with {:?}",
            again.status, again.body
        );
        let fresh = string_member(&again.body, "id", case);
        assert_ne!(
            fresh, id,
            "case {case}: the create mints a new application rather than reviving one"
        );
        let read = list_tags(&emulator, &string_member(&again.body, "arn", case)).await;
        assert!(
            reported_tags(&read, case).is_empty(),
            "case {case}: the new application carries none of the removed one's tags, got {:?}",
            read.body
        );
    }
}

/// RULE: the bound this account's store is kept under counts the records it holds, so a
/// record the clock has removed no longer counts against it.
#[tokio::test]
async fn a_removed_application_frees_the_slot_it_took_in_the_accounts_bound() {
    let emulator = Emulator::new();

    let mut ids = Vec::new();
    for index in 0..APPLICATION_QUOTA {
        let created = create_application(&emulator, json!({ "name": format!("bound-ui{index}") }))
            .await;
        assert_eq!(
            created.status, 200,
            "case fill {index}: expected 200, got {} with {:?}",
            created.status, created.body
        );
        ids.push(string_member(&created.body, "id", "fill"));
    }

    let refused = create_application(&emulator, json!({ "name": "one-too-many" })).await;
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case one-too-many: a full account refuses another create, got {:?}",
        refused.body
    );

    // One record removed frees exactly one slot: the create refused above is served
    // once the removal has fallen due, and the next one is refused again.
    assert_eq!(
        delete_application(&emulator, &ids[0]).await.status,
        200,
        "case free-a-slot: the delete is served"
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let served = create_application(&emulator, json!({ "name": "one-too-many" })).await;
    assert_eq!(
        served.status, 200,
        "case one-too-many: the slot the removed application took is free, got {} with {:?}",
        served.status, served.body
    );
    let next = create_application(&emulator, json!({ "name": "two-too-many" })).await;
    assert_eq!(
        error_code(&next),
        "ValidationException",
        "case two-too-many: one removal frees one slot and no more, got {:?}",
        next.body
    );
}
