// SPDX-License-Identifier: CC0-1.0

//! `UpdateDataSource`: the write that replaces a stored data source's configuration.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: an update addressed to `(AwsAccountId, DataSourceId)` replaces
//! the stored resource's mutable configuration with exactly what the request carries —
//! a member the request omits is cleared — refreshes `LastUpdatedTime`, leaves the
//! resource's identity (ARN, id, `Type`, `CreatedTime`) and its permissions and tags
//! alone, is accepted as asynchronous work (HTTP 202 and the in-progress update status,
//! with the terminal one reached by the clock and reported by a later read), and is
//! refused as not found when the account holds no data source with that id. So the rows
//! below vary what the rule generalises over — which optional members the create carried
//! and which the update carries (replaced, added, cleared), the source kind, the name's
//! length at both modelled bounds, and the account — rather than re-running one call.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";
/// A password submitted in `Credentials`, asserted never to come back out.
const SECRET_PASSWORD: &str = "pw-never-echoed-17";
/// The `ResourceStatus` member a succeeded update leaves behind (model enum
/// `com.amazonaws.quicksight#ResourceStatus`).
const UPDATE_SUCCESSFUL: &str = "UPDATE_SUCCESSFUL";
/// The `ResourceStatus` an accepted update reports until the clock settles it.
const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
/// The `ResourceStatus` a settled create leaves behind — what the status must no longer
/// be once an update has been applied.
const CREATION_SUCCESSFUL: &str = "CREATION_SUCCESSFUL";

fn data_source_arn(account: &str, data_source_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:datasource/{data_source_id}")
}

async fn create_data_source(harness: &Harness, account: &str, body: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(body),
        )
        .await
}

async fn update_data_source(
    harness: &Harness,
    account: &str,
    id: &str,
    body: Value,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/accounts/{account}/data-sources/{id}"),
            Some(body),
        )
        .await
}

async fn describe_data_source(harness: &Harness, account: &str, id: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/accounts/{account}/data-sources/{id}"), None)
        .await
}

/// A `Credentials` structure carrying [`SECRET_PASSWORD`].
fn credentials() -> Value {
    json!({ "CredentialPair": { "Username": "analyst", "Password": SECRET_PASSWORD } })
}

/// One row of the table: the body the data source is created with and the body the
/// update sends. Both are whole request bodies, so a row says exactly which optional
/// members each call carries.
struct Case {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    account: &'static str,
    id: &'static str,
    create: Value,
    update: Value,
}

/// The table. Each row differs from the others in which optional members survive,
/// arrive or disappear across the update, in the source kind (members of the model's
/// `DataSourceType` enum, with parameters from the matching member of the
/// `DataSourceParameters` union), in the length of `Name` (the model constrains it to
/// 1..128), and in the account the resource belongs to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "rename only, no optional members on either call",
            account: ACCOUNT,
            id: "ds-rename",
            create: json!({ "DataSourceId": "ds-rename", "Name": "before", "Type": "ATHENA" }),
            update: json!({ "Name": "after" }),
        },
        Case {
            label: "the connection parameters are replaced",
            account: ACCOUNT,
            id: "ds-params",
            create: json!({
                "DataSourceId": "ds-params",
                "Name": "lake",
                "Type": "ATHENA",
                "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "primary" } },
            }),
            update: json!({
                "Name": "lake",
                "DataSourceParameters": { "AthenaParameters": { "WorkGroup": "reporting" } },
            }),
        },
        Case {
            label: "SSL properties flip and a VPC connection arrives",
            account: ACCOUNT,
            id: "ds-vpc",
            create: json!({
                "DataSourceId": "ds-vpc",
                "Name": "warehouse",
                "Type": "POSTGRESQL",
                "DataSourceParameters": {
                    "PostgreSqlParameters": { "Host": "db.internal", "Port": 5432, "Database": "sales" }
                },
                "SslProperties": { "DisableSsl": true },
            }),
            update: json!({
                "Name": "warehouse",
                "DataSourceParameters": {
                    "PostgreSqlParameters": { "Host": "db2.internal", "Port": 5433, "Database": "sales" }
                },
                "SslProperties": { "DisableSsl": false },
                "VpcConnectionProperties": {
                    "VpcConnectionArn": "arn:aws:quicksight:us-west-2:000000000000:vpcConnection/vpc-1"
                },
            }),
        },
        Case {
            label: "every optional member the create carried is omitted, so cleared",
            account: ACCOUNT,
            id: "ds-cleared",
            create: json!({
                "DataSourceId": "ds-cleared",
                "Name": "manifest",
                "Type": "S3",
                "DataSourceParameters": {
                    "S3Parameters": {
                        "ManifestFileLocation": { "Bucket": "bucket-1", "Key": "manifest.json" }
                    }
                },
                "SslProperties": { "DisableSsl": true },
                "VpcConnectionProperties": {
                    "VpcConnectionArn": "arn:aws:quicksight:us-west-2:000000000000:vpcConnection/vpc-9"
                },
            }),
            update: json!({ "Name": "manifest" }),
        },
        Case {
            label: "credentials are submitted alongside the new parameters",
            account: ACCOUNT,
            id: "ds-credentials",
            create: json!({
                "DataSourceId": "ds-credentials",
                "Name": "mysql",
                "Type": "MYSQL",
                "DataSourceParameters": {
                    "MySqlParameters": { "Host": "mysql.internal", "Port": 3306, "Database": "app" }
                },
                "Credentials": credentials(),
            }),
            update: json!({
                "Name": "mysql-renamed",
                "DataSourceParameters": {
                    "MySqlParameters": { "Host": "mysql-2.internal", "Port": 3306, "Database": "app" }
                },
                "Credentials": credentials(),
            }),
        },
        Case {
            label: "a Name at the model's minimum length of 1",
            account: ACCOUNT,
            id: "ds-shortest-name",
            create: json!({ "DataSourceId": "ds-shortest-name", "Name": "n", "Type": "REDSHIFT" }),
            update: json!({ "Name": "a" }),
        },
        Case {
            label: "a Name at the model's maximum length of 128",
            account: ACCOUNT,
            id: "ds-longest-name",
            create: json!({ "DataSourceId": "ds-longest-name", "Name": "n", "Type": "SNOWFLAKE" }),
            update: json!({ "Name": "z".repeat(128) }),
        },
        Case {
            label: "the resource belongs to another account id",
            account: OTHER_ACCOUNT,
            id: "ds-other-account",
            create: json!({
                "DataSourceId": "ds-other-account",
                "Name": "elsewhere",
                "Type": "TERADATA",
                "SslProperties": { "DisableSsl": true },
            }),
            update: json!({
                "Name": "elsewhere-renamed",
                "SslProperties": { "DisableSsl": false },
            }),
        },
    ]
}

/// The value a member of the updated data source must have: whatever the update body
/// carried for it, and JSON null when the update omitted it.
fn expected(update: &Value, member: &str) -> Value {
    update.get(member).cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn an_update_replaces_the_stored_configuration_and_reads_back() {
    let harness = Harness::new();

    for case in cases() {
        let Case {
            label,
            account,
            id,
            create,
            update,
        } = case;

        let (status, body) = create_data_source(&harness, account, create.clone()).await;
        assert_eq!(
            status, 202,
            "case {label:?}: create expected 202, got {status} with {body}"
        );
        harness.settle_data_source(account, id);
        let (_, before) = describe_data_source(&harness, account, id).await;
        let created_time = before["DataSource"]["CreatedTime"].clone();
        assert_eq!(
            before["DataSource"]["Status"],
            json!(CREATION_SUCCESSFUL),
            "case {label:?}: the fixture must start in the create status, got {before}"
        );

        let (status, body) = update_data_source(&harness, account, id, update.clone()).await;
        assert_eq!(
            status, 202,
            "case {label:?}: update expected 202, got {status} with {body}"
        );
        assert_eq!(
            body["UpdateStatus"],
            json!(UPDATE_IN_PROGRESS),
            "case {label:?}: the update is accepted, so it expected UpdateStatus \
             {UPDATE_IN_PROGRESS}, got {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(data_source_arn(account, id)),
            "case {label:?}: update reported the wrong Arn: {body}"
        );
        assert_eq!(
            body["DataSourceId"],
            json!(id),
            "case {label:?}: update echoed the wrong id: {body}"
        );
        assert!(
            body["RequestId"].as_str().is_some_and(|value| !value.is_empty()),
            "case {label:?}: update expected a RequestId, got {body}"
        );
        // `Status` is `@httpResponseCode`-bound, so it is the HTTP status asserted
        // above rather than a body member.
        assert_eq!(
            body["Status"],
            Value::Null,
            "case {label:?}: Status is bound to the HTTP status, so it is not a body member: {body}"
        );
        let update_response = body.to_string();

        // The update is still in flight until the clock owes its completion, so a read
        // taken straight away reports the non-terminal status the update reported.
        let (_, in_flight) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            in_flight["DataSource"]["Status"],
            json!(UPDATE_IN_PROGRESS),
            "case {label:?}: before the span is over the update is still in \
             progress: {in_flight}"
        );
        harness.settle_data_source(account, id);

        let (status, body) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: describe after update expected 200, got {status} with {body}"
        );
        let source = &body["DataSource"];
        assert_eq!(
            source["Name"],
            expected(&update, "Name"),
            "case {label:?}: describe returned the wrong Name: {body}"
        );
        for member in [
            "DataSourceParameters",
            "SslProperties",
            "VpcConnectionProperties",
        ] {
            assert_eq!(
                source[member],
                expected(&update, member),
                "case {label:?}: expected {member} to be exactly what the update carried, got {body}"
            );
        }
        assert_eq!(
            source["Status"],
            json!(UPDATE_SUCCESSFUL),
            "case {label:?}: expected the stored status to be {UPDATE_SUCCESSFUL}, got {body}"
        );
        // Identity survives the update: the ARN, the id, the kind and the creation
        // instant are not inputs to it.
        assert_eq!(
            source["Arn"],
            json!(data_source_arn(account, id)),
            "case {label:?}: the update changed the Arn: {body}"
        );
        assert_eq!(
            source["DataSourceId"],
            json!(id),
            "case {label:?}: the update changed the id: {body}"
        );
        assert_eq!(
            source["Type"],
            expected(&create, "Type"),
            "case {label:?}: the update changed the Type: {body}"
        );
        assert_eq!(
            source["CreatedTime"], created_time,
            "case {label:?}: the update changed CreatedTime: {body}"
        );
        let last_updated = source["LastUpdatedTime"].as_f64().unwrap_or_else(|| {
            panic!("case {label:?}: describe returned no epoch LastUpdatedTime: {body}")
        });
        let created = created_time.as_f64().unwrap_or_else(|| {
            panic!("case {label:?}: describe returned no epoch CreatedTime: {body}")
        });
        assert!(
            last_updated >= created,
            "case {label:?}: expected LastUpdatedTime at or after CreatedTime, got {last_updated} and {created}"
        );

        // A submitted secret is never reported back by the write or by the read.
        let describe_response = body.to_string();
        for (which, rendered) in [("update", &update_response), ("describe", &describe_response)] {
            assert!(
                !rendered.contains(SECRET_PASSWORD),
                "case {label:?}: {which} echoed the submitted credentials: {rendered}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_keeps_the_permissions_and_tags_the_create_stored() {
    let harness = Harness::new();
    // Neither member is an input to UpdateDataSource — each has its own operation —
    // so an update must leave both exactly as the create left them.
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({
            "DataSourceId": "ds-tagged",
            "Name": "tagged",
            "Type": "ATHENA",
            "Permissions": [{
                "Principal": format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:user/default/analyst"),
                "Actions": ["quicksight:DescribeDataSource"],
            }],
            "Tags": [{ "Key": "team", "Value": "analytics" }],
        }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");

    let (status, body) = update_data_source(
        &harness,
        ACCOUNT,
        "ds-tagged",
        json!({ "Name": "tagged-renamed" }),
    )
    .await;
    assert_eq!(status, 202, "update: got {status} with {body}");

    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    let record = store
        .data_sources
        .get(&(ACCOUNT.to_string(), "ds-tagged".to_string()))
        .expect("the updated data source");
    assert_eq!(
        record.name, "tagged-renamed",
        "the update did not reach the store: {record:?}"
    );
    assert_eq!(
        record.permissions.len(),
        1,
        "the update dropped the stored permissions: {record:?}"
    );
    assert_eq!(
        record.tags.len(),
        1,
        "the update dropped the stored tags: {record:?}"
    );
    let rendered = format!("{record:?}");
    assert!(
        !rendered.contains(SECRET_PASSWORD),
        "the store kept the submitted credentials: {rendered}"
    );
}

#[tokio::test]
async fn an_update_of_a_data_source_the_account_does_not_hold_is_not_found() {
    let harness = Harness::new();
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({ "DataSourceId": "ds-present", "Name": "present", "Type": "ATHENA" }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");
    harness.settle_data_source(ACCOUNT, "ds-present");
    let (status, body) = harness
        .call("DELETE", &format!("/accounts/{ACCOUNT}/data-sources/ds-gone"), None)
        .await;
    assert_eq!(
        status, 404,
        "fixture: ds-gone was never created, got {status} with {body}"
    );

    // Rows: an id nothing created, an id that exists only under another account, and
    // an id whose create was never made. Each addresses no stored resource.
    let cases: [(&str, &str, &str); 3] = [
        ("an id nothing created", ACCOUNT, "ds-absent"),
        ("an id held by another account", OTHER_ACCOUNT, "ds-present"),
        ("an id with a different case", ACCOUNT, "DS-PRESENT"),
    ];
    for (label, account, id) in cases {
        let (status, body) =
            update_data_source(&harness, account, id, json!({ "Name": "whatever" })).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 for an unknown data source, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        let (status, body) = describe_data_source(&harness, account, id).await;
        assert_eq!(
            status, 404,
            "case {label:?}: a refused update must store nothing, got {status} with {body}"
        );
    }

    // The data source the refusals addressed by the wrong name is untouched.
    let (_, body) = describe_data_source(&harness, ACCOUNT, "ds-present").await;
    assert_eq!(
        body["DataSource"]["Name"],
        json!("present"),
        "a refused update renamed the stored data source: {body}"
    );
    assert_eq!(
        body["DataSource"]["Status"],
        json!(CREATION_SUCCESSFUL),
        "a refused update moved the stored status: {body}"
    );
}

#[tokio::test]
async fn a_name_outside_the_models_length_is_refused_and_changes_nothing() {
    let harness = Harness::new();
    let (status, body) = create_data_source(
        &harness,
        ACCOUNT,
        json!({ "DataSourceId": "ds-name-bounds", "Name": "original", "Type": "ATHENA" }),
    )
    .await;
    assert_eq!(status, 202, "fixture create: got {status} with {body}");

    // Rows: the model constrains `Name` to 1..128, so both published bounds are
    // accepted and the first length past each is refused — as is a missing required
    // member.
    let cases: [(&str, Value, u16); 5] = [
        ("length 1", json!({ "Name": "a" }), 202),
        ("length 128", json!({ "Name": "b".repeat(128) }), 202),
        ("length 129", json!({ "Name": "c".repeat(129) }), 400),
        ("length 0", json!({ "Name": "" }), 400),
        ("no Name at all", json!({}), 400),
    ];
    let mut last_accepted = "original".to_string();
    for (label, body, expected_status) in cases {
        let submitted = body["Name"].as_str().unwrap_or_default().to_string();
        let (status, response) =
            update_data_source(&harness, ACCOUNT, "ds-name-bounds", body).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status} with {response}"
        );
        if status == 202 {
            last_accepted = submitted;
        } else {
            assert_eq!(
                response["__type"],
                json!("ValidationException"),
                "case {label:?}: expected ValidationException, got {response}"
            );
        }
        let (_, described) = describe_data_source(&harness, ACCOUNT, "ds-name-bounds").await;
        assert_eq!(
            described["DataSource"]["Name"],
            json!(last_accepted),
            "case {label:?}: expected the last accepted Name to be the stored one, got {described}"
        );
    }
}
