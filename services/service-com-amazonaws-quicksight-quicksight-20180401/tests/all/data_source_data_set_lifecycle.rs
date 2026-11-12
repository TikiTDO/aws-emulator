// SPDX-License-Identifier: CC0-1.0

//! The data source -> data set lifecycle, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test is one rule, applied per operation: a QuickSight resource is
//! identified by `(AwsAccountId, <resource>Id)`, a create persists it, and every later
//! read, list and delete answers from that stored record — so each case varies the
//! inputs the rule generalises over (the source kind, the import mode, the physical
//! table kind, the page size, the account) rather than re-running one recorded call.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_quicksight_quicksight_20180401::EmulatedService;

/// The account every case uses unless it is about account keying. This is the
/// canonical account id the emulator presents (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";
/// A password submitted in `Credentials`, asserted never to come back out.
const SECRET_PASSWORD: &str = "pw-never-echoed-42";

/// One emulator: the router a request is dispatched to, plus the shared engine, so a
/// case can also read the store the handlers wrote to.
///
/// `pub(crate)` because every suite in this test target drives the emulator the same
/// way; a sibling suite reuses this one rather than standing up a second harness.
pub(crate) struct Harness {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases are about the handlers, not about signature verification.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Dispatch one request, signed for [`REGION`], and return its status and parsed
    /// JSON body.
    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call_in_region(REGION, method, path, body).await
    }

    /// [`Self::call`], signed for `region`: the handlers take the request's region from
    /// its SigV4 credential scope, so this is how a case drives one emulator from two
    /// regions.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = match &body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("a JSON request body")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://quicksight.{region}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/quicksight/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn create_data_source(
        &self,
        account: &str,
        id: &str,
        name: &str,
        kind: &str,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(json!({
                "DataSourceId": id,
                "Name": name,
                "Type": kind,
                "SslProperties": { "DisableSsl": true },
                "Credentials": {
                    "CredentialPair": { "Username": "analyst", "Password": SECRET_PASSWORD }
                },
            })),
        )
        .await
    }

    async fn describe_data_source(&self, account: &str, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/accounts/{account}/data-sources/{id}"), None)
            .await
    }

    async fn delete_data_source(&self, account: &str, id: &str) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/accounts/{account}/data-sources/{id}"),
            None,
        )
        .await
    }

    async fn create_data_set(
        &self,
        account: &str,
        id: &str,
        name: &str,
        import_mode: &str,
        physical_table_map: Value,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/accounts/{account}/data-sets"),
            Some(json!({
                "DataSetId": id,
                "Name": name,
                "ImportMode": import_mode,
                "PhysicalTableMap": physical_table_map,
            })),
        )
        .await
    }

    async fn describe_data_set(&self, account: &str, id: &str) -> (u16, Value) {
        self.call("GET", &format!("/accounts/{account}/data-sets/{id}"), None)
            .await
    }

    async fn delete_data_set(&self, account: &str, id: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/accounts/{account}/data-sets/{id}"), None)
            .await
    }

    async fn list_data_sets(&self, account: &str, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/accounts/{account}/data-sets{query}"), None)
            .await
    }

    async fn list_data_sources(&self, account: &str, query: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/accounts/{account}/data-sources{query}"),
            None,
        )
        .await
    }

    /// The data source ARN a created source has, as the create response reported it.
    async fn data_source_fixture(&self, account: &str, id: &str, kind: &str) -> String {
        let (status, body) = self.create_data_source(account, id, "fixture source", kind).await;
        assert_eq!(status, 202, "fixture {id}: create returned {status}: {body}");
        body["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("fixture {id}: create returned no Arn: {body}"))
            .to_string()
    }

    /// Bring the lifecycle transition the clock owes the data source `(account, id)`
    /// forward, so a case can observe the terminal status without sleeping for the
    /// span.
    ///
    /// `pub(crate)` because every suite that creates a data source needs it: the
    /// create and the update both leave the resource in a non-terminal status.
    pub(crate) fn settle_data_source(&self, account: &str, id: &str) {
        let state = service_com_amazonaws_quicksight_quicksight_20180401::state::service_state(
            &*self.engine,
        )
        .expect("the QuickSight store");
        let mut store = state.lock();
        let record = store
            .data_sources
            .get_mut(&(account.to_string(), id.to_string()))
            .unwrap_or_else(|| panic!("case {id:?}: the created data source is not stored"));
        record.status_settles_at = SystemTime::now() - Duration::from_millis(1);
    }
}

fn data_source_arn(account: &str, id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:datasource/{id}")
}

/// A one-entry `PhysicalTableMap` holding a relational table on `data_source_arn`.
fn relational_table_map(data_source_arn: &str) -> Value {
    json!({
        "orders-table": {
            "RelationalTable": {
                "DataSourceArn": data_source_arn,
                "Schema": "public",
                "Name": "orders",
                "InputColumns": [
                    { "Name": "order_id", "Type": "INTEGER" },
                    { "Name": "customer", "Type": "STRING" },
                ],
            }
        }
    })
}

/// A timestamp member is present and is a plausible epoch instant.
fn assert_epoch_timestamp(case: &str, member: &str, value: &Value) {
    let seconds = value.as_f64();
    assert!(
        seconds.is_some_and(|seconds| seconds > 1_600_000_000.0),
        "case {case}: {member} expected an epoch timestamp, got {value}"
    );
}

/// Rows differ along `Type`: the kind the issue cites plus four it does not,
/// including the first and last members of the model's enum. Catalog source:
/// `smithy_query(query="shape", shape="DataSourceType")`.
const DATA_SOURCE_CASES: &[(&str, &str, &str)] = &[
    ("ds-adobe", "Marketing", "ADOBE_ANALYTICS"),
    ("ds-athena", "My Athena", "ATHENA"),
    ("ds-redshift", "Warehouse", "REDSHIFT"),
    ("ds-snowflake", "Snowy", "SNOWFLAKE"),
    ("ds-qbusiness", "Q", "QBUSINESS"),
];

#[tokio::test]
async fn a_created_data_source_reads_back_for_every_kind_of_source() {
    let harness = Harness::new();

    let mut request_ids: Vec<String> = Vec::new();
    for (id, name, kind) in DATA_SOURCE_CASES {
        let (status, body) = harness.create_data_source(ACCOUNT, id, name, kind).await;
        assert_eq!(
            status, 202,
            "case {id}: create expected 202, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(data_source_arn(ACCOUNT, id)),
            "case {id}: create expected the account+id derived ARN, got {body}"
        );
        assert_eq!(
            body["DataSourceId"],
            json!(id),
            "case {id}: create echoed the wrong id: {body}"
        );
        assert_eq!(
            body["CreationStatus"],
            json!("CREATION_IN_PROGRESS"),
            "case {id}: the create accepts the work, so it expected CreationStatus \
             CREATION_IN_PROGRESS, got {body}"
        );
        // `Status` is `@httpResponseCode`-bound, so it IS the HTTP status asserted
        // above rather than a body member.
        assert_eq!(
            body["Status"],
            Value::Null,
            "case {id}: Status is bound to the HTTP status, so it is not a body member: {body}"
        );
        let request_id = body["RequestId"].as_str().unwrap_or_default().to_string();
        assert!(
            !request_id.is_empty(),
            "case {id}: create expected a RequestId, got {body}"
        );
        request_ids.push(request_id);
    }

    // A caller reading the data source straight after the create still sees the
    // non-terminal status the create reported: the transition is owed, not immediate.
    for (id, _, _) in DATA_SOURCE_CASES {
        let (status, body) = harness.describe_data_source(ACCOUNT, id).await;
        assert_eq!(
            status, 200,
            "case {id}: describe expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["DataSource"]["Status"],
            json!("CREATION_IN_PROGRESS"),
            "case {id}: before the span is over the data source is still being \
             created: {body}"
        );
    }

    for (id, name, kind) in DATA_SOURCE_CASES {
        // The terminal status is reached by the clock, materialised on read; bringing
        // the due instant forward is how a case observes it without waiting.
        harness.settle_data_source(ACCOUNT, id);
        let (status, body) = harness.describe_data_source(ACCOUNT, id).await;
        assert_eq!(
            status, 200,
            "case {id}: describe expected 200, got {status} with body {body}"
        );
        let source = &body["DataSource"];
        assert_eq!(
            source["DataSourceId"],
            json!(id),
            "case {id}: describe returned the wrong id: {body}"
        );
        assert_eq!(
            source["Name"],
            json!(name),
            "case {id}: describe returned the wrong Name: {body}"
        );
        assert_eq!(
            source["Type"],
            json!(kind),
            "case {id}: describe returned the wrong Type: {body}"
        );
        assert_eq!(
            source["Arn"],
            json!(data_source_arn(ACCOUNT, id)),
            "case {id}: describe returned the wrong Arn: {body}"
        );
        assert_eq!(
            source["Status"],
            json!("CREATION_SUCCESSFUL"),
            "case {id}: describe returned the wrong Status: {body}"
        );
        assert_eq!(
            source["SslProperties"]["DisableSsl"],
            json!(true),
            "case {id}: describe lost the submitted SslProperties: {body}"
        );
        assert_epoch_timestamp(id, "CreatedTime", &source["CreatedTime"]);
        assert_epoch_timestamp(id, "LastUpdatedTime", &source["LastUpdatedTime"]);
        let rendered = body.to_string();
        assert!(
            !rendered.contains(SECRET_PASSWORD),
            "case {id}: describe echoed the submitted credentials: {rendered}"
        );
        request_ids.push(body["RequestId"].as_str().unwrap_or_default().to_string());
    }

    // Every response carries its own request id.
    let unique: std::collections::HashSet<&String> = request_ids.iter().collect();
    assert_eq!(
        unique.len(),
        request_ids.len(),
        "a RequestId repeated across responses: {request_ids:?}"
    );
}

#[tokio::test]
async fn a_type_outside_the_models_enum_is_refused() {
    let harness = Harness::new();
    // Rows: a value of the right shape that is not a member, a lowercased member, and
    // the empty string. None of them names a data source kind.
    for kind in ["NOT_A_SOURCE", "athena", ""] {
        let (status, body) = harness
            .create_data_source(ACCOUNT, "ds-bad-type", "src", kind)
            .await;
        assert_eq!(
            status, 400,
            "case {kind:?}: expected 400 for a Type outside DataSourceType, got {status} with {body}"
        );
        let (describe_status, describe_body) =
            harness.describe_data_source(ACCOUNT, "ds-bad-type").await;
        assert_eq!(
            describe_status, 404,
            "case {kind:?}: a refused create must store nothing, got {describe_status} with {describe_body}"
        );
    }
}

#[tokio::test]
async fn recreating_an_id_that_already_exists_conflicts() {
    let harness = Harness::new();
    let arn = harness
        .data_source_fixture(ACCOUNT, "ds-1", "ATHENA")
        .await;
    let (status, body) = harness
        .create_data_set(
            ACCOUNT,
            "dset-1",
            "Sales",
            "SPICE",
            relational_table_map(&arn),
        )
        .await;
    assert_eq!(status, 200, "fixture data set: got {status} with {body}");

    // Rows: both resource kinds the slice creates. Re-creating either id conflicts.
    let cases: [(&str, String, Value); 2] = [
        (
            "data source",
            format!("/accounts/{ACCOUNT}/data-sources"),
            json!({ "DataSourceId": "ds-1", "Name": "again", "Type": "ATHENA" }),
        ),
        (
            "data set",
            format!("/accounts/{ACCOUNT}/data-sets"),
            json!({
                "DataSetId": "dset-1",
                "Name": "again",
                "ImportMode": "DIRECT_QUERY",
                "PhysicalTableMap": relational_table_map(&arn),
            }),
        ),
    ];
    for (case, path, body) in cases {
        let (status, response) = harness.call("POST", &path, Some(body)).await;
        assert_eq!(
            status, 409,
            "case {case}: expected 409 on a duplicate id, got {status} with {response}"
        );
        assert_eq!(
            response["__type"],
            json!("ResourceExistsException"),
            "case {case}: expected ResourceExistsException, got {response}"
        );
    }

    // The conflict changed nothing: the original records are still the stored ones.
    let (_, source) = harness.describe_data_source(ACCOUNT, "ds-1").await;
    assert_eq!(
        source["DataSource"]["Name"],
        json!("fixture source"),
        "the refused create overwrote the stored data source: {source}"
    );
    let (_, data_set) = harness.describe_data_set(ACCOUNT, "dset-1").await;
    assert_eq!(
        data_set["DataSet"]["ImportMode"],
        json!("SPICE"),
        "the refused create overwrote the stored data set: {data_set}"
    );
}

#[tokio::test]
async fn an_id_the_store_does_not_hold_is_not_found() {
    let harness = Harness::new();
    // Rows: every reader and deleter in the slice, against an id nothing created.
    let cases: [(&str, &str, String); 4] = [
        (
            "DescribeDataSource",
            "GET",
            format!("/accounts/{ACCOUNT}/data-sources/absent"),
        ),
        (
            "DescribeDataSet",
            "GET",
            format!("/accounts/{ACCOUNT}/data-sets/absent"),
        ),
        (
            "DeleteDataSource",
            "DELETE",
            format!("/accounts/{ACCOUNT}/data-sources/absent"),
        ),
        (
            "DeleteDataSet",
            "DELETE",
            format!("/accounts/{ACCOUNT}/data-sets/absent"),
        ),
    ];
    for (case, method, path) in cases {
        let (status, body) = harness.call(method, &path, None).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404 for an unknown id, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn resources_are_invisible_under_another_account_id() {
    let harness = Harness::new();
    let arn = harness
        .data_source_fixture(ACCOUNT, "ds-shared-id", "ATHENA")
        .await;
    let (status, body) = harness
        .create_data_set(
            ACCOUNT,
            "dset-shared-id",
            "Sales",
            "SPICE",
            relational_table_map(&arn),
        )
        .await;
    assert_eq!(status, 200, "fixture data set: got {status} with {body}");

    // Rows: the same ids under a different account id, through each reader.
    let cases: [(&str, String); 2] = [
        (
            "data source",
            format!("/accounts/{OTHER_ACCOUNT}/data-sources/ds-shared-id"),
        ),
        (
            "data set",
            format!("/accounts/{OTHER_ACCOUNT}/data-sets/dset-shared-id"),
        ),
    ];
    for (case, path) in cases {
        let (status, body) = harness.call("GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case}: another account must not see it, got {status} with {body}"
        );
    }

    for (case, query) in [("data sets", "data-sets"), ("data sources", "data-sources")] {
        let (status, body) = harness
            .call("GET", &format!("/accounts/{OTHER_ACCOUNT}/{query}"), None)
            .await;
        assert_eq!(status, 200, "case {case}: list expected 200, got {body}");
        let items = if query == "data-sets" {
            &body["DataSetSummaries"]
        } else {
            &body["DataSources"]
        };
        assert_eq!(
            items.as_array().map(Vec::len),
            Some(0),
            "case {case}: another account's list must be empty, got {body}"
        );
    }

    // And the same id CAN be created again under the other account, as its own resource.
    let (status, body) = harness
        .create_data_source(OTHER_ACCOUNT, "ds-shared-id", "Theirs", "S3")
        .await;
    assert_eq!(
        status, 202,
        "the same id under another account is a new resource, got {status} with {body}"
    );
    assert_eq!(
        body["Arn"],
        json!(data_source_arn(OTHER_ACCOUNT, "ds-shared-id")),
        "the ARN must carry the owning account id: {body}"
    );
}

#[tokio::test]
async fn a_created_data_set_reads_back_for_both_import_modes() {
    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;

    // Rows: both members of the model's DataSetImportMode enum. Source:
    // smithy_query(query="shape", shape="DataSetImportMode").
    for (id, import_mode, expects_ingestion) in [
        ("dset-spice", "SPICE", true),
        ("dset-direct", "DIRECT_QUERY", false),
    ] {
        let (status, body) = harness
            .create_data_set(ACCOUNT, id, "Sales", import_mode, relational_table_map(&arn))
            .await;
        assert_eq!(
            status, 200,
            "case {import_mode}: create expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:dataset/{id}")),
            "case {import_mode}: create returned the wrong Arn: {body}"
        );
        assert_eq!(
            body["DataSetId"],
            json!(id),
            "case {import_mode}: create echoed the wrong id: {body}"
        );

        let ingestion_id = body["IngestionId"].as_str();
        assert_eq!(
            ingestion_id.is_some(),
            expects_ingestion,
            "case {import_mode}: IngestionId presence expected {expects_ingestion}, got {body}"
        );
        if let Some(ingestion_id) = ingestion_id {
            assert!(
                !ingestion_id.is_empty(),
                "case {import_mode}: expected a non-empty IngestionId, got {body}"
            );
            assert_eq!(
                body["IngestionArn"],
                json!(format!(
                    "arn:aws:quicksight:{REGION}:{ACCOUNT}:dataset/{id}/ingestion/{ingestion_id}"
                )),
                "case {import_mode}: IngestionArn must name the ingestion: {body}"
            );
            // The id names a stored ingestion, not just a value in one response.
            let state = service_com_amazonaws_quicksight_quicksight_20180401::state::service_state(
                &*harness.engine,
            )
            .expect("the QuickSight store");
            let store = state.lock();
            let stored = store
                .ingestions
                .get(&(
                    ACCOUNT.to_string(),
                    id.to_string(),
                    ingestion_id.to_string(),
                ))
                .unwrap_or_else(|| {
                    panic!("case {import_mode}: the returned IngestionId is not stored")
                });
            assert_eq!(
                stored.ingestion_status, "INITIALIZED",
                "case {import_mode}: a new ingestion starts in INITIALIZED, got {:?}",
                stored.ingestion_status
            );
            // The completion the clock owes is materialised by time, not by a sleep,
            // and a caller acting straight away still sees the non-terminal status.
            let started = stored.completes_at
                - service_com_amazonaws_quicksight_quicksight_20180401::state::INGESTION_DURATION;
            assert_eq!(
                stored.status_at(started),
                "INITIALIZED",
                "case {import_mode}: at the start instant the ingestion is not yet complete"
            );
            assert_eq!(
                stored.status_at(stored.completes_at + Duration::from_millis(1)),
                "COMPLETED",
                "case {import_mode}: once due, the ingestion reports COMPLETED"
            );
            assert!(
                stored.completes_at > started,
                "case {import_mode}: the transition span must not be zero"
            );
        }

        let (status, body) = harness.describe_data_set(ACCOUNT, id).await;
        assert_eq!(
            status, 200,
            "case {import_mode}: describe expected 200, got {status} with {body}"
        );
        let data_set = &body["DataSet"];
        assert_eq!(
            data_set["Name"],
            json!("Sales"),
            "case {import_mode}: describe returned the wrong Name: {body}"
        );
        assert_eq!(
            data_set["ImportMode"],
            json!(import_mode),
            "case {import_mode}: describe returned the wrong ImportMode: {body}"
        );
        assert_eq!(
            data_set["PhysicalTableMap"]["orders-table"]["RelationalTable"]["Name"],
            json!("orders"),
            "case {import_mode}: describe lost the submitted PhysicalTableMap: {body}"
        );
        // The output columns are derived from the tables the data set is built on.
        assert_eq!(
            data_set["OutputColumns"],
            json!([
                { "Name": "order_id", "Type": "INTEGER" },
                { "Name": "customer", "Type": "STRING" },
            ]),
            "case {import_mode}: OutputColumns must come from the physical table's columns: {body}"
        );
        assert_eq!(
            data_set["ConsumedSpiceCapacityInBytes"],
            json!(0),
            "case {import_mode}: nothing is ingested yet, so no SPICE capacity is used: {body}"
        );
        assert_epoch_timestamp(import_mode, "CreatedTime", &data_set["CreatedTime"]);
    }
}

#[tokio::test]
async fn a_physical_table_naming_an_unstored_data_source_is_not_found() {
    let harness = Harness::new();
    let stored = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;
    let absent = data_source_arn(ACCOUNT, "ds-absent");
    // The same data source, but owned by another account — invisible here.
    let other_account = data_source_arn(OTHER_ACCOUNT, "ds-1");

    // Rows: three of the five kinds of PhysicalTable, each naming its data source by
    // ARN, plus a map where only the second entry is unresolvable.
    let cases: [(&str, Value); 6] = [
        (
            "RelationalTable",
            json!({ "t": { "RelationalTable": {
                "DataSourceArn": absent, "Name": "orders",
                "InputColumns": [{ "Name": "a", "Type": "STRING" }] } } }),
        ),
        (
            "CustomSql",
            json!({ "t": { "CustomSql": {
                "DataSourceArn": absent, "Name": "q", "SqlQuery": "select 1",
                "Columns": [{ "Name": "a", "Type": "INTEGER" }] } } }),
        ),
        (
            "S3Source",
            json!({ "t": { "S3Source": {
                "DataSourceArn": absent,
                "InputColumns": [{ "Name": "a", "Type": "STRING" }] } } }),
        ),
        (
            "second entry of two",
            json!({
                "ok": { "RelationalTable": {
                    "DataSourceArn": stored, "Name": "orders",
                    "InputColumns": [{ "Name": "a", "Type": "STRING" }] } },
                "zz-bad": { "RelationalTable": {
                    "DataSourceArn": other_account, "Name": "orders",
                    "InputColumns": [{ "Name": "a", "Type": "STRING" }] } },
            }),
        ),
        (
            "an ARN of another service entirely",
            json!({ "t": { "RelationalTable": {
                "DataSourceArn": "arn:aws:s3:::some-bucket", "Name": "orders",
                "InputColumns": [{ "Name": "a", "Type": "STRING" }] } } }),
        ),
        (
            "the right id in the wrong region",
            json!({ "t": { "RelationalTable": {
                "DataSourceArn": format!("arn:aws:quicksight:eu-west-1:{ACCOUNT}:datasource/ds-1"),
                "Name": "orders",
                "InputColumns": [{ "Name": "a", "Type": "STRING" }] } } }),
        ),
    ];
    for (case, physical_table_map) in cases {
        let (status, body) = harness
            .create_data_set(ACCOUNT, "dset-bad", "Sales", "SPICE", physical_table_map)
            .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404 for an unresolvable DataSourceArn, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
        let (describe_status, describe_body) = harness.describe_data_set(ACCOUNT, "dset-bad").await;
        assert_eq!(
            describe_status, 404,
            "case {case}: a refused create must store nothing, got {describe_status} with {describe_body}"
        );
    }
}

#[tokio::test]
async fn an_import_mode_outside_the_models_enum_is_refused() {
    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;
    for import_mode in ["spice", "SPICE_PLUS", ""] {
        let (status, body) = harness
            .create_data_set(
                ACCOUNT,
                "dset-bad-mode",
                "Sales",
                import_mode,
                relational_table_map(&arn),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {import_mode:?}: expected 400 for an ImportMode outside the enum, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn listing_pages_through_every_stored_item_exactly_once() {
    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-page", "ATHENA").await;
    let ids: Vec<String> = (0..7).map(|n| format!("dset-{n:02}")).collect();
    for id in &ids {
        let (status, body) = harness
            .create_data_set(ACCOUNT, id, "Sales", "DIRECT_QUERY", relational_table_map(&arn))
            .await;
        assert_eq!(status, 200, "fixture {id}: got {status} with {body}");
    }

    // Rows: page sizes at both published bounds of MaxResults (1..100) and in between,
    // plus the unpaginated request. Each must walk the 7 data sets exactly once.
    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let case = format!("max-results={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (&token, max_results) {
                (Some(token), Some(max)) => format!("?max-results={max}&next-token={token}"),
                (Some(token), None) => format!("?next-token={token}"),
                (None, Some(max)) => format!("?max-results={max}"),
                (None, None) => String::new(),
            };
            let (status, body) = harness.list_data_sets(ACCOUNT, &query).await;
            assert_eq!(
                status, 200,
                "case {case}: list expected 200, got {status} with {body}"
            );
            let page = body["DataSetSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: no DataSetSummaries in {body}"))
                .clone();
            if let Some(max) = max_results {
                assert!(
                    page.len() <= usize::try_from(max).unwrap_or_default(),
                    "case {case}: a page held {} items, more than MaxResults: {body}",
                    page.len()
                );
            }
            for summary in &page {
                assert_eq!(
                    summary["ImportMode"],
                    json!("DIRECT_QUERY"),
                    "case {case}: a summary came back with the wrong ImportMode: {summary}"
                );
                assert_eq!(
                    summary["Arn"],
                    json!(format!(
                        "arn:aws:quicksight:{REGION}:{ACCOUNT}:dataset/{}",
                        summary["DataSetId"].as_str().unwrap_or_default()
                    )),
                    "case {case}: a summary came back with the wrong Arn: {summary}"
                );
                assert_epoch_timestamp(&case, "CreatedTime", &summary["CreatedTime"]);
                seen.push(summary["DataSetId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, ids,
            "case {case}: paging must yield every id exactly once, saw {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let harness = Harness::new();
    // Rows: an opaque-looking token, a token with this service's prefix but a
    // non-hex body, and an empty one.
    for token in ["not-a-token", "qs1-zzzz", "qs1-abc", ""] {
        for (case, list) in [("data sets", "data-sets"), ("data sources", "data-sources")] {
            let (status, body) = harness
                .call(
                    "GET",
                    &format!("/accounts/{ACCOUNT}/{list}?next-token={token}"),
                    None,
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case} {token:?}: expected 400 for an unparseable NextToken, got {status} with {body}"
            );
            assert_eq!(
                body["__type"],
                json!("InvalidNextTokenException"),
                "case {case} {token:?}: expected InvalidNextTokenException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let harness = Harness::new();
    // The model constrains MaxResults to 1..100: each published bound is accepted and
    // the value just past it is refused.
    for (max_results, expected) in [(1, 200), (100, 200), (0, 400), (101, 400)] {
        let (status, body) = harness
            .list_data_sets(ACCOUNT, &format!("?max-results={max_results}"))
            .await;
        assert_eq!(
            status, expected,
            "case max-results={max_results}: expected {expected}, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_deleted_resource_is_gone_from_every_read() {
    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;
    for id in ["dset-keep", "dset-drop"] {
        let (status, body) = harness
            .create_data_set(ACCOUNT, id, "Sales", "SPICE", relational_table_map(&arn))
            .await;
        assert_eq!(status, 200, "fixture {id}: got {status} with {body}");
    }

    let (status, body) = harness.delete_data_set(ACCOUNT, "dset-drop").await;
    assert_eq!(status, 200, "delete expected 200, got {status} with {body}");
    assert_eq!(
        body["Arn"],
        json!(format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:dataset/dset-drop")),
        "delete must echo the deleted resource's ARN: {body}"
    );
    assert_eq!(
        body["DataSetId"],
        json!("dset-drop"),
        "delete must echo the deleted id: {body}"
    );

    let (status, body) = harness.describe_data_set(ACCOUNT, "dset-drop").await;
    assert_eq!(
        status, 404,
        "a deleted data set must not describe, got {status} with {body}"
    );
    let (_, body) = harness.list_data_sets(ACCOUNT, "").await;
    let listed: Vec<&str> = body["DataSetSummaries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["DataSetId"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec!["dset-keep"],
        "the deleted data set must be gone from the list: {body}"
    );
    // Its ingestions went with it.
    let state =
        service_com_amazonaws_quicksight_quicksight_20180401::state::service_state(&*harness.engine)
            .expect("the QuickSight store");
    let remaining: Vec<String> = state
        .lock()
        .ingestions
        .keys()
        .map(|(_, data_set_id, _)| data_set_id.clone())
        .collect();
    assert!(
        !remaining.iter().any(|id| id == "dset-drop"),
        "a deleted data set's ingestions must be gone too, left: {remaining:?}"
    );

    let (status, body) = harness.delete_data_source(ACCOUNT, "ds-1").await;
    assert_eq!(
        status, 200,
        "deleting the data source expected 200, got {status} with {body}"
    );
    let (status, body) = harness.describe_data_source(ACCOUNT, "ds-1").await;
    assert_eq!(
        status, 404,
        "a deleted data source must not describe, got {status} with {body}"
    );
    let (_, body) = harness.list_data_sources(ACCOUNT, "").await;
    assert_eq!(
        body["DataSources"].as_array().map(Vec::len),
        Some(0),
        "the deleted data source must be gone from the list: {body}"
    );
}

#[tokio::test]
async fn a_read_does_not_change_what_the_next_caller_sees() {
    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;
    let (status, body) = harness
        .create_data_set(
            ACCOUNT,
            "dset-1",
            "Sales",
            "SPICE",
            relational_table_map(&arn),
        )
        .await;
    assert_eq!(status, 200, "fixture data set: got {status} with {body}");

    let snapshot = {
        let state = service_com_amazonaws_quicksight_quicksight_20180401::state::service_state(
            &*harness.engine,
        )
        .expect("the QuickSight store");
        let store = state.lock();
        format!("{:?}", (&store.data_sources, &store.data_sets, &store.ingestions))
    };

    // Rows: every reporting operation in the slice, each called twice.
    for _ in 0..2 {
        for (case, path) in [
            ("DescribeDataSource", format!("/accounts/{ACCOUNT}/data-sources/ds-1")),
            ("DescribeDataSet", format!("/accounts/{ACCOUNT}/data-sets/dset-1")),
            ("ListDataSources", format!("/accounts/{ACCOUNT}/data-sources")),
            ("ListDataSets", format!("/accounts/{ACCOUNT}/data-sets")),
        ] {
            let (status, body) = harness.call("GET", &path, None).await;
            assert_eq!(status, 200, "case {case}: expected 200, got {status} with {body}");
        }
    }

    let after = {
        let state = service_com_amazonaws_quicksight_quicksight_20180401::state::service_state(
            &*harness.engine,
        )
        .expect("the QuickSight store");
        let store = state.lock();
        format!("{:?}", (&store.data_sources, &store.data_sets, &store.ingestions))
    };
    assert_eq!(
        after, snapshot,
        "a reporting operation changed the store it reports on"
    );
}

#[tokio::test]
async fn an_unimplemented_operation_still_answers_501() {
    let harness = Harness::new();
    // The rest of QuickSight is untouched by this slice: it must keep saying
    // "not written yet" rather than answering an empty success.
    let (status, body) = harness
        .call(
            "GET",
            &format!("/accounts/{ACCOUNT}/data-sets/dset-1/ingestions"),
            None,
        )
        .await;
    assert_eq!(
        status, 501,
        "ListIngestions is still a stub and must answer 501, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_create_is_refused_once_the_accounts_ceiling_is_reached() {
    use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;
    let (status, body) = harness
        .create_data_set(
            ACCOUNT,
            "dset-1",
            "Sales",
            "DIRECT_QUERY",
            relational_table_map(&arn),
        )
        .await;
    assert_eq!(status, 200, "fixture data set: got {status} with {body}");

    // Fill the account to its ceiling by cloning the two stored records under fresh
    // ids — the ceiling, not the filling, is what is under test here.
    {
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let mut store = state.lock();
        let source = store
            .data_sources
            .get(&(ACCOUNT.to_string(), "ds-1".to_string()))
            .cloned()
            .expect("the fixture data source");
        // The fixture already holds one of each, so fill to exactly the ceiling.
        for n in 0..qs_state::MAX_DATA_SOURCES_PER_ACCOUNT - 1 {
            let id = format!("filler-{n:05}");
            let mut record = source.clone();
            record.arn = format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:datasource/{id}");
            record.data_source_id = id.clone();
            store.data_sources.insert((ACCOUNT.to_string(), id), record);
        }
        let data_set = store
            .data_sets
            .get(&(ACCOUNT.to_string(), "dset-1".to_string()))
            .cloned()
            .expect("the fixture data set");
        for n in 0..qs_state::MAX_DATA_SETS_PER_ACCOUNT - 1 {
            let id = format!("filler-{n:05}");
            let mut record = data_set.clone();
            record.arn = format!("arn:aws:quicksight:{REGION}:{ACCOUNT}:dataset/{id}");
            record.data_set_id = id.clone();
            store.data_sets.insert((ACCOUNT.to_string(), id), record);
        }
    }

    {
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let store = state.lock();
        assert_eq!(
            store.data_source_count(ACCOUNT),
            qs_state::MAX_DATA_SOURCES_PER_ACCOUNT,
            "the account must be filled to exactly the data source ceiling"
        );
        assert_eq!(
            store.data_set_count(ACCOUNT),
            qs_state::MAX_DATA_SETS_PER_ACCOUNT,
            "the account must be filled to exactly the data set ceiling"
        );
    }

    // Rows: both creates, at the ceiling, then the same create for an account that is
    // nowhere near it — the ceiling is per account, not per process.
    let (status, body) = harness
        .create_data_source(ACCOUNT, "ds-over", "Too many", "ATHENA")
        .await;
    assert_eq!(
        status, 409,
        "case data source at the ceiling: expected 409, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("LimitExceededException"),
        "case data source at the ceiling: expected LimitExceededException, got {body}"
    );

    let (status, body) = harness
        .create_data_set(
            ACCOUNT,
            "dset-over",
            "Too many",
            "SPICE",
            relational_table_map(&arn),
        )
        .await;
    assert_eq!(
        status, 409,
        "case data set at the ceiling: expected 409, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("LimitExceededException"),
        "case data set at the ceiling: expected LimitExceededException, got {body}"
    );

    let (status, body) = harness
        .create_data_source(OTHER_ACCOUNT, "ds-elsewhere", "Fine", "ATHENA")
        .await;
    assert_eq!(
        status, 202,
        "case another account under the ceiling: expected 202, got {status} with {body}"
    );

    // Freeing a slot lets the create through again.
    let (status, body) = harness.delete_data_source(ACCOUNT, "filler-00000").await;
    assert_eq!(status, 200, "freeing a slot: got {status} with {body}");
    let (status, body) = harness
        .create_data_source(ACCOUNT, "ds-over", "Now there is room", "ATHENA")
        .await;
    assert_eq!(
        status, 202,
        "case a freed slot: expected 202, got {status} with {body}"
    );
}

#[tokio::test]
async fn a_create_is_refused_once_the_whole_store_is_full_however_the_account_id_varies() {
    use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

    let harness = Harness::new();
    let arn = harness.data_source_fixture(ACCOUNT, "ds-1", "ATHENA").await;
    let (status, body) = harness
        .create_data_set(
            ACCOUNT,
            "dset-1",
            "Sales",
            "DIRECT_QUERY",
            relational_table_map(&arn),
        )
        .await;
    assert_eq!(status, 200, "fixture data set: got {status} with {body}");

    // Fill both stores to their TOTAL ceilings, spread over many synthetic account ids
    // — the shape a create loop that varies the request's account id produces.
    {
        let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
        let mut store = state.lock();
        let source = store
            .data_sources
            .get(&(ACCOUNT.to_string(), "ds-1".to_string()))
            .cloned()
            .expect("the fixture data source");
        for n in 0..qs_state::MAX_DATA_SOURCES - 1 {
            let account = format!("{:012}", 100_000_000 + n / 100);
            let id = format!("filler-{n:06}");
            let mut record = source.clone();
            record.arn = format!("arn:aws:quicksight:{REGION}:{account}:datasource/{id}");
            record.data_source_id = id.clone();
            store.data_sources.insert((account, id), record);
        }
        let data_set = store
            .data_sets
            .get(&(ACCOUNT.to_string(), "dset-1".to_string()))
            .cloned()
            .expect("the fixture data set");
        for n in 0..qs_state::MAX_DATA_SETS - 1 {
            let account = format!("{:012}", 100_000_000 + n / 100);
            let id = format!("filler-{n:06}");
            let mut record = data_set.clone();
            record.arn = format!("arn:aws:quicksight:{REGION}:{account}:dataset/{id}");
            record.data_set_id = id.clone();
            store.data_sets.insert((account, id), record);
        }
        assert_eq!(
            store.data_sources.len(),
            qs_state::MAX_DATA_SOURCES,
            "the store must be filled to exactly the total data source ceiling"
        );
        assert_eq!(
            store.data_sets.len(),
            qs_state::MAX_DATA_SETS,
            "the store must be filled to exactly the total data set ceiling"
        );
    }

    // Rows: a brand-new account id — far below the per-account ceiling — for both
    // creates. The total ceiling holds however the caller varies the account id.
    for account in [OTHER_ACCOUNT, "999999999999"] {
        let (status, body) = harness
            .create_data_source(account, "ds-over", "Too many", "ATHENA")
            .await;
        assert_eq!(
            status, 409,
            "case account {account}: a fresh account must not reset the total ceiling, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("LimitExceededException"),
            "case account {account}: expected LimitExceededException, got {body}"
        );

        let (status, body) = harness
            .create_data_set(
                account,
                "dset-over",
                "Too many",
                "SPICE",
                relational_table_map(&arn),
            )
            .await;
        assert_eq!(
            status, 409,
            "case account {account}: a fresh account must not reset the total data set ceiling, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("LimitExceededException"),
            "case account {account}: expected LimitExceededException, got {body}"
        );
    }
}

#[tokio::test]
async fn an_account_id_outside_the_modelled_pattern_is_refused() {
    let harness = Harness::new();
    // `AwsAccountId` is `len 12..12, pattern ^[0-9]{12}$`, enforced before the handler
    // runs, so a malformed account id never becomes a store key. Rows: too short, too
    // long, and non-digits.
    for account in ["12345678901", "1234567890123", "abcdefghijkl"] {
        let (status, body) = harness
            .create_data_source(account, "ds-1", "src", "ATHENA")
            .await;
        assert_eq!(
            status, 400,
            "case account {account:?}: expected 400 for an account id outside the pattern, got {status} with {body}"
        );
        let (status, body) = harness.describe_data_source(account, "ds-1").await;
        assert_eq!(
            status, 400,
            "case account {account:?}: describe must refuse it too, got {status} with {body}"
        );
    }
}

/// The clock-owed span of a SPICE ingestion is a real, non-zero duration.
#[test]
fn an_ingestions_completion_is_due_within_the_chosen_span() {
    let span = service_com_amazonaws_quicksight_quicksight_20180401::state::INGESTION_DURATION;
    assert!(
        span >= Duration::from_millis(250) && span <= Duration::from_millis(750),
        "the transition span must be between 250 and 750 ms, got {span:?}"
    );
    assert!(
        SystemTime::now() + span > SystemTime::now(),
        "the span must move the due instant forward"
    );
}
