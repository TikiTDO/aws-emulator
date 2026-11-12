// SPDX-License-Identifier: CC0-1.0

//! Q topics: the create and the read that reports it, over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so what is asserted is what a QuickSight SDK client would see:
//! the route, the status, and a body the handler computed from the shared store.
//!
//! The rule under test: a topic is identified by `(AwsAccountId, TopicId)`, `CreateTopic`
//! persists the definition it was given under that identity and reports the topic's ARN
//! plus the ARN of the refresh that indexes the data sets the definition associates, and
//! `DescribeTopic` answers from that stored record. So the rows below vary the inputs the
//! rule generalises over — the id's form, which optional members the definition carries,
//! how many data sets it associates, the user-experience version and the account — rather
//! than re-running one recorded call.

use serde_json::{json, Value};

use service_com_amazonaws_quicksight_quicksight_20180401::state as qs_state;

use super::data_source_data_set_lifecycle::Harness;

/// The account most rows use.
const ACCOUNT: &str = "000000000000";
/// A second, unrelated 12-digit account id, for the rows about account keying.
const OTHER_ACCOUNT: &str = "210987654321";
/// The region the test client signs for; it is the region in every ARN below.
const REGION: &str = "us-west-2";

/// The `TopicUserExperienceVersion` members. Catalog source:
/// `smithy_query(query="shape", shape="TopicUserExperienceVersion")`.
const USER_EXPERIENCE_VERSIONS: &[&str] = &["LEGACY", "NEW_READER_EXPERIENCE"];

fn topic_arn(account: &str, topic_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:topic/{topic_id}")
}

fn data_set_arn(account: &str, data_set_id: &str) -> String {
    format!("arn:aws:quicksight:{REGION}:{account}:dataset/{data_set_id}")
}

/// A one-entry `PhysicalTableMap` holding a relational table on `data_source_arn`, so a
/// row can create the data set a topic associates.
fn relational_table_map(data_source_arn: &str) -> Value {
    json!({
        "orders-table": {
            "RelationalTable": {
                "DataSourceArn": data_source_arn,
                "Schema": "public",
                "Name": "orders",
                "InputColumns": [
                    { "Name": "order_id", "Type": "INTEGER" },
                    { "Name": "revenue", "Type": "DECIMAL" },
                ],
            }
        }
    })
}

async fn create_topic(harness: &Harness, account: &str, body: Value) -> (u16, Value) {
    harness
        .call("POST", &format!("/accounts/{account}/topics"), Some(body))
        .await
}

async fn describe_topic(harness: &Harness, account: &str, topic_id: &str) -> (u16, Value) {
    harness
        .call("GET", &format!("/accounts/{account}/topics/{topic_id}"), None)
        .await
}

/// Create one data source and one data set under `account`, and answer the data set's
/// ARN — the ARN a topic's `DatasetMetadata` names.
async fn data_set_fixture(
    harness: &Harness,
    account: &str,
    data_source_id: &str,
    data_set_id: &str,
    import_mode: &str,
) -> String {
    let (status, body) = harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sources"),
            Some(json!({
                "DataSourceId": data_source_id,
                "Name": "fixture source",
                "Type": "ATHENA",
            })),
        )
        .await;
    assert_eq!(
        status, 202,
        "fixture {data_source_id}: create expected 202, got {status} with body {body}"
    );
    let source_arn = body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {data_source_id}: create returned no Arn: {body}"))
        .to_string();

    let (status, body) = harness
        .call(
            "POST",
            &format!("/accounts/{account}/data-sets"),
            Some(json!({
                "DataSetId": data_set_id,
                "Name": "fixture set",
                "ImportMode": import_mode,
                "PhysicalTableMap": relational_table_map(&source_arn),
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "fixture {data_set_id}: create expected 200, got {status} with body {body}"
    );
    body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture {data_set_id}: create returned no Arn: {body}"))
        .to_string()
}

/// Every member `expected` carries is present in `actual` with the same value, lists
/// compared element-wise and in order. A response may carry MORE members than the
/// request did — the model gives several topic members a default, and the service fills
/// a created child resource's ARN — so containment, not equality, is what "the stored
/// definition reads back" means.
///
/// `pub(crate)` because every suite in this test target compares a stored definition the
/// same way; a sibling suite reuses this one rather than keeping its own copy.
pub(crate) fn json_contains(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => expected
            .iter()
            .all(|(member, value)| actual.get(member).is_some_and(|got| json_contains(value, got))),
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual.iter())
                    .all(|(expected, actual)| json_contains(expected, actual))
        }
        _ => expected == actual,
    }
}

/// One `CreateTopic` and what the topic must report afterwards.
struct TopicCase {
    /// Case name, and the `TopicId` the row creates.
    topic_id: &'static str,
    /// The `Topic` (`TopicDetails`) the row submits.
    details: Value,
    /// The `CustomInstructions` text the row submits, or `None` to leave the optional
    /// member out.
    custom_instructions: Option<&'static str>,
    /// The folder the row asks the topic to reside in, or `None`.
    folder_arn: Option<&'static str>,
    /// The `Tags` the row submits, or `None`.
    tags: Option<Value>,
    /// How many data sets the definition associates: a topic that associates at least
    /// one is indexed by a refresh, and one that associates none has nothing to index.
    associated_data_sets: usize,
}

/// Rows spanning the id's form (plain, and every character class the model's `TopicId`
/// pattern `^[A-Za-z0-9-_.\+]*$` allows), the number of associated data sets (one, two,
/// none), both `TopicUserExperienceVersion` members, and a definition that carries no
/// optional member at all.
fn topic_cases(spice_data_set_arn: &str, direct_query_data_set_arn: &str) -> Vec<TopicCase> {
    vec![
        TopicCase {
            topic_id: "sales-insights",
            details: json!({
                "Name": "Sales insights",
                "Description": "Revenue questions for the sales org",
                "UserExperienceVersion": "NEW_READER_EXPERIENCE",
                "ConfigOptions": { "QBusinessInsightsEnabled": true },
                "DataSets": [{
                    "DatasetArn": spice_data_set_arn,
                    "DatasetName": "orders",
                    "DatasetDescription": "one row per order",
                    "DataAggregation": {
                        "DatasetRowDateGranularity": "DAY",
                        "DefaultDateColumnName": "order_date",
                    },
                    "Columns": [{
                        "ColumnName": "revenue",
                        "ColumnFriendlyName": "Revenue",
                        "ColumnDataRole": "MEASURE",
                        "Aggregation": "SUM",
                        "ColumnSynonyms": ["sales", "turnover"],
                    }],
                    "CalculatedFields": [{
                        "CalculatedFieldName": "revenue_per_order",
                        "Expression": "sum({revenue}) / distinct_count({order_id})",
                    }],
                    "NamedEntities": [{
                        "EntityName": "revenue",
                        "EntitySynonyms": ["top line"],
                    }],
                    "Filters": [{
                        "FilterName": "high value",
                        "OperandFieldName": "revenue",
                        "FilterType": "NUMERIC_EQUALITY_FILTER",
                        "NumericEqualityFilter": { "Constant": { "ConstantType": "SINGULAR" } },
                    }],
                }],
            }),
            custom_instructions: Some("Answer in whole dollars."),
            folder_arn: Some("arn:aws:quicksight:us-west-2:000000000000:folder/sales"),
            tags: Some(json!([{ "Key": "team", "Value": "sales" }])),
            associated_data_sets: 1,
        },
        TopicCase {
            topic_id: "ops.metrics+weekly_v2-1",
            details: json!({
                "Name": "Ops metrics",
                "UserExperienceVersion": "LEGACY",
                "DataSets": [
                    { "DatasetArn": direct_query_data_set_arn, "DatasetName": "inventory" },
                    { "DatasetArn": spice_data_set_arn, "DatasetName": "orders" },
                ],
            }),
            custom_instructions: None,
            folder_arn: None,
            tags: None,
            associated_data_sets: 2,
        },
        TopicCase {
            topic_id: "no-data-sets",
            details: json!({
                "Name": "Nothing associated yet",
                "Description": "a topic whose data sets come later",
            }),
            custom_instructions: Some("Prefer monthly grain."),
            folder_arn: None,
            tags: None,
            associated_data_sets: 0,
        },
        TopicCase {
            topic_id: "bare",
            details: json!({}),
            custom_instructions: None,
            folder_arn: None,
            tags: None,
            associated_data_sets: 0,
        },
    ]
}

/// The `CreateTopic` request body one case sends.
fn create_body(case: &TopicCase) -> Value {
    let mut body = json!({ "TopicId": case.topic_id, "Topic": case.details });
    if let Some(instructions) = case.custom_instructions {
        body["CustomInstructions"] = json!({ "CustomInstructionsString": instructions });
    }
    if let Some(folder_arn) = case.folder_arn {
        body["FolderArns"] = json!([folder_arn]);
    }
    if let Some(tags) = &case.tags {
        body["Tags"] = tags.clone();
    }
    body
}

#[tokio::test]
async fn a_created_topic_reads_back_for_every_kind_of_definition() {
    let harness = Harness::new();
    let spice_arn = data_set_fixture(&harness, ACCOUNT, "ds-orders", "orders", "SPICE").await;
    let direct_query_arn =
        data_set_fixture(&harness, ACCOUNT, "ds-inventory", "inventory", "DIRECT_QUERY").await;

    let cases = topic_cases(&spice_arn, &direct_query_arn);
    let mut request_ids: Vec<String> = Vec::new();
    let mut refresh_arns: Vec<String> = Vec::new();

    for case in &cases {
        let topic_id = case.topic_id;
        let (status, body) = create_topic(&harness, ACCOUNT, create_body(case)).await;
        assert_eq!(
            status, 200,
            "case {topic_id}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(topic_arn(ACCOUNT, topic_id)),
            "case {topic_id}: create expected the account+id derived ARN, got {body}"
        );
        assert_eq!(
            body["TopicId"],
            json!(topic_id),
            "case {topic_id}: create echoed the wrong id: {body}"
        );
        // `Status` is `@httpResponseCode`-bound, so it IS the HTTP status asserted
        // above rather than a body member.
        assert_eq!(
            body["Status"],
            Value::Null,
            "case {topic_id}: Status is bound to the HTTP status, so it is not a body member: {body}"
        );
        let request_id = body["RequestId"].as_str().unwrap_or_default().to_string();
        assert!(
            !request_id.is_empty(),
            "case {topic_id}: create expected a RequestId, got {body}"
        );
        request_ids.push(request_id);

        let refresh_arn = body["RefreshArn"].as_str().map(str::to_string);
        if case.associated_data_sets == 0 {
            assert_eq!(
                refresh_arn, None,
                "case {topic_id}: a topic associating no data set has nothing to index, so it must report no RefreshArn: {body}"
            );
        } else {
            let refresh_arn = refresh_arn.unwrap_or_else(|| {
                panic!("case {topic_id}: create expected a RefreshArn, got {body}")
            });
            let prefix = format!("{}/refresh/", topic_arn(ACCOUNT, topic_id));
            assert!(
                refresh_arn.starts_with(&prefix),
                "case {topic_id}: expected a RefreshArn under {prefix}, got {refresh_arn:?}"
            );
            assert!(
                refresh_arn.len() > prefix.len(),
                "case {topic_id}: expected a RefreshArn naming a refresh id, got {refresh_arn:?}"
            );
            refresh_arns.push(refresh_arn);
        }
    }

    // Every refresh the creates minted names a distinct object.
    let mut distinct = refresh_arns.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        refresh_arns.len(),
        "each create must mint its own refresh, got {refresh_arns:?}"
    );

    for case in &cases {
        let topic_id = case.topic_id;
        let (status, body) = describe_topic(&harness, ACCOUNT, topic_id).await;
        assert_eq!(
            status, 200,
            "case {topic_id}: describe expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(topic_arn(ACCOUNT, topic_id)),
            "case {topic_id}: describe expected the create's ARN, got {body}"
        );
        assert_eq!(
            body["TopicId"],
            json!(topic_id),
            "case {topic_id}: describe reported the wrong id: {body}"
        );
        assert!(
            json_contains(&case.details, &body["Topic"]),
            "case {topic_id}: expected the stored definition {}, got {}",
            case.details,
            body["Topic"]
        );
        match case.custom_instructions {
            Some(instructions) => assert_eq!(
                body["CustomInstructions"]["CustomInstructionsString"],
                json!(instructions),
                "case {topic_id}: describe expected the stored custom instructions, got {body}"
            ),
            None => assert_eq!(
                body["CustomInstructions"],
                Value::Null,
                "case {topic_id}: a topic created without custom instructions must report none, got {body}"
            ),
        }
        assert_eq!(
            body["Topic"]["DataSets"]
                .as_array()
                .map_or(0, |data_sets| data_sets.len()),
            case.associated_data_sets,
            "case {topic_id}: expected {} associated data set(s), got {}",
            case.associated_data_sets,
            body["Topic"]
        );
    }

    // The store holds one record per created topic, with the folder each asked for.
    let state = qs_state::service_state(&*harness.engine).expect("the QuickSight store");
    let store = state.lock();
    for case in &cases {
        let topic_id = case.topic_id;
        let record = store
            .topics
            .get(&(ACCOUNT.to_string(), topic_id.to_string()))
            .unwrap_or_else(|| panic!("case {topic_id}: the store holds no such topic"));
        assert_eq!(
            record.folder_arns,
            case.folder_arn.map(str::to_string).into_iter().collect::<Vec<String>>(),
            "case {topic_id}: expected the submitted FolderArns, got {:?}",
            record.folder_arns
        );
        assert_eq!(
            record.tags.len(),
            case.tags
                .as_ref()
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            "case {topic_id}: expected the submitted Tags, got {:?}",
            record.tags
        );
    }

    // Every response reported its own request id.
    request_ids.sort();
    let minted = request_ids.len();
    request_ids.dedup();
    assert_eq!(
        request_ids.len(),
        minted,
        "each create must report its own RequestId, got {request_ids:?}"
    );
}

#[tokio::test]
async fn a_topic_is_identified_by_account_and_topic_id() {
    let harness = Harness::new();
    // The SAME TopicId under two accounts: two topics, each reporting its own
    // definition, and neither visible from a third account.
    for (account, name) in [(ACCOUNT, "first account"), (OTHER_ACCOUNT, "second account")] {
        let (status, body) = create_topic(
            &harness,
            account,
            json!({ "TopicId": "shared-id", "Topic": { "Name": name } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account}: create expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Arn"],
            json!(topic_arn(account, "shared-id")),
            "case {account}: create expected an ARN in its own account, got {body}"
        );
    }

    for (account, name) in [(ACCOUNT, "first account"), (OTHER_ACCOUNT, "second account")] {
        let (status, body) = describe_topic(&harness, account, "shared-id").await;
        assert_eq!(
            status, 200,
            "case {account}: describe expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Topic"]["Name"],
            json!(name),
            "case {account}: describe expected that account's own topic, got {body}"
        );
    }

    let (status, body) = describe_topic(&harness, "555555555555", "shared-id").await;
    assert_eq!(
        status, 404,
        "case third account: another account must not see the topic, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceNotFoundException"),
        "case third account: expected ResourceNotFoundException, got {body}"
    );
}

#[tokio::test]
async fn every_user_experience_version_is_accepted_and_nothing_else_is() {
    let harness = Harness::new();

    for version in USER_EXPERIENCE_VERSIONS {
        let topic_id = format!("ux-{}", version.to_lowercase());
        let (status, body) = create_topic(
            &harness,
            ACCOUNT,
            json!({
                "TopicId": topic_id,
                "Topic": { "Name": "version row", "UserExperienceVersion": version },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {version}: create expected 200, got {status} with body {body}"
        );
        let (status, body) = describe_topic(&harness, ACCOUNT, &topic_id).await;
        assert_eq!(
            status, 200,
            "case {version}: describe expected 200, got {status} with body {body}"
        );
        assert_eq!(
            body["Topic"]["UserExperienceVersion"],
            json!(version),
            "case {version}: describe expected the stored version, got {body}"
        );
    }

    // A value just outside the catalog, and one that differs only in case. The member
    // is a modelled enum, so the framework refuses it before the handler runs.
    for version in ["NEW_WRITER_EXPERIENCE", "legacy"] {
        let (status, body) = create_topic(
            &harness,
            ACCOUNT,
            json!({
                "TopicId": "ux-refused",
                "Topic": { "Name": "version row", "UserExperienceVersion": version },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {version}: expected 400 for a value outside the enum, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {version}: expected ValidationException, got {body}"
        );
        let (status, body) = describe_topic(&harness, ACCOUNT, "ux-refused").await;
        assert_eq!(
            status, 404,
            "case {version}: a refused create must store nothing, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_create_is_refused_rather_than_overwriting_or_inventing_a_data_set() {
    let harness = Harness::new();
    let spice_arn = data_set_fixture(&harness, ACCOUNT, "ds-orders", "orders", "SPICE").await;

    let (status, body) = create_topic(
        &harness,
        ACCOUNT,
        json!({ "TopicId": "taken", "Topic": { "Name": "The first one" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the first create expected 200, got {status} with body {body}"
    );

    let (status, body) = create_topic(
        &harness,
        ACCOUNT,
        json!({ "TopicId": "taken", "Topic": { "Name": "The second one" } }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case duplicate id: expected 409, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ResourceExistsException"),
        "case duplicate id: expected ResourceExistsException, got {body}"
    );
    let (_, body) = describe_topic(&harness, ACCOUNT, "taken").await;
    assert_eq!(
        body["Topic"]["Name"],
        json!("The first one"),
        "case duplicate id: the refused create must not overwrite the stored topic, got {body}"
    );

    // An empty `TopicId` matches the model's pattern, but no route addresses the topic
    // it would create (`/accounts/{AwsAccountId}/topics/{TopicId}`), so the create is
    // refused rather than storing a topic nothing can read.
    let (status, body) = create_topic(
        &harness,
        ACCOUNT,
        json!({ "TopicId": "", "Topic": { "Name": "Nameless" } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case empty id: expected 400, got {status} with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("InvalidParameterValueException"),
        "case empty id: expected InvalidParameterValueException, got {body}"
    );

    // Rows: a `DatasetArn` no data set in this account has — absent altogether, another
    // account's, another region's, another service's, and one that is not an ARN.
    let unresolvable: [(&str, String); 5] = [
        ("absent", data_set_arn(ACCOUNT, "no-such-set")),
        ("another account", data_set_arn(OTHER_ACCOUNT, "orders")),
        (
            "another region",
            format!("arn:aws:quicksight:eu-west-1:{ACCOUNT}:dataset/orders"),
        ),
        (
            "another service",
            format!("arn:aws:glue:{REGION}:{ACCOUNT}:dataset/orders"),
        ),
        ("not an ARN", "orders".to_string()),
    ];
    for (case, arn) in &unresolvable {
        let (status, body) = create_topic(
            &harness,
            ACCOUNT,
            json!({
                "TopicId": "topic-bad-set",
                "Topic": {
                    "Name": "Unresolvable",
                    // The first association resolves, so what the row varies is the
                    // second one.
                    "DataSets": [
                        { "DatasetArn": spice_arn },
                        { "DatasetArn": arn },
                    ],
                },
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404 for an unresolvable DatasetArn, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {body}"
        );
        let (status, body) = describe_topic(&harness, ACCOUNT, "topic-bad-set").await;
        assert_eq!(
            status, 404,
            "case {case}: a refused create must store nothing, got {status} with {body}"
        );
    }
}
