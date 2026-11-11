// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY an OpenSearch UI application is addressed by — the
//! ARN a create mints for it and the id that ARN is built over — and for the refusal a
//! request naming an application this account does not hold earns.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the per-operation request helpers from the sibling suites; this one adds no
//! second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an application's ARN names the service token the OpenSearch user-interface plane
//!   is addressed under, the Region the request was signed for, this account and the
//!   resource type over the id the create minted — asserted over a table of creates
//!   differing in the shape and the length of the name (both published bounds) and in
//!   the Region they are signed for, with every reporting path (the create, the read and
//!   the listing) reporting the one ARN, and each create's id distinct and inside the
//!   modelled `Id` expression;
//! * that id is the application's own and not a reading of what the request asked for:
//!   it is distinct for every application a store mints, and so is the endpoint derived
//!   from it — asserted over a table of names, including two that differ only in their
//!   last character and one created again after the first was taken away;
//! * a DOMAIN of the same service keeps the token its own ARN carries, so the token is
//!   the resource kind's and not a rename of the service;
//! * the ARN a create answers with is a value the default-application setting takes and
//!   reports back, so the identity one call mints is the identity another call resolves;
//! * a start of a saved-object migration into an application this account does not hold
//!   states that the application does not exist, naming the id the request carried,
//!   under the status the service answers that miss with — asserted over a table of ids
//!   differing in length and alphabet, against a created application the same call
//!   serves.

use bytes::Bytes;
use serde_json::{json, Value};

use super::application_updates::{create_application, get_application};
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, DEFAULT_ACCESS_KEY_ID, PREFIX,
    RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::migration_listing::list_migrations;

/// The service token a DOMAIN's ARN carries — this service's `arnNamespace`, which the
/// application plane does not share.
const DOMAIN_ARN_TOKEN: &str = "es";

/// The service token an APPLICATION's ARN carries.
const APPLICATION_ARN_TOKEN: &str = "opensearch";

/// One create the identity rule is asserted over: the Region the request is signed for
/// and the name it asks for.
struct Row {
    case: &'static str,
    region: &'static str,
    name: &'static str,
}

/// The table every row of [`an_application_arn_names_the_application_plane`] drives.
///
/// The rows differ along what the rule generalises over: the Region the request is
/// signed for (three, one of them not the service's default) and the shape and length of
/// the name — the 3-character lower bound `ApplicationName` publishes, a hyphenated
/// name, a digit-carrying one and the 30-character upper bound.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the 3-character name bound, in the Region the harness signs for",
            region: "us-east-1",
            name: "ui1",
        },
        Row {
            case: "a hyphenated name in another Region",
            region: "eu-west-1",
            name: "identity-probe-ui",
        },
        Row {
            case: "a digit-carrying name in a third Region",
            region: "ap-southeast-2",
            name: "ui2x3y4",
        },
        Row {
            case: "the 30-character name bound",
            region: "us-west-2",
            name: "abcdefghij-klmnopqrst-uvwxyz12",
        },
    ]
}

/// `CreateApplication` signed for `region`, so the ARN the create reports is built for
/// the Region the request names rather than for the harness's default.
///
/// The harness's own `call` fixes the credential scope's Region, so this row builds the
/// request itself — the same bodiless-signature shape [`Emulator::call_as`] uses, with
/// the scope's Region as the only difference.
async fn create_in_region(emulator: &Emulator, region: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{PREFIX}/opensearch/application"))
        .header("host", "es.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={DEFAULT_ACCESS_KEY_ID}/20260101/{region}/es/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    emulator.dispatch(request).await
}

/// `ListApplications`: `GET /2021-01-01/opensearch/list-applications`.
async fn list_applications(emulator: &Emulator) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/list-applications"), None)
        .await
}

/// `PutDefaultApplicationSetting`: `PUT /2021-01-01/opensearch/defaultApplicationSetting`.
async fn put_default(emulator: &Emulator, application_arn: &str, set_as_default: bool) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            Some(json!({ "applicationArn": application_arn, "setAsDefault": set_as_default })),
        )
        .await
}

/// `GetDefaultApplicationSetting`: `GET /2021-01-01/opensearch/defaultApplicationSetting`.
async fn get_default(emulator: &Emulator) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            None,
        )
        .await
}

/// `StartMigration`: `POST /2021-01-01/opensearch/app-migrations` into `application_id`.
/// The source is the search-collection ARN this service validates a migration source
/// against, so a refusal under test is about the application and nothing else.
async fn start_migration(emulator: &Emulator, application_id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/app-migrations"),
            Some(json!({
                "applicationId": application_id,
                "migrationOptions": {
                    "source": {
                        "datasourceArn":
                            "arn:aws:aoss:us-east-1:123456789012:collection/identityprobe",
                    },
                    "workspace": { "workspaceId": "ws-identity" },
                },
            })),
        )
        .await
}

/// The error message of an answer.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Whether `id` is inside the expression the model declares for `Id`:
/// `^[a-z0-9]{3,30}$` (`smithy_query(query="shape",shape="Id")`).
fn is_application_id(id: &str) -> bool {
    (3..=30).contains(&id.chars().count())
        && id
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

#[tokio::test]
async fn an_application_arn_names_the_application_plane() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let mut minted: Vec<(String, String)> = Vec::new();

    for row in rows() {
        let case = row.case;
        let created = create_in_region(&emulator, row.region, json!({ "name": row.name })).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );

        let id = string_member(&created.body, "id", case);
        assert!(
            is_application_id(&id),
            "case {case}: expected an id inside the modelled expression, got {id:?}"
        );

        // The ARN the rule states: the application plane's token, the Region the request
        // was signed for, this account, and the resource type over the minted id.
        let expected = format!(
            "arn:aws:{APPLICATION_ARN_TOKEN}:{}:{account}:application/{id}",
            row.region
        );
        let reported = string_member(&created.body, "arn", case);
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected:?}, got {reported:?}"
        );

        // The id a create mints is unique per application, so no two rows can be
        // addressed by one ARN.
        for (held_id, held_arn) in &minted {
            assert_ne!(
                &id, held_id,
                "case {case}: expected an id no other application holds, got {id:?}"
            );
            assert_ne!(
                &reported, held_arn,
                "case {case}: expected an ARN no other application holds, got {reported:?}"
            );
        }

        // Every reporting path answers from the one stored record: the read of this
        // application reports the ARN the create answered with.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApplication, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            string_member(&read.body, "arn", case),
            expected,
            "case {case}: expected the read to report the ARN the create minted, got {:?}",
            read.body
        );

        minted.push((id, reported));
    }

    // And so does the listing, for every application the account holds.
    let listed = list_applications(&emulator).await;
    let summaries = listed.body["ApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an ApplicationSummaries list, got {:?}", listed.body))
        .clone();
    for (id, arn) in &minted {
        let summary = summaries
            .iter()
            .find(|summary| summary["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("expected the listing to report {id:?}, got {summaries:?}"));
        assert_eq!(
            summary["arn"].as_str(),
            Some(arn.as_str()),
            "case the listing of {id:?}: expected {arn:?}, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn an_id_is_the_applications_own_and_not_a_reading_of_its_name() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: two names that differ only
    // in their last character, and a name created TWICE — the second create made once
    // the first application's removal has landed, so the account holds one application
    // of that name and the id it was minted under is not the id the first one had.
    let mut seen: Vec<(String, String)> = Vec::new();
    for (case, name) in [
        ("a name of its own", "identity-mint-ui-a"),
        ("a name differing in one character", "identity-mint-ui-b"),
        ("the first name again, after the first application went", "identity-mint-ui-a"),
    ] {
        if seen.len() == 2 {
            // The name is free again only once the record has gone, which the delete
            // starts and the clock finishes.
            let (held, _) = &seen[0];
            let dropped = emulator
                .call(
                    "DELETE",
                    &format!("{PREFIX}/opensearch/application/{held}"),
                    None,
                )
                .await;
            assert_eq!(
                dropped.status, 200,
                "case {case}: expected 200 from DeleteApplication of a live application, got {} \
                 with {:?}",
                dropped.status, dropped.body
            );
            tokio::time::sleep(SETTLE_WAIT).await;
        }

        let created = create_application(&emulator, json!({ "name": name })).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);
        assert!(
            is_application_id(&id),
            "case {case}: expected an id inside the modelled expression, got {id:?}"
        );
        let read = get_application(&emulator, &id).await;
        let endpoint = string_member(&read.body, "endpoint", case);
        assert!(
            endpoint.contains(&id),
            "case {case}: expected the endpoint to be the one derived from the id {id:?}, got \
             {endpoint:?}"
        );

        for (held_id, held_endpoint) in &seen {
            assert_ne!(
                &id, held_id,
                "case {case}: expected an id no other application of this store holds, got {id:?}"
            );
            assert_ne!(
                &endpoint, held_endpoint,
                "case {case}: expected an endpoint of its own, got {endpoint:?}"
            );
        }
        seen.push((id, endpoint));
    }
}

#[tokio::test]
async fn a_domain_keeps_the_token_its_own_arn_carries() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // The two resource kinds of ONE service, created in one run: the token is the
    // resource kind's, so the application's ARN carrying another one is not a rename of
    // the service.
    for name in ["identity-token-one", "identity-token-two"] {
        let created = emulator.create(json!({ "DomainName": name })).await;
        assert_eq!(
            created.status, 200,
            "case CreateDomain({name:?}): expected 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_member(&created.body["DomainStatus"], "ARN", name);
        let expected = format!("arn:aws:{DOMAIN_ARN_TOKEN}:us-east-1:{account}:domain/{name}");
        assert_eq!(
            arn, expected,
            "case CreateDomain({name:?}): expected {expected:?}, got {arn:?}"
        );
    }

    let created = create_application(&emulator, json!({ "name": "identity-token-ui" })).await;
    let arn = string_member(&created.body, "arn", "identity-token-ui");
    assert!(
        arn.starts_with(&format!("arn:aws:{APPLICATION_ARN_TOKEN}:")),
        "case the application beside those domains: expected the application plane's token, \
         got {arn:?}"
    );
}

#[tokio::test]
async fn the_arn_a_create_mints_is_the_arn_the_default_setting_takes() {
    let emulator = Emulator::new();

    // One row per application created in this run, so the ARNs the setting is driven
    // with are the ones the service itself minted rather than values written out here.
    for name in ["default-setting-ui-one", "default-setting-ui-two"] {
        let created = create_application(&emulator, json!({ "name": name })).await;
        let arn = string_member(&created.body, "arn", name);

        // The write reports the application it acted on.
        let written = put_default(&emulator, &arn, true).await;
        assert_eq!(
            written.status, 200,
            "case PutDefaultApplicationSetting({name:?}): expected 200, got {} with {:?}",
            written.status, written.body
        );
        assert_eq!(
            string_member(&written.body, "applicationArn", name),
            arn,
            "case PutDefaultApplicationSetting({name:?}): expected the ARN the request named, \
             got {:?}",
            written.body
        );

        // And the setting holds it, read back through the operation that reports it.
        let read = get_default(&emulator).await;
        assert_eq!(
            string_member(&read.body, "applicationArn", name),
            arn,
            "case GetDefaultApplicationSetting after {name:?}: expected the written ARN, got {:?}",
            read.body
        );

        // Clearing names the ARN in effect and reports that same application, while the
        // setting it released resolves to no resource.
        let cleared = put_default(&emulator, &arn, false).await;
        assert_eq!(
            string_member(&cleared.body, "applicationArn", name),
            arn,
            "case the clear of {name:?}: expected the ARN the request named, got {:?}",
            cleared.body
        );
        let after = get_default(&emulator).await;
        assert_eq!(
            (after.status, error_code(&after).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case GetDefaultApplicationSetting after the clear of {name:?}: expected the \
             modelled not-found, got {} with {:?}",
            after.status,
            after.body
        );
    }
}

#[tokio::test]
async fn a_migration_into_an_application_this_account_does_not_hold_states_that_it_does_not_exist()
{
    let emulator = Emulator::new();

    // The application the account DOES hold, so every refusal below is about the id the
    // request carried rather than about an empty store — and so the same call is shown
    // to be served for an id that resolves.
    let created = create_application(&emulator, json!({ "name": "migration-identity-ui" })).await;
    let held = string_member(&created.body, "id", "the held application");
    let served = start_migration(&emulator, &held).await;
    assert_eq!(
        served.status, 200,
        "case StartMigration into the held application: expected 200, got {} with {:?}",
        served.status, served.body
    );

    // The rows differ along the length and the alphabet of the id — the shortest and the
    // longest the modelled `Id` admits, a digits-only one and a word-shaped one — so the
    // rule is asserted over the id rather than over one value.
    let absent = [
        "abc".to_string(),
        "a".repeat(30),
        "0123456789".to_string(),
        "nosuchapplicationhere".to_string(),
    ];

    for id in absent {
        assert_ne!(
            id, held,
            "case StartMigration({id:?}): the row must name another application"
        );
        let answer = start_migration(&emulator, &id).await;
        let expected = format!("Application {id} does not exist");
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case StartMigration({id:?}): expected the modelled not-found under 404, got {} with \
             {:?}",
            answer.status,
            answer.body
        );
        let stated = message(&answer);
        assert_eq!(
            stated, expected,
            "case StartMigration({id:?}): expected {expected:?}, got {stated:?}"
        );
    }

    // The refusals started nothing: the one job this account holds is the one the served
    // call started.
    let listed = list_migrations(&emulator, &held, "").await;
    assert_eq!(
        listed.body["migrations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case the listing after the refusals: expected the one started job, got {:?}",
        listed.body
    );
}
