// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateApplication`: what a change to an OpenSearch UI
//! application applies, and what it leaves exactly as it was.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds only the three application calls it
//! drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an update applies the members the request CARRIES and leaves every member it
//!   leaves out at the value the application already held, over a table of updates
//!   differing in which members they name, how many entries each list carries
//!   (none, one, several, an explicitly empty one, the published upper bound), which
//!   configuration keys they use, and what the application was created with;
//! * what an update answers with is what a later `GetApplication` reports, and the
//!   id, name, ARN and creation instant of the application never move;
//! * successive updates accumulate on the ONE stored record, and the instant it was
//!   last changed moves forward with each of them;
//! * an update changes the application's settings and NOT its lifecycle: the status
//!   move the clock owed a just-created application is still owed after it, and still
//!   falls due on its own;
//! * a list of configuration settings past the bound the member publishes is refused
//!   with the modelled `ValidationException` and changes nothing;
//! * an id that names no application of this account is refused with the modelled
//!   `ResourceNotFoundException`, and no record is created by the refusal.

use serde_json::{json, Map, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    APPLICATION_DOES_NOT_EXIST, APPLICATION_STATUS_ACTIVE, APPLICATION_STATUS_CREATING,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The two values `AppConfigType` declares
/// (`smithy_query(query="shape",shape="AppConfigType")`), so the table below changes a
/// configuration key the issue did not cite as well as one it did.
const APP_CONFIG_USERS: &str = "opensearchDashboards.dashboardAdmin.users";
const APP_CONFIG_GROUPS: &str = "opensearchDashboards.dashboardAdmin.groups";

/// `AppConfigs` is `@length(min: 0, max: 200)`
/// (`smithy_query(query="operation",operation="UpdateApplication")`), so this is the
/// largest list of configuration settings an update may carry.
const APP_CONFIGS_MAX: usize = 200;

/// An IAM Identity Center instance ARN, as the create and the updates name one.
const SSO_INSTANCE: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// The members of `IamIdentityCenterOptions` a response reports, so a case compares
/// every one of them rather than the one it changed.
const IDENTITY_CENTER_MEMBERS: [&str; 4] = [
    "enabled",
    "iamIdentityCenterInstanceArn",
    "iamRoleForIdentityCenterApplicationArn",
    "iamIdentityCenterApplicationArn",
];

/// `CreateApplication`: `POST /2021-01-01/opensearch/application`.
///
/// Reached by a sibling suite as `super::application_updates::create_application`, so
/// the crate grows no second copy of this route.
pub(crate) async fn create_application(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(body),
        )
        .await
}

/// `UpdateApplication`: `PUT /2021-01-01/opensearch/application/{id}`. Reached by a
/// sibling suite on the same rule as [`create_application`].
pub(crate) async fn update_application(emulator: &Emulator, id: &str, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/application/{id}"),
            Some(body),
        )
        .await
}

/// `GetApplication`: `GET /2021-01-01/opensearch/application/{id}`. Reached by a
/// sibling suite on the same rule as [`create_application`].
pub(crate) async fn get_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await
}

/// What a response reports for a LIST member a request carried: the list itself when
/// it had entries, and nothing at all when the request named an empty one — a member
/// the application holds nothing for is left out rather than answered with an empty
/// list.
fn reported_list(carried: Option<&Value>) -> Value {
    match carried {
        Some(Value::Array(entries)) if !entries.is_empty() => Value::Array(entries.clone()),
        _ => Value::Null,
    }
}

/// The value a response reports for list `member` once `update` has been applied to an
/// application created with `create`: the update's list where it carried one, and the
/// create's where it did not.
fn expected_list(create: &Value, update: &Value, member: &str) -> Value {
    reported_list(update.get(member).or_else(|| create.get(member)))
}

/// What a READ of the whole stored record reports for a list member whose target lets
/// it hold zero entries (`AppConfigs` is `@length(min: 0, max: 200)`,
/// `smithy_query(query="shape",shape="AppConfigs")`): the list the application holds,
/// and the EMPTY list where it holds none.
///
/// Reached by a sibling suite as `super::application_updates::read_list`, so the crate
/// holds ONE statement of what such a member reads back as.
pub(crate) fn read_list(carried: Option<&Value>) -> Value {
    match carried {
        Some(Value::Array(entries)) => Value::Array(entries.clone()),
        _ => Value::Array(Vec::new()),
    }
}

/// The Identity Center settings a response reports once `update` has been applied to an
/// application created with `create`.
///
/// The same rule one level down: a member the update carries replaces the stored one
/// and a member it leaves out keeps it. The Identity Center application's own ARN is
/// minted by that service, so it is reported by neither.
fn expected_identity_center(create: &Value, update: &Value) -> Value {
    let stored = create.get("iamIdentityCenterOptions");
    let asked = update.get("iamIdentityCenterOptions");
    if stored.is_none() && asked.is_none() {
        return Value::Null;
    }
    let mut merged = Map::new();
    for source in [stored, asked].into_iter().flatten() {
        for (member, value) in source.as_object().cloned().unwrap_or_default() {
            merged.insert(member, value);
        }
    }
    Value::Object(merged)
}

/// A create and the update applied to it, as one row of the table below.
struct Row {
    case: &'static str,
    create: Value,
    update: Value,
}

/// A list of `count` configuration settings, alternating between the two keys
/// `AppConfigType` declares, so a row can carry a list of any size the member allows.
///
/// Reached by a sibling suite as `super::application_updates::app_configs`, so the
/// crate holds ONE generator of this model-bounded list.
pub(crate) fn app_configs(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "key": if index % 2 == 0 { APP_CONFIG_USERS } else { APP_CONFIG_GROUPS },
                    "value": format!("arn:aws:iam::123456789012:user/user-{index}"),
                })
            })
            .collect(),
    )
}

/// The table every row of [`an_update_applies_the_members_it_carries`] drives: one row
/// per way an update can differ — which members it names, the sizes of the lists it
/// names (including the published upper bound and an explicitly empty one), the
/// configuration keys it uses, and whether the application it changes was created with
/// those members at all.
fn updates() -> Vec<Row> {
    let baseline = json!({
        "dataSources": [{
            "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/logs",
            "dataSourceDescription": "the logs domain",
        }],
        "appConfigs": [{ "key": APP_CONFIG_USERS, "value": "arn:aws:iam::123456789012:user/ana" }],
        "iamIdentityCenterOptions": {
            "enabled": true,
            "iamIdentityCenterInstanceArn": SSO_INSTANCE,
            "iamRoleForIdentityCenterApplicationArn":
                "arn:aws:iam::123456789012:role/identity-center",
        },
    });
    let bare = json!({});

    vec![
        Row {
            case: "an update naming no member at all changes nothing",
            create: baseline.clone(),
            update: json!({}),
        },
        Row {
            case: "one configuration setting, the data sources left alone",
            create: baseline.clone(),
            update: json!({
                "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "operators" }],
            }),
        },
        Row {
            case: "three data sources, the configuration settings left alone",
            create: baseline.clone(),
            update: json!({
                "dataSources": [
                    { "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/metrics" },
                    {
                        "dataSourceArn": "arn:aws:s3:::my-catalog",
                        "dataSourceDescription": "an S3 Glue catalog",
                        "iamRoleForDataSourceArn":
                            "arn:aws:iam::123456789012:role/data-source-reader",
                    },
                    { "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/traces" },
                ],
            }),
        },
        Row {
            case: "an explicitly empty appConfigs list leaves the application holding none",
            create: baseline.clone(),
            update: json!({ "appConfigs": [] }),
        },
        Row {
            case: "an explicitly empty dataSources list leaves the application holding none",
            create: baseline.clone(),
            update: json!({ "dataSources": [] }),
        },
        Row {
            case: "both lists at once",
            create: baseline.clone(),
            update: json!({
                "dataSources": [{ "dataSourceArn": "arn:aws:s3:::bucket-both" }],
                "appConfigs": [
                    { "key": APP_CONFIG_USERS, "value": "arn:aws:iam::123456789012:user/bo" },
                    { "key": APP_CONFIG_GROUPS, "value": "everyone" },
                ],
            }),
        },
        Row {
            case: "the published upper bound of configuration settings",
            create: baseline.clone(),
            update: json!({ "appConfigs": app_configs(APP_CONFIGS_MAX) }),
        },
        Row {
            case: "one Identity Center member, the other two left alone",
            create: baseline.clone(),
            update: json!({ "iamIdentityCenterOptions": { "enabled": false } }),
        },
        Row {
            case: "every Identity Center member at once",
            create: baseline.clone(),
            update: json!({
                "iamIdentityCenterOptions": {
                    "enabled": false,
                    "iamIdentityCenterInstanceArn": "arn:aws:sso:::instance/ssoins-fedcba0987654321",
                    "iamRoleForIdentityCenterApplicationArn":
                        "arn:aws:iam::123456789012:role/other-identity-center",
                },
            }),
        },
        Row {
            case: "lists added to an application created with none",
            create: bare.clone(),
            update: json!({
                "dataSources": [{ "dataSourceArn": "arn:aws:s3:::bucket-added" }],
                "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "added" }],
            }),
        },
        Row {
            case: "Identity Center asked for on an application created without it",
            create: bare.clone(),
            update: json!({
                "iamIdentityCenterOptions": {
                    "enabled": true,
                    "iamIdentityCenterInstanceArn": SSO_INSTANCE,
                },
            }),
        },
        Row {
            case: "an empty update of an application created with nothing but its name",
            create: bare,
            update: json!({}),
        },
    ]
}

#[tokio::test]
async fn an_update_applies_the_members_it_carries() {
    let emulator = Emulator::new();

    for (index, row) in updates().into_iter().enumerate() {
        let case = row.case;
        let mut create = row.create.clone();
        create["name"] = json!(format!("update-case-{index}"));

        let created = create_application(&emulator, create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from CreateApplication, got {} with {:?}",
            created.status, created.body
        );
        let id = string_member(&created.body, "id", case);

        let updated = update_application(&emulator, &id, row.update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateApplication, got {} with {:?}",
            updated.status, updated.body
        );

        // The application the update answers about is the one it was called on: the
        // identity of the record is not the update's to change.
        for member in ["id", "name", "arn", "createdAt"] {
            assert_eq!(
                updated.body[member], created.body[member],
                "case {case}: the update reports the {member} the create persisted, got {:?}",
                updated.body
            );
        }

        // The members the update carried are in place, and the ones it left out still
        // hold what the create persisted.
        for member in ["dataSources", "appConfigs"] {
            assert_eq!(
                updated.body[member],
                expected_list(&create, &row.update, member),
                "case {case}: the update reports the {member} it left in place, got {:?}",
                updated.body
            );
        }
        let identity_center = expected_identity_center(&create, &row.update);
        if identity_center.is_null() {
            assert_eq!(
                updated.body["iamIdentityCenterOptions"],
                Value::Null,
                "case {case}: an application nothing asked Identity Center for reports none, \
                 got {:?}",
                updated.body
            );
        } else {
            for member in IDENTITY_CENTER_MEMBERS {
                assert_eq!(
                    updated.body["iamIdentityCenterOptions"][member], identity_center[member],
                    "case {case}: the update reports the Identity Center {member} it left in \
                     place, got {:?}",
                    updated.body
                );
            }
        }

        // When the application was last changed is the instant the update changed it,
        // which is never before it was created.
        let created_at = created.body["createdAt"].as_f64().unwrap_or_default();
        let last_updated_at = updated.body["lastUpdatedAt"].as_f64();
        assert!(
            last_updated_at.is_some_and(|instant| instant >= created_at),
            "case {case}: the update reports when it changed the application, got {:?}",
            updated.body
        );

        // The read of the id reports the SAME record the update answered with: one
        // application, not the update's answer beside a store that disagrees with it.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from GetApplication, got {} with {:?}",
            read.status, read.body
        );
        for member in [
            "id",
            "name",
            "arn",
            "dataSources",
            "iamIdentityCenterOptions",
            "createdAt",
            "lastUpdatedAt",
        ] {
            assert_eq!(
                read.body[member], updated.body[member],
                "case {case}: the read reports the {member} the update persisted, got {:?}",
                read.body
            );
        }
        // The read reports the whole stored record, so the configuration settings come
        // back as the list the application holds — `[]` where it holds none.
        assert_eq!(
            read.body["appConfigs"],
            read_list(row.update.get("appConfigs").or_else(|| create.get("appConfigs"))),
            "case {case}: the read reports the configuration settings the update persisted, got \
             {:?}",
            read.body
        );

        // An update changes the application's settings, not where it is in its
        // lifecycle: the status is still the one of a just-created application.
        let status = string_member(&read.body, "status", case);
        assert!(
            status == APPLICATION_STATUS_CREATING || status == APPLICATION_STATUS_ACTIVE,
            "case {case}: the update left the application's lifecycle alone, got {status:?}"
        );
    }
}

#[tokio::test]
async fn successive_updates_accumulate_on_the_one_stored_record() {
    let emulator = Emulator::new();

    let created = create_application(&emulator, json!({ "name": "accumulating-ui" })).await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200 from CreateApplication, got {} with {:?}",
        created.status, created.body
    );
    let id = string_member(&created.body, "id", "create");

    // Each update carries ONE member, so what the next one reads back is what the
    // previous one left: the three of them build one record rather than three.
    let steps = vec![
        (
            "the data sources first",
            json!({ "dataSources": [{ "dataSourceArn": "arn:aws:s3:::bucket-first" }] }),
        ),
        (
            "the configuration settings second",
            json!({ "appConfigs": [{ "key": APP_CONFIG_USERS, "value": "arn:aws:iam::123456789012:user/cleo" }] }),
        ),
        (
            "Identity Center third",
            json!({
                "iamIdentityCenterOptions": {
                    "enabled": true,
                    "iamIdentityCenterInstanceArn": SSO_INSTANCE,
                },
            }),
        ),
    ];

    let mut expected = json!({});
    let mut previous = created.body["createdAt"].as_f64().unwrap_or_default();
    for (case, step) in steps {
        // The instant the record was last changed is read at a different clock reading
        // for each step, so the move of that instant is observable.
        tokio::time::sleep(SETTLE_WAIT).await;

        let updated = update_application(&emulator, &id, step.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateApplication, got {} with {:?}",
            updated.status, updated.body
        );
        for (member, value) in step.as_object().cloned().unwrap_or_default() {
            expected[member] = value;
        }

        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.body["dataSources"],
            reported_list(expected.get("dataSources")),
            "case {case}: the application still holds the dataSources an earlier update \
             left, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["appConfigs"],
            read_list(expected.get("appConfigs")),
            "case {case}: the application still holds the appConfigs an earlier update \
             left, got {:?}",
            read.body
        );
        for member in IDENTITY_CENTER_MEMBERS {
            assert_eq!(
                read.body["iamIdentityCenterOptions"][member],
                expected["iamIdentityCenterOptions"][member],
                "case {case}: the application still holds the Identity Center {member} an \
                 earlier update left, got {:?}",
                read.body
            );
        }

        let last_updated_at = read.body["lastUpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            last_updated_at > previous,
            "case {case}: the instant the application was last changed moved forward, got \
             {last_updated_at:?} after {previous:?}"
        );
        assert_eq!(
            read.body["createdAt"], created.body["createdAt"],
            "case {case}: no update moves the instant the application was created, got {:?}",
            read.body
        );
        previous = last_updated_at;
    }
}

#[tokio::test]
async fn an_update_leaves_the_status_the_clock_owes_in_place() {
    let emulator = Emulator::new();

    // One row per kind of change, so what is asserted is that NO update touches the
    // lifecycle rather than that one of them does not.
    let rows = vec![
        ("no member at all", json!({})),
        (
            "the data sources",
            json!({ "dataSources": [{ "dataSourceArn": "arn:aws:s3:::settling" }] }),
        ),
        (
            "the configuration settings",
            json!({ "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "settling" }] }),
        ),
        (
            "Identity Center",
            json!({ "iamIdentityCenterOptions": { "enabled": false } }),
        ),
    ];

    let mut ids = Vec::new();
    for (index, (case, update)) in rows.into_iter().enumerate() {
        let created =
            create_application(&emulator, json!({ "name": format!("settling-{index}") })).await;
        let id = string_member(&created.body, "id", case);

        // The update happens well inside the provisioning span, so what the read that
        // follows it reports is the status the create minted the application in: an
        // update that moved the status, or dropped the move the clock owed, fails here.
        let updated = update_application(&emulator, &id, update).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: an update of an application being provisioned is served, got {} \
             with {:?}",
            updated.status, updated.body
        );
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.body["status"],
            json!(APPLICATION_STATUS_CREATING),
            "case {case}: the update left the application being provisioned, got {:?}",
            read.body
        );
        ids.push((case, id));
    }

    // Nothing below asks for the move: the span each application was minted with falls
    // due on its own, and the next read materialises it — the update neither brought it
    // forward nor cancelled it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, id) in &ids {
        let read = get_application(&emulator, id).await;
        assert_eq!(
            read.body["status"],
            json!(APPLICATION_STATUS_ACTIVE),
            "case {case}: the status the clock owed is in place once the span passed, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_list_of_configuration_settings_past_the_published_bound_is_refused() {
    let emulator = Emulator::new();

    let created = create_application(&emulator, json!({ "name": "config-bound-ui" })).await;
    assert_eq!(
        created.status, 200,
        "case create: expected 200 from CreateApplication, got {} with {:?}",
        created.status, created.body
    );
    let id = string_member(&created.body, "id", "create");

    // Each published bound of `AppConfigs` and one past it: the empty list and the
    // largest list the member allows are served, and one entry more is refused.
    let mut served = Value::Null;
    for count in [0, 1, APP_CONFIGS_MAX, APP_CONFIGS_MAX + 1] {
        let asked = app_configs(count);
        let answer = update_application(&emulator, &id, json!({ "appConfigs": asked })).await;

        if count <= APP_CONFIGS_MAX {
            assert_eq!(
                answer.status, 200,
                "case {count} settings: a list the member allows is served, got {} with {:?}",
                answer.status, answer.body
            );
            served = answer.body["appConfigs"].clone();
            assert_eq!(
                served,
                reported_list(Some(&app_configs(count))),
                "case {count} settings: the update reports the settings it was given, got {:?}",
                answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {count} settings: a list past the bound is refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {count} settings: the refusal is the modelled client-input error, got {:?}",
            answer.body
        );

        // The refused update changed nothing: the application still holds the settings
        // the last served one left.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.body["appConfigs"], served,
            "case {count} settings: the refused update left the settings alone, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_update_of_an_id_that_names_no_application_is_refused() {
    let emulator = Emulator::new();

    // The account holds one application, so what each row below exercises is the id
    // rather than an empty store.
    let created = create_application(
        &emulator,
        json!({
            "name": "held-by-updates",
            "appConfigs": [{ "key": APP_CONFIG_USERS, "value": "arn:aws:iam::123456789012:user/dee" }],
        }),
    )
    .await;
    let held = string_member(&created.body, "id", "held");

    // One row per shape of well-formed id the account holds nothing under: the shortest
    // the pattern allows, the longest, a digits-only one, and the held id with one
    // character changed.
    let mut absent = vec![
        "abc".to_string(),
        "a".repeat(30),
        "0123456789".to_string(),
        format!("{}z", &held[..held.len() - 1]),
    ];
    absent.retain(|id| *id != held);

    for id in absent {
        let refused = update_application(
            &emulator,
            &id,
            json!({ "appConfigs": [{ "key": APP_CONFIG_GROUPS, "value": "nobody" }] }),
        )
        .await;
        assert_eq!(
            refused.status, RECORDED_NOT_FOUND_STATUS,
            "case {id}: an id naming no application is refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {id}: the refusal is the modelled not-found, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["message"],
            json!(APPLICATION_DOES_NOT_EXIST),
            "case {id}: the refusal states that the account holds no such application, got {:?}",
            refused.body
        );

        // The refused update created nothing: the id it named still resolves to no
        // application at all.
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case {id}: a refused update mints no application, got {} with {:?}",
            read.status, read.body
        );
    }

    // The application the account does hold is untouched by every refusal above.
    let read = get_application(&emulator, &held).await;
    assert_eq!(
        read.body["appConfigs"], created.body["appConfigs"],
        "case held: the refused updates changed nothing about the held application, got {:?}",
        read.body
    );
}
