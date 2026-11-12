// SPDX-License-Identifier: CC0-1.0

//! The Clean Rooms ML training-dataset -> audience-model lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one the control plane states for this resource
//! chain: a resource's members are whatever its create stored, every operation computes
//! its answer from that record, and the ARN the create minted is the only name any later
//! call accepts for it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::state::{
    AUDIENCE_MODEL_TRAINING, RESOURCE_QUOTA,
};
use service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the requests in this suite are signed for, unless a case signs for another.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`),
/// spelled out here rather than imported so a wrong account in the handler fails.
const ACCOUNT: &str = "000000000000";

/// An IAM role ARN matching `IamRoleArn`'s
/// `^arn:aws[-a-z]*:iam::[0-9]{12}:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/CleanRoomsMLRole";

/// A KMS key ARN matching `KmsKeyArn`'s `^arn:aws[-a-z]*:kms:[-a-z0-9]+:[0-9]{12}:key/.+$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:eu-west-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// Every value `ColumnType` accepts, from
/// `smithy_query(query="shape", shape="ColumnType")`. The cases below build schemas out
/// of entries the issue never cited, so no handler can special-case one spelling.
const COLUMN_TYPES: &[&str] = &[
    "USER_ID",
    "ITEM_ID",
    "TIMESTAMP",
    "CATEGORICAL_FEATURE",
    "NUMERICAL_FEATURE",
];

/// The only value `DatasetType` accepts, from the same query.
const DATASET_TYPE: &str = "INTERACTIONS";

/// The largest page `ListTrainingDatasetsRequest$maxResults` admits
/// (`@range(min: 1, max: 100)`), spelled out rather than imported for the same reason.
const MAX_RESULTS_CEILING: i64 = 100;

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope naming this service in `region`. It is what routes a request
/// to Clean Rooms ML, and its region is what a minted ARN names.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cleanrooms-ml/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// Percent-encode a value for one URI path SEGMENT, the way an AWS SDK encodes a
/// non-greedy `@httpLabel`: everything outside the unreserved set is escaped, so the `/`
/// inside an ARN cannot be read as a segment boundary.
pub(crate) fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// What the emulator answered: the HTTP status and the JSON body.
///
/// `pub(crate)` because the sibling suites of this integration-test crate drive the
/// router through the same harness rather than copying it.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub(crate) fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    pub(crate) fn at(&self, member: &str) -> Value {
        self.body
            .pointer(&format!("/{member}"))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn text(&self, member: &str) -> String {
        self.at(member).as_str().unwrap_or_default().to_string()
    }
}

pub(crate) async fn signed_call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("cleanrooms-ml.{region}.amazonaws.com"))
        .header("authorization", authorization(region));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    signed_call(router, REGION, method, uri, body).await
}

async fn create_dataset(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/training-dataset", Some(request)).await
}

async fn get_dataset(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/training-dataset/{}", label(arn)),
        None,
    )
    .await
}

async fn delete_dataset(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/training-dataset/{}", label(arn)),
        None,
    )
    .await
}

async fn list_datasets(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/training-dataset{query}"), None).await
}

async fn create_model(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/audience-model", Some(request)).await
}

async fn get_model(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/audience-model/{}", label(arn)),
        None,
    )
    .await
}

async fn delete_model(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/audience-model/{}", label(arn)),
        None,
    )
    .await
}

/// One `Dataset`, the only member of the `trainingData` list
/// (`CreateTrainingDatasetRequest$trainingData` is bounded `@length(min: 1, max: 1)`).
/// `columns` picks how many `ColumnType` entries the schema carries, so a case can vary
/// the shape of the stored data without varying anything else.
fn training_data(table: &str, database: &str, catalog: Option<&str>, columns: usize) -> Value {
    let schema: Vec<Value> = COLUMN_TYPES
        .iter()
        .take(columns)
        .enumerate()
        .map(|(index, column_type)| {
            json!({
                "columnName": format!("column_{index}"),
                // `ColumnTypeList` is bounded @length(min: 1, max: 1).
                "columnTypes": [column_type],
            })
        })
        .collect();
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

/// A create request carrying only the three required members.
fn minimal_dataset(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "trainingData": [training_data("interactions", "clean_rooms_demo", None, 1)],
    })
}

/// True when `value` satisfies the ARN pattern both ARN shapes in this chain share,
/// `^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:<resource>/[-a-zA-Z0-9_/.]+$`.
///
/// Checked here rather than through the crate's own predicate, so a wrong predicate
/// cannot make this test pass.
pub(crate) fn matches_arn_pattern(value: &str, resource: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:cleanrooms-ml:{region}:{ACCOUNT}:{resource}/");
    let Some(id) = value.strip_prefix(&prefix) else {
        return false;
    };
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
}

/// The instant `value` names, in seconds since the Unix epoch.
///
/// A modelled `Timestamp` reaches the wire either as an epoch-seconds number or as the
/// RFC 3339 string `@timestampFormat("date-time")` asks for; which of the two the
/// protocol layer renders is not this service's choice, so every case here compares
/// INSTANTS and accepts both forms.
pub(crate) fn timestamp_seconds(value: &Value) -> f64 {
    if let Some(seconds) = value.as_f64() {
        return seconds;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("expected a timestamp, got {value:?}"));
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("expected an epoch-seconds number or an RFC 3339 instant ({error}): {text:?}")
        })
        .as_secs_f64()
}

/// True when `value` is a modelled `Timestamp` in either wire form.
pub(crate) fn is_timestamp(value: &Value) -> bool {
    value.as_f64().is_some()
        || value.as_str().is_some_and(|text| {
            ::aws_smithy_types::DateTime::from_str(
                text,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok()
        })
}

/// Now, in seconds since the Unix epoch — the window a create's `createTime` must fall
/// in.
pub(crate) fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the Unix epoch")
        .as_secs_f64()
}

/// Compare one member of a create request against the same member of a Get answer.
///
/// A timestamp member is compared as an instant (see [`timestamp_seconds`]); every other
/// member must come back exactly as it was stored.
pub(crate) fn assert_member_round_trips(case: &str, member: &str, expected: &Value, observed: &Value) {
    if member.ends_with("Time") {
        let expected_seconds = timestamp_seconds(expected);
        let observed_seconds = timestamp_seconds(observed);
        assert!(
            (expected_seconds - observed_seconds).abs() < 0.001,
            "case {case:?}: member {member:?} was stored as {expected:?} but is reported as \
             {observed:?}"
        );
        return;
    }
    assert_eq!(
        observed, expected,
        "case {case:?}: member {member:?} was stored as {expected:?} but is reported as \
         {observed:?}"
    );
}

/// The ARNs `ListTrainingDatasets` reported, in the order it reported them.
fn listed_arns(answer: &Answer) -> Vec<String> {
    answer.body["trainingDatasets"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "ListTrainingDatasets must return a trainingDatasets list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|summary| {
            summary["trainingDatasetArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every persisted training dataset, by following `nextToken` to the end.
async fn all_dataset_arns(router: &MultiServiceRouter) -> Vec<String> {
    let mut all = Vec::new();
    let mut token: Option<String> = None;
    // The store is bounded by `RESOURCE_QUOTA` and a page holds at least one record, so
    // this many pages is more than enough; the bound is what stops a token that never
    // terminates from hanging the test.
    for _ in 0..=RESOURCE_QUOTA {
        let query = match token.as_deref() {
            Some(token) => format!("?nextToken={}", label(token)),
            None => String::new(),
        };
        let answer = list_datasets(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "listing every dataset must answer HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        all.extend(listed_arns(&answer));
        token = answer
            .at("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            return all;
        }
    }
    panic!("following nextToken did not terminate within the store's bound");
}

/// `CreateTrainingDataset` persists what it was given and `GetTrainingDataset` reports
/// that record: for every input member, the reported value is the created one, the status
/// is the one status `TrainingDatasetStatus` models, and the ARN names the resource type,
/// the signed region and the emulator account.
///
/// The rows differ along what the rule generalises over — the name's length and
/// characters, whether a description or tags are there at all, how many tags, how many
/// schema columns and which `ColumnType`s, and whether the Glue source names a catalog.
#[tokio::test]
async fn a_created_training_dataset_is_reported_from_what_was_stored() {
    let longest_name = "n".repeat(63);
    let longest_description = "d".repeat(255);
    let cases: Vec<(&str, Value)> = vec![
        ("only the required members", minimal_dataset("ds-minimal")),
        (
            "described and tagged",
            json!({
                "name": "ds-described",
                "roleArn": ROLE_ARN,
                "trainingData": [training_data("events", "demo_db", None, 2)],
                "description": "bootstrap demo",
                "tags": {"env": "test"},
            }),
        ),
        (
            "the shortest name NameString allows",
            minimal_dataset("a"),
        ),
        (
            "the longest name NameString allows",
            minimal_dataset(&longest_name),
        ),
        (
            "a name carrying the inner spaces the pattern allows",
            minimal_dataset("demo ds 2026"),
        ),
        (
            "the longest description ResourceDescription allows",
            json!({
                "name": "ds-long-description",
                "roleArn": ROLE_ARN,
                "trainingData": [training_data("events", "demo_db", None, 1)],
                "description": longest_description,
            }),
        ),
        (
            "an empty tag map",
            json!({
                "name": "ds-no-tags",
                "roleArn": ROLE_ARN,
                "trainingData": [training_data("events", "demo_db", None, 1)],
                "tags": {},
            }),
        ),
        (
            "every ColumnType, a catalog id and three tags",
            json!({
                "name": "ds-full-schema",
                "roleArn": ROLE_ARN,
                "trainingData": [training_data(
                    "interactions 2026",
                    "clean_rooms_demo",
                    Some("210987654321"),
                    COLUMN_TYPES.len(),
                )],
                "description": "every column type",
                "tags": {"env": "test", "team": "ml", "cost-centre": "42"},
            }),
        ),
    ];

    let router = emulator();
    let mut minted = BTreeSet::new();
    for (case, request) in cases {
        let before = unix_now();
        let created = create_dataset(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateTrainingDataset, got {} with {:?}",
            created.status, created.body
        );
        let arn = created.text("trainingDatasetArn");
        assert!(
            matches_arn_pattern(&arn, "training-dataset", REGION),
            "case {case:?}: expected a training-dataset ARN in {REGION} for account \
             {ACCOUNT}, got {arn:?}"
        );
        assert!(
            minted.insert(arn.clone()),
            "case {case:?}: every create must mint a new ARN, got the earlier {arn:?}"
        );

        let got = get_dataset(&router, &arn).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected HTTP 200 from GetTrainingDataset, got {} with {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.at("trainingDatasetArn"),
            json!(arn),
            "case {case:?}: GetTrainingDataset must report the ARN it was asked about"
        );
        assert_eq!(
            got.at("status"),
            json!("ACTIVE"),
            "case {case:?}: TrainingDatasetStatus models ACTIVE and nothing else, got {:?}",
            got.at("status")
        );

        // Every member the create carried lands on the same member of the Get answer.
        // `trainingData` is the one member the create RESOLVES rather than echoes — the
        // Glue catalog a request leaves unnamed is the calling account's own, and a
        // column set is reported in column-name order — so it is compared against that
        // projection, which `training_dataset_input_config_projection` states and pins.
        for (member, expected) in request.as_object().expect("each case is a JSON object") {
            let expected = if member == "trainingData" {
                crate::training_dataset_input_config_projection::reported_training_data(expected)
            } else {
                expected.clone()
            };
            assert_member_round_trips(case, member, &expected, &got.at(member));
        }
        // A create that named no tags stored none, and that is what is reported.
        if request.get("tags").is_none() {
            assert_eq!(
                got.at("tags"),
                json!({}),
                "case {case:?}: a dataset created with no tags carries none, got {:?}",
                got.at("tags")
            );
        }
        // An absent optional member is reported as absent, not as an invented value.
        if request.get("description").is_none() {
            assert_eq!(
                got.at("description"),
                Value::Null,
                "case {case:?}: an unspecified description must not be invented, got {:?}",
                got.at("description")
            );
        }
        let after = unix_now();
        for member in ["createTime", "updateTime"] {
            let reported = got.at(member);
            assert!(
                is_timestamp(&reported),
                "case {case:?}: {member} must be a modelled timestamp, got {reported:?}"
            );
            let seconds = timestamp_seconds(&reported);
            assert!(
                (before..=after).contains(&seconds),
                "case {case:?}: {member} must be the instant the create ran, expected it in \
                 {before}..={after}, got {seconds}"
            );
        }
    }
}

/// A minted ARN names the region the request was signed for, and
/// `CreateTrainingDatasetRequest$name` is unique "in your account AND region": one name
/// is accepted once in every region, and refused the second time within a region.
#[tokio::test]
async fn a_name_is_unique_per_region_and_the_arn_names_that_region() {
    // Rows differ only in the signed region, which is what the rule generalises over.
    let cases = ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];
    // One name, used in every region.
    const NAME: &str = "one-name-per-region";

    let router = emulator();
    for region in cases {
        let created = signed_call(
            &router,
            region,
            "POST",
            "/training-dataset",
            Some(minimal_dataset(NAME)),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: a name taken in another region must still be free here, got \
             {} with {:?}",
            created.status, created.body
        );
        let arn = created.text("trainingDatasetArn");
        assert!(
            matches_arn_pattern(&arn, "training-dataset", region),
            "case {region:?}: the minted ARN must name the signed region, got {arn:?}"
        );
    }

    // And inside one region the second create under that name is refused.
    for region in cases {
        let refused = signed_call(
            &router,
            region,
            "POST",
            "/training-dataset",
            Some(minimal_dataset(NAME)),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {region:?}: a repeat inside one region must be refused with HTTP 400, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {region:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }
}

/// A Clean Rooms ML endpoint is regional: it reports the resources created under its own
/// region and no others, so the same store serves four regions four one-record views, and
/// a resource is invisible — not merely unmodifiable — through another region's endpoint.
#[tokio::test]
async fn an_endpoint_serves_only_its_own_regions_resources() {
    // Rows differ in the signed region, which is what the rule generalises over.
    let cases = ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];

    let router = emulator();
    let mut by_region = Vec::new();
    for region in cases {
        let dataset = signed_call(
            &router,
            region,
            "POST",
            "/training-dataset",
            Some(minimal_dataset("ds-regional")),
        )
        .await;
        assert_eq!(
            dataset.status, 200,
            "case {region:?}: the fixture create must succeed, got {} with {:?}",
            dataset.status, dataset.body
        );
        let dataset_arn = dataset.text("trainingDatasetArn");

        let model = signed_call(
            &router,
            region,
            "POST",
            "/audience-model",
            Some(json!({ "name": "am-regional", "trainingDatasetArn": dataset_arn })),
        )
        .await;
        assert_eq!(
            model.status, 200,
            "case {region:?}: the fixture model must be created, got {} with {:?}",
            model.status, model.body
        );
        by_region.push((region, dataset_arn, model.text("audienceModelArn")));
    }

    for (region, dataset_arn, model_arn) in &by_region {
        // The list this endpoint serves holds exactly its own dataset.
        let listed = signed_call(&router, region, "GET", "/training-dataset", None).await;
        assert_eq!(
            listed_arns(&listed),
            vec![dataset_arn.clone()],
            "case {region:?}: an endpoint lists its own region's datasets and no others"
        );

        for (other, other_dataset, other_model) in &by_region {
            if other == region {
                continue;
            }
            for (operation, method, uri) in [
                (
                    "GetTrainingDataset",
                    "GET",
                    format!("/training-dataset/{}", label(other_dataset)),
                ),
                (
                    "DeleteTrainingDataset",
                    "DELETE",
                    format!("/training-dataset/{}", label(other_dataset)),
                ),
                (
                    "GetAudienceModel",
                    "GET",
                    format!("/audience-model/{}", label(other_model)),
                ),
                (
                    "DeleteAudienceModel",
                    "DELETE",
                    format!("/audience-model/{}", label(other_model)),
                ),
            ] {
                let answer = signed_call(&router, region, method, &uri, None).await;
                assert_eq!(
                    answer.status, 404,
                    "case {region:?}: {operation} on {other:?}'s resource must answer HTTP \
                     404, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {region:?}: {operation} on {other:?}'s resource expected \
                     ResourceNotFoundException, got {:?}",
                    answer.body
                );
            }
        }

        // And the refused cross-region deletes released nothing: the owning endpoint
        // still serves both resources.
        let still_there = signed_call(
            &router,
            region,
            "GET",
            &format!("/audience-model/{}", label(model_arn)),
            None,
        )
        .await;
        assert_eq!(
            still_there.status, 200,
            "case {region:?}: a cross-region delete must not release the model, got {} \
             with {:?}",
            still_there.status, still_there.body
        );
    }
}

/// `name` "must be unique in your account and region": a second create under a name
/// already taken is refused, and it adds nothing to the store.
#[tokio::test]
async fn a_duplicate_training_dataset_name_is_refused() {
    // Rows differ along the name's shape, and along whether the repeat carries the same
    // body as the first create or a different one.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "the same request twice",
            "ds-dup".to_string(),
            minimal_dataset("ds-dup"),
        ),
        (
            "the same name with different training data",
            "ds-dup-2".to_string(),
            json!({
                "name": "ds-dup-2",
                "roleArn": ROLE_ARN,
                "trainingData": [training_data("other", "other_db", Some("210987654321"), 3)],
                "description": "a different body under a taken name",
            }),
        ),
        (
            "the shortest name NameString allows",
            "z".to_string(),
            minimal_dataset("z"),
        ),
        (
            "a name carrying inner spaces",
            "duplicate ds".to_string(),
            minimal_dataset("duplicate ds"),
        ),
    ];

    let router = emulator();
    for (case, name, repeat) in cases {
        let first = create_dataset(&router, minimal_dataset(&name)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create must succeed, got {} with {:?}",
            first.status, first.body
        );
        let arn = first.text("trainingDatasetArn");

        let second = create_dataset(&router, repeat).await;
        assert_eq!(
            second.status, 400,
            "case {case:?}: a duplicate name must be refused with HTTP 400, got {} with {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            second.body
        );

        let listed = listed_arns(&list_datasets(&router, "").await);
        assert_eq!(
            listed.iter().filter(|listed| *listed == &arn).count(),
            1,
            "case {case:?}: the refused create must add nothing to the store, got {listed:?}"
        );
    }
}

/// `ListTrainingDatasets` is `@paginated(items: trainingDatasets)`: whatever `maxResults`
/// it is given, following `nextToken` to the end enumerates every persisted dataset
/// exactly once, in one stable order, and no page is longer than was asked for.
#[tokio::test]
async fn list_training_datasets_pages_over_one_stable_order() {
    let router = emulator();
    let mut created = Vec::new();
    for index in 0..7 {
        let answer = create_dataset(&router, minimal_dataset(&format!("ds-{index}"))).await;
        assert_eq!(
            answer.status, 200,
            "the fixture create {index} must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        created.push(answer.text("trainingDatasetArn"));
    }

    let unpaged = listed_arns(&list_datasets(&router, "").await);
    let mut expected = created.clone();
    expected.sort();
    assert_eq!(
        unpaged, expected,
        "an unpaged list must report every persisted dataset in one stable order"
    );

    // Rows differ in the page size: none at all, one, a size that does not divide the
    // total, the exact total, and the ceiling the model admits.
    let cases: Vec<(&str, Option<i64>)> = vec![
        ("no maxResults at all", None),
        ("one at a time", Some(1)),
        ("two at a time", Some(2)),
        ("three at a time, which does not divide seven", Some(3)),
        ("exactly the number persisted", Some(7)),
        ("the ceiling the model admits", Some(MAX_RESULTS_CEILING)),
    ];

    for (case, max_results) in cases {
        let page_limit = max_results.unwrap_or(MAX_RESULTS_CEILING);
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more page than the smallest size could need, so a token that never
        // terminates fails here instead of hanging.
        for page in 0..=created.len() + 1 {
            let mut query = String::new();
            if let Some(max) = max_results {
                query.push_str(&format!("?maxResults={max}"));
            }
            if let Some(token) = token.as_deref() {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str(&format!("nextToken={}", label(token)));
            }
            let answer = list_datasets(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: page {page} expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page_arns = listed_arns(&answer);
            assert!(
                i64::try_from(page_arns.len()).expect("a page fits in i64") <= page_limit,
                "case {case:?}: page {page} is longer than {page_limit}, got {page_arns:?}"
            );
            walked.extend(page_arns);
            token = answer
                .at("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: paging must terminate, still holding {token:?} after \
             {} pages",
            created.len() + 2
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: following nextToken must enumerate every dataset exactly once, \
             in the unpaged order"
        );
    }
}

/// A `nextToken` `ListTrainingDatasets` never issued is refused rather than silently
/// serving the first page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    // Rows differ along how the token is wrong: not an ARN at all, an ARN of the wrong
    // resource type, a short account, an empty resource id, and a token for a resource
    // this operation does not page over.
    let cases = [
        ("not an ARN at all", "page-2"),
        (
            "an audience-model ARN",
            "arn:aws:cleanrooms-ml:eu-west-1:000000000000:audience-model/abc",
        ),
        (
            "an account id that is not twelve digits",
            "arn:aws:cleanrooms-ml:eu-west-1:0000:training-dataset/abc",
        ),
        (
            "an empty resource id",
            "arn:aws:cleanrooms-ml:eu-west-1:000000000000:training-dataset/",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::training-dataset/abc",
        ),
    ];

    let router = emulator();
    let created = create_dataset(&router, minimal_dataset("ds-token")).await;
    assert_eq!(
        created.status, 200,
        "the fixture create must succeed, got {} with {:?}",
        created.status, created.body
    );

    for (case, token) in cases {
        let answer = list_datasets(&router, &format!("?nextToken={}", label(token))).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unissued nextToken must be refused with HTTP 400, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // A token this operation DID issue the shape of — a training dataset ARN past every
    // persisted one — is honoured, and reports the empty tail.
    let tail = format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/zzzzzzzz");
    let answer = list_datasets(&router, &format!("?nextToken={}", label(&tail))).await;
    assert_eq!(
        answer.status, 200,
        "a well-formed token past the last dataset must be honoured, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        listed_arns(&answer).is_empty(),
        "a token past the last dataset reports no datasets, got {:?}",
        answer.body
    );
}

/// `CreateAudienceModel` persists what it was given, against a training dataset that must
/// exist, and `GetAudienceModel` reports that record.
///
/// The rows differ along which optional members are present — the training window, the
/// KMS key, the description and how many tags — and along the name's shape.
#[tokio::test]
async fn a_created_audience_model_is_reported_from_what_was_stored() {
    let router = emulator();
    let dataset = create_dataset(&router, minimal_dataset("ds-for-models")).await;
    assert_eq!(
        dataset.status, 200,
        "the fixture create must succeed, got {} with {:?}",
        dataset.status, dataset.body
    );
    let dataset_arn = dataset.text("trainingDatasetArn");

    let longest_name = "m".repeat(63);
    let cases: Vec<(&str, Value)> = vec![
        (
            "only the required members",
            json!({ "name": "am-minimal", "trainingDatasetArn": dataset_arn }),
        ),
        (
            "a training window, a KMS key, a description and tags",
            json!({
                "name": "am-full",
                "trainingDatasetArn": dataset_arn,
                // A modelled `Timestamp` crosses this protocol as epoch seconds (see
                // `timestamp_seconds`), so the window is stated in the form the
                // framework's restJson1 layer parses.
                "trainingDataStartTime": 1_767_225_600_i64,
                "trainingDataEndTime": 1_782_864_000_i64,
                "kmsKeyArn": KMS_KEY_ARN,
                "description": "similarity over the first half of the year",
                "tags": {"env": "test", "team": "ml"},
            }),
        ),
        (
            "a start of the window but no end",
            json!({
                "name": "am-open-ended",
                "trainingDatasetArn": dataset_arn,
                "trainingDataStartTime": 1_770_091_506_i64,
            }),
        ),
        (
            "the longest name NameString allows and an empty tag map",
            json!({
                "name": longest_name,
                "trainingDatasetArn": dataset_arn,
                "tags": {},
            }),
        ),
    ];

    for (case, request) in cases {
        let before = unix_now();
        let created = create_model(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateAudienceModel, got {} with {:?}",
            created.status, created.body
        );
        let arn = created.text("audienceModelArn");
        assert!(
            matches_arn_pattern(&arn, "audience-model", REGION),
            "case {case:?}: expected an audience-model ARN in {REGION} for account \
             {ACCOUNT}, got {arn:?}"
        );

        let got = get_model(&router, &arn).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected HTTP 200 from GetAudienceModel, got {} with {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.at("audienceModelArn"),
            json!(arn),
            "case {case:?}: GetAudienceModel must report the ARN it was asked about"
        );
        for (member, expected) in request.as_object().expect("each case is a JSON object") {
            let observed = got.at(member);
            // A timestamp member is compared as an INSTANT: a `date-time` member is
            // reported as an RFC 3339 string whichever of the two forms the request sent
            // it in. See [`assert_member_round_trips`].
            assert_member_round_trips(case, member, expected, &observed);
        }
        if request.get("kmsKeyArn").is_none() {
            assert_eq!(
                got.at("kmsKeyArn"),
                Value::Null,
                "case {case:?}: an unspecified kmsKeyArn must not be invented, got {:?}",
                got.at("kmsKeyArn")
            );
        }
        let after = unix_now();
        for member in ["createTime", "updateTime"] {
            let reported = got.at(member);
            assert!(
                is_timestamp(&reported),
                "case {case:?}: {member} must be a modelled timestamp, got {reported:?}"
            );
            let seconds = timestamp_seconds(&reported);
            assert!(
                (before..=after).contains(&seconds),
                "case {case:?}: {member} must be the instant the create ran, expected it in \
                 {before}..={after}, got {seconds}"
            );
        }
    }
}

/// An audience model is trained asynchronously: it carries a transient
/// `AudienceModelStatus` for the whole span the create started, and is `ACTIVE` only once
/// that span has passed — the same answer a caller that never polled would have seen.
#[tokio::test]
async fn an_audience_model_settles_to_active_after_its_training_span() {
    let router = emulator();
    let dataset = create_dataset(&router, minimal_dataset("ds-settling")).await;
    let dataset_arn = dataset.text("trainingDatasetArn");

    // Rows differ in how often the model is polled before the span passes, so a status
    // that moves because something READ it fails here.
    for (case, polls) in [("polled once", 1), ("polled four times", 4)] {
        let created = create_model(
            &router,
            json!({ "name": format!("am-{case}"), "trainingDatasetArn": dataset_arn }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = created.text("audienceModelArn");

        for poll in 0..polls {
            let got = get_model(&router, &arn).await;
            assert_eq!(
                got.at("status"),
                json!("CREATE_PENDING"),
                "case {case:?}: poll {poll} inside the training span must still report the \
                 transient status, got {:?}",
                got.at("status")
            );
        }

        tokio::time::sleep(AUDIENCE_MODEL_TRAINING + Duration::from_millis(50)).await;
        let got = get_model(&router, &arn).await;
        assert_eq!(
            got.at("status"),
            json!("ACTIVE"),
            "case {case:?}: once the training span has passed the model is ACTIVE, got {:?}",
            got.at("status")
        );
        // And it stays there: the settled status is persisted, not recomputed per call.
        let again = get_model(&router, &arn).await;
        assert_eq!(
            again.at("status"),
            json!("ACTIVE"),
            "case {case:?}: a settled model stays ACTIVE, got {:?}",
            again.at("status")
        );
    }
}

/// An ARN that names no persisted resource is `ResourceNotFoundException` (404) — on
/// every operation that takes one, whether it reports or releases.
#[tokio::test]
async fn an_arn_naming_no_resource_is_not_found() {
    let router = emulator();

    // Rows differ along the resource id and the region the ARN names: a well-formed ARN
    // for a resource that was never created, one from another region, and one whose id
    // carries the punctuation the pattern allows.
    let dataset_arns = [
        format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/missing"),
        format!("arn:aws:cleanrooms-ml:us-east-1:{ACCOUNT}:training-dataset/00000000-0000-0000-0000-000000000001"),
        format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:training-dataset/nested/id.v2_final"),
    ];
    let model_arns = [
        format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/missing"),
        format!("arn:aws:cleanrooms-ml:ap-southeast-2:{ACCOUNT}:audience-model/00000000-0000-0000-0000-000000000009"),
        format!("arn:aws:cleanrooms-ml:{REGION}:{ACCOUNT}:audience-model/nested/id.v2_final"),
    ];

    for arn in &dataset_arns {
        for (operation, answer) in [
            ("GetTrainingDataset", get_dataset(&router, arn).await),
            ("DeleteTrainingDataset", delete_dataset(&router, arn).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {operation} {arn:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation} {arn:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            // The refusal names the KIND of resource that was looked for and the
            // identifier the request carried for it: "<Kind> <identifier> does not
            // exist." is the wording of this class for every resource kind.
            assert_eq!(
                answer.at("message"),
                json!(format!("Training Dataset {arn} does not exist.")),
                "case {operation} {arn:?}: unexpected refusal wording, got {:?}",
                answer.at("message")
            );
        }
    }
    for arn in &model_arns {
        for (operation, answer) in [
            ("GetAudienceModel", get_model(&router, arn).await),
            ("DeleteAudienceModel", delete_model(&router, arn).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {operation} {arn:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation} {arn:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.at("message"),
                json!(format!("Audience Model {arn} does not exist.")),
                "case {operation} {arn:?}: unexpected refusal wording, got {:?}",
                answer.at("message")
            );
        }
    }

    // And a create against a training dataset that does not exist is the same 404: the
    // model cannot be trained from a dataset that is not there.
    for arn in &dataset_arns {
        let answer = create_model(
            &router,
            json!({ "name": "am-orphan", "trainingDatasetArn": arn }),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case CreateAudienceModel {arn:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case CreateAudienceModel {arn:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

/// "You can't delete a training dataset if there are any audience models that depend on
/// the training dataset": the delete is refused while a dependent is persisted, and
/// succeeds once every one of them is gone. A delete then reads back as not found, and
/// the list no longer reports it.
#[tokio::test]
async fn a_training_dataset_is_released_only_once_nothing_depends_on_it() {
    // Rows differ in how many audience models depend on the dataset, which is what the
    // rule generalises over.
    for dependents in [1usize, 2, 3] {
        let case = format!("{dependents} dependent audience model(s)");
        let router = emulator();
        let dataset = create_dataset(&router, minimal_dataset("ds-dependencies")).await;
        assert_eq!(
            dataset.status, 200,
            "case {case:?}: the fixture create must succeed, got {} with {:?}",
            dataset.status, dataset.body
        );
        let dataset_arn = dataset.text("trainingDatasetArn");

        let mut models = Vec::new();
        for index in 0..dependents {
            let created = create_model(
                &router,
                json!({
                    "name": format!("am-dependent-{index}"),
                    "trainingDatasetArn": dataset_arn,
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: fixture model {index} must be created, got {} with {:?}",
                created.status, created.body
            );
            models.push(created.text("audienceModelArn"));
        }

        // Every state with at least one dependent left refuses the delete.
        for (index, model) in models.iter().enumerate() {
            let refused = delete_dataset(&router, &dataset_arn).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?}: with {} dependent(s) left the delete must answer HTTP 409, \
                 got {} with {:?}",
                models.len() - index,
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                refused.body
            );
            // The refusal left the dataset exactly as it was.
            assert_eq!(
                get_dataset(&router, &dataset_arn).await.status,
                200,
                "case {case:?}: a refused delete must not release the dataset"
            );

            let deleted = delete_model(&router, model).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: DeleteAudienceModel {model:?} expected HTTP 200, got {} \
                 with {:?}",
                deleted.status, deleted.body
            );
            let gone = get_model(&router, model).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: a deleted model must read back as not found, got {} with {:?}",
                gone.status, gone.body
            );
        }

        let released = delete_dataset(&router, &dataset_arn).await;
        assert_eq!(
            released.status, 200,
            "case {case:?}: with nothing left depending on it the delete must answer HTTP \
             200, got {} with {:?}",
            released.status, released.body
        );
        let gone = get_dataset(&router, &dataset_arn).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted dataset must read back as not found, got {} with {:?}",
            gone.status, gone.body
        );
        assert!(
            listed_arns(&list_datasets(&router, "").await).is_empty(),
            "case {case:?}: a deleted dataset must not be listed any more"
        );
    }
}

/// The training dataset store is bounded: at the emulator's maximum a further create is
/// refused and adds nothing, and releasing one record frees room for exactly one more.
#[tokio::test]
async fn the_training_dataset_store_is_bounded() {
    let router = emulator();
    for index in 0..RESOURCE_QUOTA {
        let answer = create_dataset(&router, minimal_dataset(&format!("ds-quota-{index}"))).await;
        assert_eq!(
            answer.status, 200,
            "create {index} inside the bound must succeed, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // Rows differ in the request the over-quota create carries, so the refusal cannot be
    // a property of one body.
    let over_quota: Vec<(&str, Value)> = vec![
        ("a minimal request", minimal_dataset("ds-quota-over")),
        (
            "a described and tagged request",
            json!({
                "name": "ds-quota-over-2",
                "roleArn": ROLE_ARN,
                "trainingData": [training_data("events", "demo_db", Some("210987654321"), 3)],
                "description": "over the bound",
                "tags": {"env": "test"},
            }),
        ),
    ];
    for (case, request) in &over_quota {
        let refused = create_dataset(&router, request.clone()).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a create past the bound must be refused with HTTP 400, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
    }
    let held = all_dataset_arns(&router).await;
    assert_eq!(
        held.len(),
        RESOURCE_QUOTA,
        "a refused create must add nothing: the store must still hold exactly \
         {RESOURCE_QUOTA} datasets"
    );

    // Releasing one record frees room for exactly one more.
    let released = delete_dataset(&router, &held[0]).await;
    assert_eq!(
        released.status, 200,
        "releasing a dataset must succeed, got {} with {:?}",
        released.status, released.body
    );
    let accepted = create_dataset(&router, minimal_dataset("ds-quota-after-delete")).await;
    assert_eq!(
        accepted.status, 200,
        "with room freed the next create must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let full_again = create_dataset(&router, minimal_dataset("ds-quota-over-again")).await;
    assert_eq!(
        full_again.status, 400,
        "the bound holds again once the freed room is used, got {} with {:?}",
        full_again.status, full_again.body
    );
}

/// The audience model store is bounded the same way, and this operation DOES model the
/// refusal: `ServiceQuotaExceededException`, which reports the bound it hit.
#[tokio::test]
async fn the_audience_model_store_is_bounded() {
    let router = emulator();
    let dataset = create_dataset(&router, minimal_dataset("ds-for-quota")).await;
    assert_eq!(
        dataset.status, 200,
        "the fixture create must succeed, got {} with {:?}",
        dataset.status, dataset.body
    );
    let dataset_arn = dataset.text("trainingDatasetArn");

    let mut models = Vec::with_capacity(RESOURCE_QUOTA);
    for index in 0..RESOURCE_QUOTA {
        let answer = create_model(
            &router,
            json!({
                "name": format!("am-quota-{index}"),
                "trainingDatasetArn": dataset_arn,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "create {index} inside the bound must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        models.push(answer.text("audienceModelArn"));
    }

    // Rows differ in the request the over-quota create carries.
    let over_quota: Vec<(&str, Value)> = vec![
        (
            "a minimal request",
            json!({ "name": "am-quota-over", "trainingDatasetArn": dataset_arn }),
        ),
        (
            "a described and tagged request",
            json!({
                "name": "am-quota-over-2",
                "trainingDatasetArn": dataset_arn,
                "kmsKeyArn": KMS_KEY_ARN,
                "description": "over the bound",
                "tags": {"env": "test"},
            }),
        ),
    ];
    for (case, request) in &over_quota {
        let refused = create_model(&router, request.clone()).await;
        assert_eq!(
            refused.status, 402,
            "case {case:?}: a create past the bound must answer the modelled \
             ServiceQuotaExceededException status 402, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.at("quotaValue").as_f64(),
            Some(RESOURCE_QUOTA as f64),
            "case {case:?}: the refusal must report the bound it hit, got {:?}",
            refused.at("quotaValue")
        );
    }

    // Releasing one record frees room for exactly one more.
    let released = delete_model(&router, &models[0]).await;
    assert_eq!(
        released.status, 200,
        "releasing a model must succeed, got {} with {:?}",
        released.status, released.body
    );
    let accepted = create_model(
        &router,
        json!({ "name": "am-quota-after-delete", "trainingDatasetArn": dataset_arn }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "with room freed the next create must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let full_again = create_model(
        &router,
        json!({ "name": "am-quota-over-again", "trainingDatasetArn": dataset_arn }),
    )
    .await;
    assert_eq!(
        full_again.status, 402,
        "the bound holds again once the freed room is used, got {} with {:?}",
        full_again.status, full_again.body
    );
}
