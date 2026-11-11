// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CLOUDWATCH half of Contributor Insights for DynamoDB: the insight
//! rules an enable mints, and the key activity the data plane attributes to them.
//!
//! The rules pinned here:
//!
//! 1. an accepted `ENABLE` creates, in CloudWatch's insight-rule store, one rule per key kind
//!    the mode in force and the resource's key schema call for — the accessed half only in the
//!    mode that tracks all access, the sort-key kinds only for a table that declares a sort key
//!    — each named `DynamoDBContributorInsights-<kind>-<table>-<epoch millis>` with ONE suffix
//!    for the whole enable, and `DescribeContributorInsights` lists exactly those names;
//! 2. every access that names an item's key contributes to the rule collecting its measure,
//!    keyed by the partition key it addressed: a read or a write contributes the capacity it
//!    consumed, so `GetInsightRuleReport` ranks the table's keys by how much they were accessed,
//!    with `KeyLabels` naming the key the contributors are classified by;
//! 3. a `DISABLE` deletes exactly that resource's rules — another table's keep collecting — so a
//!    report asked for a deleted rule is refused `InvalidParameterValueException` with the
//!    coded sentence CloudWatch states a bad rule name with.
//!
//! Each rule is asserted over a SET of inputs: the modes, both key schemas, several kinds of
//! access (consistent and eventually consistent reads, a write, a query) and several keys of
//! one table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call_cloudwatch, describe_when_active, emulator_with_cloudwatch, error_code, message, ok,
    ok_cloudwatch,
};

/// The prefix DynamoDB mints its rule names under, and the kinds it spells them with, read from
/// the crate that mints them rather than written out here.
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::contributor_insights as insights;

/// The period a report's datapoints are aggregated over: the one-minute granularity Contributor
/// Insights folds contributor data at.
const PERIOD_SECS: i64 = 60;

/// Epoch seconds, the unit a report's window is expressed in.
fn now_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs(),
    )
    .expect("the epoch seconds fit in an i64")
}

/// Create one on-demand table in the suite's Region, with a sort key when `sort_key` says so.
async fn create_table(router: &MultiServiceRouter, case: &str, name: &str, sort_key: bool) {
    let mut key_schema = vec![json!({"AttributeName": "Part", "KeyType": "HASH"})];
    let mut attributes = vec![json!({"AttributeName": "Part", "AttributeType": "S"})];
    if sort_key {
        key_schema.push(json!({"AttributeName": "Sort", "KeyType": "RANGE"}));
        attributes.push(json!({"AttributeName": "Sort", "AttributeType": "S"}));
    }
    let created = ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": key_schema,
            "AttributeDefinitions": attributes,
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        created["TableDescription"]["TableName"],
        json!(name),
        "case {case:?}: the create must answer for the table it made, got {created}"
    );
    // The data plane serves an ACTIVE table only, so the suite waits the create out exactly as
    // a client's waiter does.
    describe_when_active(router, case, name).await;
}

/// One `UpdateContributorInsights` in the suite's Region.
async fn set_insights(
    router: &MultiServiceRouter,
    table: &str,
    action: &str,
    mode: Option<&str>,
) -> Value {
    let mut request = json!({"TableName": table, "ContributorInsightsAction": action});
    if let Some(mode) = mode {
        request["ContributorInsightsMode"] = json!(mode);
    }
    ok(router, "UpdateContributorInsights", request).await
}

/// Every Contributor Insights rule name CloudWatch holds, ascending — paged through
/// `NextToken`, the way a client reads the whole list.
async fn described_rule_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    // Bounded: a page always carries at least one rule, so a listing that never terminates is a
    // defect rather than something to loop on.
    for _ in 0..32 {
        let mut request = json!({});
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let page = ok_cloudwatch(router, "DescribeInsightRules", request).await;
        let rules = page["InsightRules"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a listing answers its rules, got {page}"));
        names.extend(
            rules
                .iter()
                .filter_map(|rule| rule["Name"].as_str().map(ToString::to_string)),
        );
        next = page["NextToken"].as_str().map(ToString::to_string);
        if next.is_none() {
            names.sort();
            return names;
        }
    }
    panic!("case {case:?}: DescribeInsightRules never finished paging")
}

/// The rule CloudWatch holds under `name`, as `DescribeInsightRules` reports it.
async fn described_rule(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let mut next: Option<String> = None;
    for _ in 0..32 {
        let mut request = json!({});
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let page = ok_cloudwatch(router, "DescribeInsightRules", request).await;
        if let Some(found) = page["InsightRules"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .find(|rule| rule["Name"] == json!(name))
        {
            return found.clone();
        }
        next = page["NextToken"].as_str().map(ToString::to_string);
        if next.is_none() {
            panic!("case {case:?}: no rule named {name:?} is listed");
        }
    }
    panic!("case {case:?}: DescribeInsightRules never finished paging")
}

/// The names among `names` that are the rules of `table`.
fn rules_of(names: &[String], table: &str) -> Vec<String> {
    names
        .iter()
        .filter(|name| name.contains(&format!("-{table}-")))
        .cloned()
        .collect()
}

/// The kind one minted rule name spells: the segment between the service's prefix and the
/// resource the rule collects for.
fn kind_of(name: &str) -> String {
    name.strip_prefix(&format!("{}-", insights::RULE_NAME_PREFIX))
        .and_then(|rest| rest.split('-').next())
        .unwrap_or_default()
        .to_string()
}

/// The kind each of `names` spells, ascending.
fn kinds_in(names: &[String]) -> Vec<String> {
    let mut kinds: Vec<String> = names.iter().map(|name| kind_of(name)).collect();
    kinds.sort();
    kinds
}

/// The kind each rule name of `table` spells, ascending.
fn kinds_of(names: &[String], table: &str) -> Vec<String> {
    kinds_in(&rules_of(names, table))
}

/// The epoch-millis suffix a rule name ends with.
fn suffix_of(name: &str) -> String {
    name.rsplit('-').next().unwrap_or_default().to_string()
}

/// The report for `rule` over a window around now, ranked by `Sum`.
async fn report(router: &MultiServiceRouter, rule: &str) -> Value {
    let end = now_secs() + PERIOD_SECS;
    ok_cloudwatch(
        router,
        "GetInsightRuleReport",
        json!({
            "RuleName": rule,
            "StartTime": end - 3 * PERIOD_SECS,
            "EndTime": end,
            "Period": PERIOD_SECS,
            "MaxContributorCount": 20,
            "OrderBy": "Sum",
            "Metrics": ["Sum", "SampleCount", "UniqueContributors"],
        }),
    )
    .await
}

/// The `(keys joined, ApproximateAggregateValue)` pairs a report lists, in order.
fn reported_contributors(report: &Value) -> Vec<(String, f64)> {
    report["Contributors"]
        .as_array()
        .map(|contributors| {
            contributors
                .iter()
                .map(|contributor| {
                    let keys = contributor["Keys"]
                        .as_array()
                        .map(|keys| {
                            keys.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                        .unwrap_or_default();
                    (
                        keys,
                        contributor["ApproximateAggregateValue"]
                            .as_f64()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the minting table: the resource's key schema, the mode the enable names, and the
/// rule kinds it must mint.
struct MintRow {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// Whether the table declares a sort key.
    sort_key: bool,
    /// The mode the enable names, or `None` for an enable that names none.
    mode: Option<&'static str>,
    /// The kinds the enable must mint, ascending.
    kinds: &'static [&'static str],
}

#[tokio::test]
async fn enabling_contributor_insights_mints_the_mode_s_rules_for_the_table_s_key_schema() {
    let router = emulator_with_cloudwatch();
    let rows: Vec<MintRow> = vec![
        MintRow {
            case: "a hash-only table tracking all access and throttled events",
            sort_key: false,
            mode: Some("ACCESSED_AND_THROTTLED_KEYS"),
            kinds: &[
                insights::RULE_KIND_PARTITION_ACCESSED,
                insights::RULE_KIND_PARTITION_THROTTLED,
            ],
        },
        MintRow {
            case: "a hash-only table tracking throttled events only",
            sort_key: false,
            mode: Some("THROTTLED_KEYS"),
            kinds: &[insights::RULE_KIND_PARTITION_THROTTLED],
        },
        MintRow {
            case: "a table with a sort key tracking all access and throttled events",
            sort_key: true,
            mode: Some("ACCESSED_AND_THROTTLED_KEYS"),
            kinds: &[
                insights::RULE_KIND_PARTITION_ACCESSED,
                insights::RULE_KIND_PARTITION_THROTTLED,
                insights::RULE_KIND_SORT_ACCESSED,
                insights::RULE_KIND_SORT_THROTTLED,
            ],
        },
        MintRow {
            case: "a table with a sort key tracking throttled events only",
            sort_key: true,
            mode: Some("THROTTLED_KEYS"),
            kinds: &[
                insights::RULE_KIND_PARTITION_THROTTLED,
                insights::RULE_KIND_SORT_THROTTLED,
            ],
        },
        MintRow {
            case: "an enable that names no mode, which tracks neither half less",
            sort_key: false,
            mode: None,
            kinds: &[
                insights::RULE_KIND_PARTITION_ACCESSED,
                insights::RULE_KIND_PARTITION_THROTTLED,
            ],
        },
    ];

    let mut enabled: Vec<(&'static str, String, Vec<String>)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let table = format!("insights-mint-{index}");
        create_table(&router, case, &table, row.sort_key).await;
        set_insights(&router, &table, "ENABLE", row.mode).await;

        let names = described_rule_names(&router, case).await;
        let minted = rules_of(&names, &table);
        let mut expected_kinds: Vec<String> =
            row.kinds.iter().map(|kind| (*kind).to_string()).collect();
        expected_kinds.sort();
        let kinds = kinds_of(&names, &table);
        assert_eq!(
            kinds, expected_kinds,
            "case {case:?}: expected the rule kinds {expected_kinds:?}, got {kinds:?} from \
             {minted:?}"
        );
        // One enable, one creation instant: every rule it minted carries the same suffix.
        let suffixes: std::collections::BTreeSet<String> =
            minted.iter().map(|name| suffix_of(name)).collect();
        assert_eq!(
            suffixes.len(),
            1,
            "case {case:?}: the rules of one enable share one suffix, got {minted:?}"
        );
        let suffix = suffixes.iter().next().cloned().unwrap_or_default();
        assert!(
            suffix.parse::<u64>().is_ok(),
            "case {case:?}: the suffix is the enable's instant in epoch millis, got {suffix:?}"
        );
        for name in &minted {
            assert!(
                name.starts_with(&format!("{}-", insights::RULE_NAME_PREFIX)),
                "case {case:?}: every minted name carries the service's prefix, got {name:?}"
            );
            // The rule a client reads is one Amazon Web Services manages, in the state that
            // collects, and its definition names the DynamoDB table whose key activity it
            // graphs and the measure it ranks its keys by.
            let rule = described_rule(&router, case, name).await;
            let definition: Value = rule["Definition"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(Value::Null);
            assert_eq!(
                (
                    rule["State"].clone(),
                    rule["ManagedRule"].clone(),
                    definition["TableName"].clone()
                ),
                (json!("ENABLED"), json!(true), json!(table)),
                "case {case:?}: the rule {name:?} is a managed rule collecting for the table, got \
                 {rule}"
            );
            let measure = if name.contains(&format!("-{}-", insights::RULE_KIND_PARTITION_ACCESSED))
                || name.contains(&format!("-{}-", insights::RULE_KIND_SORT_ACCESSED))
            {
                insights::MEASURE_CONSUMED_THROUGHPUT_UNITS
            } else {
                insights::MEASURE_THROTTLE_COUNT
            };
            assert_eq!(
                definition["Contribution"]["ValueOf"],
                json!(measure),
                "case {case:?}: the rule {name:?} aggregates the measure its kind collects, got \
                 {definition}"
            );
        }

        // The resource reports the rules the store holds for it, not a list of its own.
        let described = ok(
            &router,
            "DescribeContributorInsights",
            json!({"TableName": table}),
        )
        .await;
        let mut reported: Vec<String> = described["ContributorInsightsRuleList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the read answers its rule list, got {described}")
            })
            .iter()
            .filter_map(|name| name.as_str().map(ToString::to_string))
            .collect();
        reported.sort();
        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(
            reported, sorted,
            "case {case:?}: expected the rule list {sorted:?}, got {reported:?}"
        );
        enabled.push((case, table, minted));
    }

    // A DISABLE deletes exactly that table's rules: every other table's keep collecting, and a
    // report asked for a deleted rule is refused for the NAME it carried.
    let (case, table, minted) = enabled
        .first()
        .cloned()
        .expect("the table has at least one row");
    set_insights(&router, &table, "DISABLE", None).await;
    let names = described_rule_names(&router, case).await;
    assert!(
        rules_of(&names, &table).is_empty(),
        "case {case:?}: a disable deletes the table's rules, got {:?}",
        rules_of(&names, &table)
    );
    for (other_case, other_table, other_minted) in enabled.iter().skip(1) {
        let mut still_there = rules_of(&names, other_table);
        still_there.sort();
        let mut expected = other_minted.clone();
        expected.sort();
        assert_eq!(
            still_there, expected,
            "case {other_case:?}: another table's disable leaves its rules alone, expected \
             {expected:?}, got {still_there:?}"
        );
    }
    let described = ok(
        &router,
        "DescribeContributorInsights",
        json!({"TableName": table}),
    )
    .await;
    assert_eq!(
        described["ContributorInsightsRuleList"],
        Value::Null,
        "case {case:?}: a disabled resource names no rule, got {described}"
    );
    for deleted in &minted {
        let (status, body) = call_cloudwatch(
            &router,
            "GetInsightRuleReport",
            json!({
                "RuleName": deleted,
                "StartTime": now_secs() - PERIOD_SECS,
                "EndTime": now_secs() + PERIOD_SECS,
                "Period": PERIOD_SECS,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "InvalidParameterValueException".to_string(),
                format!("RULE_NAME: {deleted}")
            ),
            "case {case:?}: a report of the deleted rule {deleted:?} is refused for the name it \
             carried, got {status} with {body}"
        );
    }

    // A second enable of a resource that is already collecting keeps the rules it has: the same
    // mode again mints nothing, and a WIDER mode adds only the kinds the narrower one had none
    // of — so the rules a client already graphs keep their names and what they collected.
    let (case, table, before) = enabled
        .get(1)
        .cloned()
        .expect("the table has a throttled-keys row");
    set_insights(&router, &table, "ENABLE", Some("THROTTLED_KEYS")).await;
    let mut again = rules_of(&described_rule_names(&router, case).await, &table);
    again.sort();
    let mut expected = before.clone();
    expected.sort();
    assert_eq!(
        again, expected,
        "case {case:?}: re-stating the mode mints nothing further, expected {expected:?}, got \
         {again:?}"
    );
    set_insights(&router, &table, "ENABLE", Some("ACCESSED_AND_THROTTLED_KEYS")).await;
    let widened = rules_of(&described_rule_names(&router, case).await, &table);
    assert_eq!(
        kinds_of(&widened, &table),
        vec![
            insights::RULE_KIND_PARTITION_ACCESSED.to_string(),
            insights::RULE_KIND_PARTITION_THROTTLED.to_string(),
        ],
        "case {case:?}: widening the mode adds the accessed half, got {widened:?}"
    );
    for kept in &before {
        assert!(
            widened.contains(kept),
            "case {case:?}: the rule {kept:?} the first enable minted is kept, got {widened:?}"
        );
    }
    // NARROWING the mode is the same rule read the other way: the mode says which halves the
    // resource collects, so the kinds it no longer tracks stop existing while the kinds it still
    // tracks keep their rules.
    set_insights(&router, &table, "ENABLE", Some("THROTTLED_KEYS")).await;
    let narrowed = rules_of(&described_rule_names(&router, case).await, &table);
    assert_eq!(
        (kinds_of(&narrowed, &table), narrowed.clone()),
        (
            vec![insights::RULE_KIND_PARTITION_THROTTLED.to_string()],
            before.clone()
        ),
        "case {case:?}: narrowing the mode leaves only the throttled half, under the names it \
         already had, got {narrowed:?}"
    );
}

/// The names among `names` that are the rules of the index `index` of `table`: an index's rule
/// names the index after the table it belongs to.
fn rules_of_index(names: &[String], table: &str, index: &str) -> Vec<String> {
    names
        .iter()
        .filter(|name| name.contains(&format!("-{table}-{index}-")))
        .cloned()
        .collect()
}

/// One row of the index table: the index enabled, whether its own key schema declares a sort
/// key, and the kinds its enable must mint.
struct IndexRow {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The index the enable names.
    index: &'static str,
    /// The kinds that enable must mint, ascending.
    kinds: &'static [&'static str],
}

#[tokio::test]
async fn enabling_contributor_insights_on_an_index_mints_the_index_s_own_key_schema_s_rules() {
    let router = emulator_with_cloudwatch();
    const TABLE: &str = "insights-indexed";
    const HASH_ONLY_INDEX: &str = "HashOnly.Index";
    const SORTED_INDEX: &str = "Sorted.Index";
    let case = "a table with a sort key carrying two indexes";
    // The TABLE declares a sort key and one of its indexes does not, which is what tells the
    // index's own key schema apart from its table's.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "Part", "KeyType": "HASH"},
                {"AttributeName": "Sort", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "Part", "AttributeType": "S"},
                {"AttributeName": "Sort", "AttributeType": "S"},
                {"AttributeName": "Other", "AttributeType": "S"},
                {"AttributeName": "Second", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": HASH_ONLY_INDEX,
                    "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": SORTED_INDEX,
                    "KeySchema": [
                        {"AttributeName": "Other", "KeyType": "HASH"},
                        {"AttributeName": "Second", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    describe_when_active(&router, case, TABLE).await;

    let rows: [IndexRow; 2] = [
        IndexRow {
            case: "a hash-only index of a table that declares a sort key",
            index: HASH_ONLY_INDEX,
            kinds: &[
                insights::RULE_KIND_PARTITION_ACCESSED,
                insights::RULE_KIND_PARTITION_THROTTLED,
            ],
        },
        IndexRow {
            case: "an index that declares a sort key of its own",
            index: SORTED_INDEX,
            kinds: &[
                insights::RULE_KIND_PARTITION_ACCESSED,
                insights::RULE_KIND_PARTITION_THROTTLED,
                insights::RULE_KIND_SORT_ACCESSED,
                insights::RULE_KIND_SORT_THROTTLED,
            ],
        },
    ];

    for row in &rows {
        let case = row.case;
        ok(
            &router,
            "UpdateContributorInsights",
            json!({
                "TableName": TABLE,
                "IndexName": row.index,
                "ContributorInsightsAction": "ENABLE",
                "ContributorInsightsMode": "ACCESSED_AND_THROTTLED_KEYS",
            }),
        )
        .await;
        let names = described_rule_names(&router, case).await;
        let minted = rules_of_index(&names, TABLE, row.index);
        let mut expected: Vec<String> = row.kinds.iter().map(|kind| (*kind).to_string()).collect();
        expected.sort();
        assert_eq!(
            kinds_in(&minted),
            expected,
            "case {case:?}: expected the rule kinds {expected:?} for {:?}, got {minted:?}",
            row.index
        );
        // The resource that reports them is the INDEX: the table's own setting is untouched, so
        // it names no rule of its own.
        let described = ok(
            &router,
            "DescribeContributorInsights",
            json!({"TableName": TABLE, "IndexName": row.index}),
        )
        .await;
        let mut reported: Vec<String> = described["ContributorInsightsRuleList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the read answers its rule list, got {described}")
            })
            .iter()
            .filter_map(|name| name.as_str().map(ToString::to_string))
            .collect();
        reported.sort();
        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(
            reported, sorted,
            "case {case:?}: expected the index's rule list {sorted:?}, got {reported:?}"
        );
    }
    let table_itself = ok(
        &router,
        "DescribeContributorInsights",
        json!({"TableName": TABLE}),
    )
    .await;
    assert_eq!(
        table_itself["ContributorInsightsRuleList"],
        Value::Null,
        "case {case:?}: enabling an index's insights leaves the table's own off, got \
         {table_itself}"
    );

    // Disabling one index deletes its rules only: the other index keeps collecting.
    ok(
        &router,
        "UpdateContributorInsights",
        json!({
            "TableName": TABLE,
            "IndexName": HASH_ONLY_INDEX,
            "ContributorInsightsAction": "DISABLE",
        }),
    )
    .await;
    let names = described_rule_names(&router, case).await;
    assert_eq!(
        (
            rules_of_index(&names, TABLE, HASH_ONLY_INDEX),
            kinds_in(&rules_of_index(&names, TABLE, SORTED_INDEX))
        ),
        (
            Vec::new(),
            vec![
                insights::RULE_KIND_PARTITION_ACCESSED.to_string(),
                insights::RULE_KIND_PARTITION_THROTTLED.to_string(),
                insights::RULE_KIND_SORT_ACCESSED.to_string(),
                insights::RULE_KIND_SORT_THROTTLED.to_string(),
            ]
        ),
        "case {case:?}: one index's disable leaves the other index's rules alone, got {names:?}"
    );
}

/// One row of the access table: the accesses made of one key, and what they must contribute.
struct AccessRow {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The partition key the accesses address.
    key: &'static str,
    /// How the key is accessed.
    access: Access,
    /// How many times.
    times: usize,
    /// What the accesses must contribute in total.
    contributed: f64,
}

/// The kinds of access a row makes.
#[derive(Debug, Clone, Copy)]
enum Access {
    /// A strongly consistent `GetItem`, which consumes one unit for an item under 4 KB.
    ConsistentRead,
    /// An eventually consistent `GetItem`, which consumes half of one.
    EventualRead,
    /// A `PutItem` of a small item, which consumes one write unit.
    Write,
    /// A `Query` pinned to the key, strongly consistent.
    ConsistentQuery,
}

#[tokio::test]
async fn every_access_that_names_a_key_contributes_to_the_table_s_accessed_keys_rule() {
    let router = emulator_with_cloudwatch();
    const TABLE: &str = "insights-report";
    let case = "the table under test";
    create_table(&router, case, TABLE, false).await;
    // The items are stored BEFORE insights are on, so what the rule holds is the accesses the
    // rows below make and nothing else.
    for key in ["hot", "warm", "cool", "unread"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": {"Part": {"S": key}}}),
        )
        .await;
    }
    set_insights(&router, TABLE, "ENABLE", Some("ACCESSED_AND_THROTTLED_KEYS")).await;

    // The accessed-keys rule of the table, read off the resource's own rule list: the rules
    // exist from the enable, so every row below is asserted against this one.
    let described = ok(
        &router,
        "DescribeContributorInsights",
        json!({"TableName": TABLE}),
    )
    .await;
    let rule_list: Vec<String> = described["ContributorInsightsRuleList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the read answers its rule list, got {described}"))
        .iter()
        .filter_map(|name| name.as_str().map(ToString::to_string))
        .collect();
    let accessed = rule_list
        .iter()
        .find(|name| name.contains(&format!("-{}-", insights::RULE_KIND_PARTITION_ACCESSED)))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the table has an accessed-keys rule, got {rule_list:?}")
        })
        .clone();

    let rows: Vec<AccessRow> = vec![
        AccessRow {
            case: "strongly consistent reads of one key",
            key: "hot",
            access: Access::ConsistentRead,
            times: 5,
            contributed: 5.0,
        },
        AccessRow {
            case: "eventually consistent reads, which consume half a unit each",
            key: "warm",
            access: Access::EventualRead,
            times: 4,
            contributed: 2.0,
        },
        AccessRow {
            case: "a write of a key",
            key: "cool",
            access: Access::Write,
            times: 1,
            contributed: 1.0,
        },
        AccessRow {
            case: "a query pinned to one partition",
            key: "warm",
            access: Access::ConsistentQuery,
            times: 1,
            contributed: 1.0,
        },
    ];

    let mut expected: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();
    for row in &rows {
        for _ in 0..row.times {
            match row.access {
                Access::ConsistentRead | Access::EventualRead => {
                    ok(
                        &router,
                        "GetItem",
                        json!({
                            "TableName": TABLE,
                            "Key": {"Part": {"S": row.key}},
                            "ConsistentRead": matches!(row.access, Access::ConsistentRead),
                        }),
                    )
                    .await;
                }
                Access::Write => {
                    ok(
                        &router,
                        "PutItem",
                        json!({"TableName": TABLE, "Item": {"Part": {"S": row.key}}}),
                    )
                    .await;
                }
                Access::ConsistentQuery => {
                    ok(
                        &router,
                        "Query",
                        json!({
                            "TableName": TABLE,
                            "KeyConditionExpression": "Part = :part",
                            "ExpressionAttributeValues": {":part": {"S": row.key}},
                            "ConsistentRead": true,
                        }),
                    )
                    .await;
                }
            }
        }
        *expected.entry(row.key).or_default() += row.contributed;
        // Each row is asserted as it is made: the key it accessed carries everything the rows so
        // far contributed to it, so a row that contributes nothing is caught where it happened
        // rather than in the totals below.
        let so_far = expected.get(row.key).copied().unwrap_or_default();
        let got = reported_contributors(&report(&router, &accessed).await);
        let row_case = row.case;
        assert!(
            got.iter()
                .any(|(keys, value)| keys == row.key && (value - so_far).abs() < f64::EPSILON),
            "case {row_case:?}: the key {:?} must have contributed {so_far} after its accesses, \
             got {got:?}",
            row.key
        );
    }

    let answered = report(&router, &accessed).await;
    assert_eq!(
        answered["KeyLabels"],
        json!([insights::KEY_LABEL_PARTITION]),
        "case {case:?}: the contributors are classified by the partition key, got {answered}"
    );
    assert_eq!(
        answered["AggregationStatistic"],
        json!("Sum"),
        "case {case:?}: the rule aggregates the measure it collects, got {answered}"
    );
    // Descending by what each key was accessed for, which is what ranks the hot key first.
    let mut ranked: Vec<(String, f64)> = expected
        .iter()
        .map(|(key, value)| ((*key).to_string(), *value))
        .collect();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let got = reported_contributors(&answered);
    assert_eq!(
        got, ranked,
        "case {case:?}: expected the contributors {ranked:?}, got {got:?} in {answered}"
    );
    assert_eq!(
        answered["AggregateValue"].as_f64(),
        Some(ranked.iter().map(|(_, value)| value).sum::<f64>()),
        "case {case:?}: the aggregate is the sum of every contributor, got {answered}"
    );
    assert!(
        got.iter().all(|(keys, _)| keys != "unread"),
        "case {case:?}: a key nothing accessed is no contributor, got {got:?}"
    );

    // The THROTTLED-keys rule of the same table collected nothing: no access was refused, so
    // the other half of the feature holds no contributor.
    let throttled = rule_list
        .iter()
        .find(|name| name.contains(&format!("-{}-", insights::RULE_KIND_PARTITION_THROTTLED)))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the table has a throttled-keys rule, got {rule_list:?}")
        })
        .clone();
    let answered = report(&router, &throttled).await;
    assert_eq!(
        reported_contributors(&answered),
        Vec::new(),
        "case {case:?}: nothing was throttled, so that rule has no contributor, got {answered}"
    );
}

/// The read capacity the throttled row's table is provisioned for: the smallest a table can
/// carry, so its documented burst is spent in a handful of reads.
const PROVISIONED_READ_UNITS: i64 = 1;

/// The bytes of the item that row reads: "one read request unit represents one strongly
/// consistent read request ... for an item up to 4 KB in size" (the Developer Guide), so a read
/// of this item costs about a hundred units and a 1-RCU table's whole burst is a few of them.
const ITEM_BYTES: usize = 399 * 1024;

/// How many reads that row drives before it gives up on ever being throttled.
const MOST_READS: usize = 24;

#[tokio::test]
async fn a_throttled_access_contributes_to_the_table_s_throttled_keys_rule() {
    let router = emulator_with_cloudwatch();
    const TABLE: &str = "insights-throttled";
    let case = "reads past a 1-RCU table's burst, tracking throttled events only";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {
                "ReadCapacityUnits": PROVISIONED_READ_UNITS,
                // Write capacity enough that seeding the item is never the refusal under test.
                "WriteCapacityUnits": 4000,
            },
        }),
    )
    .await;
    describe_when_active(&router, case, TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"Part": {"S": "hot"}, "blob": {"S": "x".repeat(ITEM_BYTES)}},
        }),
    )
    .await;
    set_insights(&router, TABLE, "ENABLE", Some("THROTTLED_KEYS")).await;

    // Only the narrower mode's rule is minted, so what the served reads consumed reaches no
    // rule at all and the one rule there is holds the throttled accesses.
    let rules = rules_of(&described_rule_names(&router, case).await, TABLE);
    assert_eq!(
        rules.len(),
        1,
        "case {case:?}: the narrower mode mints one rule for a hash-only table, got {rules:?}"
    );
    let throttled_rule = rules.first().cloned().unwrap_or_default();
    assert!(
        throttled_rule
            .contains(&format!("-{}-", insights::RULE_KIND_PARTITION_THROTTLED)),
        "case {case:?}: that rule is the throttled-keys one, got {throttled_rule:?}"
    );

    let mut throttled = 0.0_f64;
    let mut served = 0usize;
    for _ in 0..MOST_READS {
        let (status, _) = crate::emulator::call(
            &router,
            "GetItem",
            json!({
                "TableName": TABLE,
                "Key": {"Part": {"S": "hot"}},
                "ConsistentRead": true,
            }),
        )
        .await;
        if status == 200 {
            served += 1;
        } else {
            throttled += 1.0;
        }
    }
    assert!(
        throttled > 0.0 && served > 0,
        "case {case:?}: the burst must serve some reads and refuse the rest, got {served} served \
         and {throttled} refused"
    );

    let answered = report(&router, &throttled_rule).await;
    assert_eq!(
        (
            answered["KeyLabels"].clone(),
            reported_contributors(&answered)
        ),
        (
            json!([insights::KEY_LABEL_PARTITION]),
            vec![("hot".to_string(), throttled)]
        ),
        "case {case:?}: the throttled key is the only contributor, with one event per refused \
         read, got {answered}"
    );
}

#[tokio::test]
async fn a_table_the_feature_is_off_for_has_no_rule_to_report() {
    let router = emulator_with_cloudwatch();
    const TABLE: &str = "insights-off";
    let case = "a table no enable has named";
    create_table(&router, case, TABLE, false).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"Part": {"S": "key"}}}),
    )
    .await;
    ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"Part": {"S": "key"}}, "ConsistentRead": true}),
    )
    .await;
    let names = described_rule_names(&router, case).await;
    assert!(
        names.is_empty(),
        "case {case:?}: no enable means no rule, got {names:?}"
    );
    let described = ok(
        &router,
        "DescribeContributorInsights",
        json!({"TableName": TABLE}),
    )
    .await;
    assert_eq!(
        (
            described["ContributorInsightsStatus"].clone(),
            described["ContributorInsightsRuleList"].clone()
        ),
        (json!("DISABLED"), Value::Null),
        "case {case:?}: the resource reports the feature off and no rule, got {described}"
    );
}
