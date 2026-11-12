// SPDX-License-Identifier: CC0-1.0

//! What a Clean Rooms ML training dataset's read-back reports for the dataset metadata
//! the create was given, and in which wire form its instants travel.
//!
//! Three rules are under test, each asserted over a TABLE whose rows differ along what
//! the rule generalises over:
//!
//! 1. The Glue data source a dataset is read from is reported with the catalog resolved:
//!    `GlueDataSource$catalogId` is optional on the wire, and the Glue Data Catalog a
//!    request leaves unnamed is the one of the account making the call — so the read-back
//!    names that account, and names the submitted catalog when the request gave one.
//! 2. A dataset's input `schema` is a column set keyed by column name, so it is reported
//!    in column-name order whatever order one request listed it in, with each entry's
//!    `columnTypes` still beside the name it was submitted with.
//! 3. Every timestamp member of this model declares `@timestampFormat("date-time")`, so
//!    each instant this service reports is an RFC 3339 date-time STRING rather than the
//!    `restJson1` default epoch-seconds number — on every operation, not only the one
//!    this suite's first two rules are about.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this crate drives the router through, defined once in
// the sibling suite rather than copied here.
use crate::training_dataset_audience_model_lifecycle::{
    is_timestamp, label, signed_call, timestamp_seconds, unix_now, Answer,
};

/// The region the requests in this suite are signed for.
const REGION: &str = "ap-south-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails. It is
/// also the Glue Data Catalog a request that names none is read from.
const ACCOUNT: &str = "000000000000";

/// An IAM role ARN matching `IamRoleArn`'s `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CleanRoomsMLRole";

/// Every value `ColumnType` accepts, in the order the model declares them, from
/// `smithy_query(query="shape", shape="ColumnType")`. The tables below use entries the
/// issue never cited, so no handler can special-case one spelling.
const COLUMN_TYPES: &[&str] = &[
    "USER_ID",
    "ITEM_ID",
    "TIMESTAMP",
    "CATEGORICAL_FEATURE",
    "NUMERICAL_FEATURE",
];

/// The only value `DatasetType` accepts, from the same query.
const DATASET_TYPE: &str = "INTERACTIONS";

/// The longest schema `DatasetSchemaList` admits (`@length(min: 1, max: 100)`), from
/// `smithy_query(query="shape", shape="DatasetSchemaList")`.
const MAX_SCHEMA_COLUMNS: usize = 100;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn create_dataset(router: &MultiServiceRouter, request: Value) -> Answer {
    signed_call(router, REGION, "POST", "/training-dataset", Some(request)).await
}

async fn get_dataset(router: &MultiServiceRouter, arn: &str) -> Answer {
    signed_call(
        router,
        REGION,
        "GET",
        &format!("/training-dataset/{}", label(arn)),
        None,
    )
    .await
}

async fn list_datasets(router: &MultiServiceRouter) -> Answer {
    signed_call(router, REGION, "GET", "/training-dataset", None).await
}

async fn create_model(router: &MultiServiceRouter, request: Value) -> Answer {
    signed_call(router, REGION, "POST", "/audience-model", Some(request)).await
}

async fn get_model(router: &MultiServiceRouter, arn: &str) -> Answer {
    signed_call(
        router,
        REGION,
        "GET",
        &format!("/audience-model/{}", label(arn)),
        None,
    )
    .await
}

/// One `ColumnSchema`: a column name and the one `ColumnType` beside it
/// (`ColumnTypeList` is bounded `@length(min: 1, max: 1)`).
fn column(name: &str, column_type: &str) -> Value {
    json!({ "columnName": name, "columnTypes": [column_type] })
}

/// One `Dataset` built from an already-ordered column list and an optional catalog —
/// the only member of the `trainingData` list, which
/// `CreateTrainingDatasetRequest$trainingData` bounds `@length(min: 1, max: 1)`.
fn training_data(table: &str, database: &str, catalog: Option<&str>, schema: Vec<Value>) -> Value {
    let mut glue = json!({ "tableName": table, "databaseName": database });
    if let Some(catalog) = catalog {
        glue["catalogId"] = json!(catalog);
    }
    json!({
        "type": DATASET_TYPE,
        "inputConfig": {
            "schema": schema,
            "dataSource": { "glueDataSource": glue },
        },
    })
}

/// The `trainingData` list a create request carries, for a schema given as
/// `(columnName, columnType)` pairs in submission order.
fn create_request(name: &str, catalog: Option<&str>, schema: &[(&str, &str)]) -> Value {
    let columns = schema
        .iter()
        .map(|(column_name, column_type)| column(column_name, column_type))
        .collect();
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "trainingData": [training_data("interactions", "clean_rooms_demo", catalog, columns)],
    })
}

/// The `trainingData` a read-back reports for one a create request carried: the same
/// datasets with the Glue catalog resolved to the calling account when the request named
/// none, and each `schema` in column-name order.
///
/// `pub(crate)` because the lifecycle suite compares its own create requests against the
/// same projection rather than restating it.
pub(crate) fn reported_training_data(submitted: &Value) -> Value {
    let datasets = submitted
        .as_array()
        .unwrap_or_else(|| panic!("trainingData is a list: {submitted:?}"))
        .iter()
        .map(|dataset| {
            let mut dataset = dataset.clone();
            let input_config = &mut dataset["inputConfig"];
            let glue = &mut input_config["dataSource"]["glueDataSource"];
            if glue.get("catalogId").is_none() {
                glue["catalogId"] = json!(ACCOUNT);
            }
            let schema = input_config["schema"]
                .as_array()
                .unwrap_or_else(|| panic!("inputConfig.schema is a list: {input_config:?}"));
            let mut ordered = schema.clone();
            ordered.sort_by(|left, right| {
                left["columnName"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["columnName"].as_str().unwrap_or_default())
            });
            input_config["schema"] = Value::Array(ordered);
            dataset
        })
        .collect();
    Value::Array(datasets)
}

/// The Glue data source the read-back `got` reports for its one dataset.
fn reported_glue(got: &Answer) -> Value {
    got.body
        .pointer("/trainingData/0/inputConfig/dataSource/glueDataSource")
        .cloned()
        .unwrap_or_else(|| panic!("the read-back carries a Glue data source: {:?}", got.body))
}

/// The `(columnName, columnType)` pairs the read-back `got` reports, in the order it
/// reports them.
fn reported_columns(got: &Answer) -> Vec<(String, String)> {
    got.body
        .pointer("/trainingData/0/inputConfig/schema")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the read-back carries a schema list: {:?}", got.body))
        .iter()
        .map(|entry| {
            let name = entry["columnName"].as_str().unwrap_or_default().to_string();
            let types = entry["columnTypes"]
                .as_array()
                .unwrap_or_else(|| panic!("columnTypes is a list: {entry:?}"));
            assert_eq!(
                types.len(),
                1,
                "ColumnTypeList is bounded 1..1, got {entry:?}"
            );
            (name, types[0].as_str().unwrap_or_default().to_string())
        })
        .collect()
}

/// Create `request` and read the dataset it minted back, asserting both answered 200.
async fn create_then_get(router: &MultiServiceRouter, case: &str, request: Value) -> Answer {
    let created = create_dataset(router, request).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateTrainingDataset, got {} with {:?}",
        created.status, created.body
    );
    let arn = created.body["trainingDatasetArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateTrainingDataset must report the ARN it minted, got {:?}",
                created.body
            )
        })
        .to_string();
    let got = get_dataset(router, &arn).await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: expected HTTP 200 from GetTrainingDataset, got {} with {:?}",
        got.status, got.body
    );
    got
}

/// RULE: the Glue data source a training dataset is read from is reported with its
/// catalog resolved — the catalog the create named, or the account making the call when
/// the create named none, because that account's catalog is the Glue Data Catalog an
/// unnamed one resolves to. `tableName` and `databaseName` are reported as submitted.
///
/// The rows differ along what the rule generalises over: whether the create named a
/// catalog at all, and which 12-digit `AccountId` it named (`@pattern ^[0-9]{12}$`,
/// `@length 12..12`) — this emulator's own account, another account, and the lowest and
/// highest 12-digit values that pattern admits.
#[tokio::test]
async fn a_dataset_reports_the_glue_catalog_its_data_is_read_from() {
    let cases: [(&str, Option<&str>, &str); 5] = [
        ("no catalog named", None, ACCOUNT),
        ("this account's own catalog", Some(ACCOUNT), ACCOUNT),
        (
            "a catalog in another account",
            Some("210987654321"),
            "210987654321",
        ),
        (
            "the lowest 12-digit account id",
            Some("000000000001"),
            "000000000001",
        ),
        (
            "the highest 12-digit account id",
            Some("999999999999"),
            "999999999999",
        ),
    ];

    let router = emulator();
    for (index, (case, submitted, expected)) in cases.into_iter().enumerate() {
        let request = create_request(
            &format!("ds-catalog-{index}"),
            submitted,
            &[("user_id", COLUMN_TYPES[0])],
        );
        let got = create_then_get(&router, case, request).await;
        let glue = reported_glue(&got);
        assert_eq!(
            glue["catalogId"],
            json!(expected),
            "case {case:?}: expected catalogId {expected:?}, got {:?}",
            glue["catalogId"]
        );
        assert_eq!(
            glue["tableName"],
            json!("interactions"),
            "case {case:?}: tableName must be the submitted one, got {:?}",
            glue["tableName"]
        );
        assert_eq!(
            glue["databaseName"],
            json!("clean_rooms_demo"),
            "case {case:?}: databaseName must be the submitted one, got {:?}",
            glue["databaseName"]
        );
    }
}

/// RULE: a dataset's input schema is a column set keyed by column name, so the read-back
/// reports its entries in column-name order whatever order the create listed them in,
/// and each entry still carries the `columnTypes` submitted with that name.
///
/// The rows differ along what the rule generalises over: how many columns there are (one,
/// a few, and the longest list `DatasetSchemaList` admits), the submitted order
/// (already ordered, reversed, rotated, interleaved) and the form of the names
/// (`ColumnName` admits letters, digits, underscores, spaces and inner hyphens).
#[tokio::test]
async fn a_dataset_schema_is_reported_in_column_name_order() {
    let many: Vec<(String, &str)> = (0..MAX_SCHEMA_COLUMNS)
        .rev()
        .map(|index| {
            (
                format!("col_{index:03}"),
                COLUMN_TYPES[index % COLUMN_TYPES.len()],
            )
        })
        .collect();
    let many: Vec<(&str, &str)> = many
        .iter()
        .map(|(name, column_type)| (name.as_str(), *column_type))
        .collect();

    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("one column", vec![("session_id", COLUMN_TYPES[4])]),
        (
            "three columns already in name order",
            vec![
                ("basket_id", COLUMN_TYPES[1]),
                ("customer", COLUMN_TYPES[0]),
                ("purchased_at", COLUMN_TYPES[2]),
            ],
        ),
        (
            "three columns in reverse name order",
            vec![
                ("purchased_at", COLUMN_TYPES[2]),
                ("customer", COLUMN_TYPES[0]),
                ("basket_id", COLUMN_TYPES[1]),
            ],
        ),
        (
            "three columns rotated out of name order",
            vec![
                ("customer", COLUMN_TYPES[0]),
                ("basket_id", COLUMN_TYPES[1]),
                ("purchased_at", COLUMN_TYPES[2]),
            ],
        ),
        (
            "names carrying digits, underscores, spaces and a hyphen",
            vec![
                ("zone 9", COLUMN_TYPES[3]),
                ("feature-1", COLUMN_TYPES[4]),
                ("_internal", COLUMN_TYPES[3]),
                ("0_bucket", COLUMN_TYPES[4]),
                ("alpha beta", COLUMN_TYPES[3]),
            ],
        ),
        ("the longest schema, reversed", many),
    ];

    let router = emulator();
    for (index, (case, submitted)) in cases.into_iter().enumerate() {
        let request = create_request(&format!("ds-schema-{index}"), None, &submitted);
        let got = create_then_get(&router, case, request).await;

        // The expectation is computed from the submitted pairs, not from the answer: the
        // names sorted, each with the type it was submitted beside.
        let mut expected = submitted.clone();
        expected.sort_by_key(|(left, _)| *left);
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(name, column_type)| (name.to_string(), column_type.to_string()))
            .collect();

        let observed = reported_columns(&got);
        assert_eq!(
            observed, expected,
            "case {case:?}: expected the schema in column-name order {expected:?}, got \
             {observed:?}"
        );
    }
}

/// RULE: every timestamp this service reports is an RFC 3339 date-time string, because
/// every timestamp member of this model declares `@timestampFormat("date-time")` — never
/// the epoch-seconds number `restJson1` defaults to. A reported instant is the one the
/// call that set it ran at.
///
/// The rows are the operations and members the rule covers, so it is pinned beyond the
/// two members the report named: the two instants of a training dataset as its Get and
/// its List report them, and an audience model's own two plus the training window it
/// echoes — submitted once as an epoch number and once as a date-time string, the two
/// forms a client may send.
#[tokio::test]
async fn every_reported_instant_is_a_date_time_string() {
    let router = emulator();
    let before = unix_now();

    let dataset = create_then_get(
        &router,
        "a training dataset",
        create_request("ds-instants", None, &[("user_id", COLUMN_TYPES[0])]),
    )
    .await;
    let dataset_arn = dataset.body["trainingDatasetArn"]
        .as_str()
        .expect("the read-back names the dataset")
        .to_string();

    let listed = list_datasets(&router).await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListTrainingDatasets, got {} with {:?}",
        listed.status, listed.body
    );
    let summary = listed.body["trainingDatasets"]
        .as_array()
        .and_then(|datasets| datasets.first().cloned())
        .unwrap_or_else(|| panic!("the list reports the dataset: {:?}", listed.body));

    // A training window sent as an epoch number and the same window sent as a date-time
    // string: both are accepted, and both are reported in the date-time form.
    let window_seconds = 1_767_225_600_i64;
    let window_text = "2026-01-01T00:00:00Z";
    let numeric_model = create_model(
        &router,
        json!({
            "name": "am-epoch-window",
            "trainingDatasetArn": dataset_arn,
            "trainingDataStartTime": window_seconds,
        }),
    )
    .await;
    assert_eq!(
        numeric_model.status, 200,
        "expected HTTP 200 from CreateAudienceModel, got {} with {:?}",
        numeric_model.status, numeric_model.body
    );
    let textual_model = create_model(
        &router,
        json!({
            "name": "am-date-time-window",
            "trainingDatasetArn": dataset_arn,
            "trainingDataStartTime": window_text,
        }),
    )
    .await;
    assert_eq!(
        textual_model.status, 200,
        "expected HTTP 200 from CreateAudienceModel, got {} with {:?}",
        textual_model.status, textual_model.body
    );
    let numeric_model = get_model(
        &router,
        numeric_model.body["audienceModelArn"]
            .as_str()
            .expect("the create names the model"),
    )
    .await;
    let textual_model = get_model(
        &router,
        textual_model.body["audienceModelArn"]
            .as_str()
            .expect("the create names the model"),
    )
    .await;
    let after = unix_now();

    // (case, the reported value, the instant it must name — `None` for one the call set,
    // which is checked against the window the calls ran in instead.)
    let rows: Vec<(&str, Value, Option<f64>)> = vec![
        ("GetTrainingDataset createTime", dataset.at("createTime"), None),
        ("GetTrainingDataset updateTime", dataset.at("updateTime"), None),
        (
            "ListTrainingDatasets createTime",
            summary["createTime"].clone(),
            None,
        ),
        (
            "ListTrainingDatasets updateTime",
            summary["updateTime"].clone(),
            None,
        ),
        (
            "GetAudienceModel createTime",
            numeric_model.at("createTime"),
            None,
        ),
        (
            "GetAudienceModel updateTime",
            numeric_model.at("updateTime"),
            None,
        ),
        (
            "an epoch-number training window, echoed",
            numeric_model.at("trainingDataStartTime"),
            Some(window_seconds as f64),
        ),
        (
            "a date-time training window, echoed",
            textual_model.at("trainingDataStartTime"),
            Some(window_seconds as f64),
        ),
    ];

    for (case, reported, instant) in rows {
        assert!(
            reported.is_string(),
            "case {case:?}: a date-time timestamp travels as an RFC 3339 string, got \
             {reported:?}"
        );
        assert!(
            is_timestamp(&reported),
            "case {case:?}: expected a date-time this model's clients can parse, got \
             {reported:?}"
        );
        let seconds = timestamp_seconds(&reported);
        match instant {
            Some(expected) => assert!(
                (seconds - expected).abs() < 0.001,
                "case {case:?}: expected the instant {expected}, got {seconds}"
            ),
            None => assert!(
                (before..=after).contains(&seconds),
                "case {case:?}: expected the instant the call ran, in {before}..={after}, \
                 got {seconds}"
            ),
        }
    }
}
