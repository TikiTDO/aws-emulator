// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the value set a workload's `IndustryType` carries.
//!
//! The rule under test, over a TABLE of inputs rather than one request: a member whose
//! documentation publishes a closed value set carries one of those values, whatever the
//! model types it as — "If specified, must be one of the following: Agriculture … Other"
//! (`smithy_query(query="doc", shape="WorkloadIndustryType")`). The model bounds the member
//! only by `@length(max: 100)`, so the boundary walk cannot hold it to the set; every
//! operation that WRITES a workload holds it there instead, and a value outside the set is
//! refused with the modelled `ValidationException` (HTTP 400) naming `IndustryType`, leaving
//! the store exactly as it was.
//!
//! Both operations that write the member are covered — `CreateWorkload` and
//! `UpdateWorkload` — because the set belongs to the member, not to one request.
//!
//! Every request goes through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, the boundary constraint walk and the handler all run exactly as they
//! do in production.

use serde_json::{json, Value};

use super::workload_harness::{
    created_id, minimal_create, stored_workload, stored_workload_count, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// Every value the member's documentation publishes, in the order it lists them.
///
/// SOURCE: `smithy_query(query="doc", shape="WorkloadIndustryType")` — "If specified, must
/// be one of the following: Agriculture | Automobile | … | Other". Spelled out here rather
/// than read off the code under test, so the set the emulator holds to is pinned against
/// the documentation and not against itself.
const PUBLISHED_INDUSTRY_TYPES: &[&str] = &[
    "Agriculture",
    "Automobile",
    "Defense",
    "Design and Engineering",
    "Digital Advertising",
    "Education",
    "Environmental Protection",
    "Financial Services",
    "Gaming",
    "General Public Services",
    "Healthcare",
    "Hospitality",
    "InfoTech",
    "Justice and Public Safety",
    "Life Sciences",
    "Manufacturing",
    "Media & Entertainment",
    "Mining & Resources",
    "Oil & Gas",
    "Power & Utilities",
    "Professional Services",
    "Real Estate & Construction",
    "Retail & Wholesale",
    "Social Protection",
    "Telecommunications",
    "Travel, Transportation & Logistics",
    "Other",
];

/// The published `@length` maximum of `WorkloadIndustryType`
/// (`smithy_query(query="shape", shape="WorkloadIndustryType")`): the only bound the model
/// itself puts on the member.
const INDUSTRY_TYPE_MAX: usize = 100;

/// The values no workload may carry, each a value a caller plausibly sends and the service
/// does not publish.
///
/// The rows differ along WHAT is wrong with the value: a word no published type carries (the
/// instance the issue reported), a published type in another case (membership is exact), a
/// near miss of one, a prefix of one, a published type with surrounding whitespace, a
/// published type's list separator sent as the value, the empty string, and a value AT the
/// published length maximum that is still outside the set.
fn out_of_set_values() -> Vec<(String, String)> {
    vec![
        ("a-word-no-type-carries", "Not An Industry".to_string()),
        ("a-published-type-lowercased", "infotech".to_string()),
        ("a-published-type-uppercased", "HEALTHCARE".to_string()),
        (
            "a-near-miss-of-a-published-type",
            "Media and Entertainment".to_string(),
        ),
        ("a-prefix-of-a-published-type", "Agricultur".to_string()),
        ("a-published-type-with-padding", " Other ".to_string()),
        (
            "two-published-types-at-once",
            "Gaming, Education".to_string(),
        ),
        ("the-empty-string", String::new()),
        (
            "at-the-length-maximum-and-out-of-set",
            "i".repeat(INDUSTRY_TYPE_MAX),
        ),
    ]
    .into_iter()
    .map(|(case, value)| (case.to_string(), value))
    .collect()
}

/// The sentence the refusal states for an out-of-set `IndustryType`: the member, and the
/// set it has to satisfy, as the service words a member-constraint failure.
fn expected_sentence() -> String {
    format!(
        "Value at IndustryType failed to satisfy constraint: Member must satisfy enum value \
         set: [{}]",
        PUBLISHED_INDUSTRY_TYPES.join(", ")
    )
}

/// A `CreateWorkload` request carrying `industry_type`.
fn create_request(name: &str, industry_type: &Value) -> Value {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["IndustryType"] = industry_type.clone();
    request
}

/// The `IndustryType` a stored workload reports through `GetWorkload`, absent when the
/// record carries none — read through the shared `stored_workload` reader.
async fn reported_industry_type(client: &Client, workload_id: &str, case: &str) -> Option<String> {
    stored_workload(client, workload_id, case)
        .await
        .get("IndustryType")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The set the emulator holds the member to is the set the documentation publishes — the
/// values the refusal below lists, and the values the create and update accept.
#[tokio::test]
async fn the_industry_type_catalog_is_the_published_set() {
    assert_eq!(
        state::WORKLOAD_INDUSTRY_TYPES,
        PUBLISHED_INDUSTRY_TYPES,
        "case \"catalog\": the emulator's industry types must be the documented set, got {:?}",
        state::WORKLOAD_INDUSTRY_TYPES
    );
    for value in PUBLISHED_INDUSTRY_TYPES {
        assert!(
            value.len() <= INDUSTRY_TYPE_MAX,
            "case {value:?}: a published industry type must fit the member's length maximum, \
             got {} characters",
            value.len()
        );
    }
}

/// EVERY published industry type can be created, is read back as sent, and can be replaced
/// by another published value through `UpdateWorkload`.
///
/// The rows are the WHOLE published set — including the values the issue never cited — and
/// each row's update moves the workload to the NEXT value of the set, so every value is
/// exercised on both write paths.
#[tokio::test]
async fn every_published_industry_type_is_accepted_and_read_back() {
    let client = Client::new();
    for (index, industry_type) in PUBLISHED_INDUSTRY_TYPES.iter().enumerate() {
        let case = format!("published-{index}");
        let created = client
            .create(create_request(&case, &json!(industry_type)))
            .await;
        let workload_id = created_id(&created, &case);
        assert_eq!(
            reported_industry_type(&client, &workload_id, &case).await,
            Some((*industry_type).to_string()),
            "case {case:?}: the stored workload must report the industry type {industry_type:?} \
             as sent"
        );

        // The next value of the set, so the update is a DIFFERENT published value for every
        // row rather than a rewrite of the same one.
        let replacement = PUBLISHED_INDUSTRY_TYPES[(index + 1) % PUBLISHED_INDUSTRY_TYPES.len()];
        let updated = client
            .update(&workload_id, json!({ "IndustryType": replacement }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateWorkload must accept the published industry type \
             {replacement:?}, got {updated:?}"
        );
        assert_eq!(
            reported_industry_type(&client, &workload_id, &case).await,
            Some(replacement.to_string()),
            "case {case:?}: the stored workload must report the updated industry type \
             {replacement:?}"
        );
    }
}

/// A `CreateWorkload` whose `IndustryType` is outside the published set is refused and
/// records no workload — and the name it asked for is still free afterwards.
#[tokio::test]
async fn a_create_with_an_unpublished_industry_type_is_refused_and_stores_nothing() {
    let client = Client::new();
    let sentence = expected_sentence();

    for (case, value) in out_of_set_values() {
        let before = stored_workload_count(&client);
        let answer = client.create(create_request(&case, &json!(value))).await;

        assert_eq!(
            answer.status, 400,
            "case {case:?}: the industry type {value:?} is not published, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {case:?}: the rule is about the member's value, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields"),
            Some(&json!([{ "Name": "IndustryType", "Message": sentence }])),
            "case {case:?}: the refusal must report the offending member, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(format!("[Validation] {sentence}").as_str()),
            "case {case:?}: Message must state the rule once, got {answer:?}"
        );
        assert_eq!(
            stored_workload_count(&client),
            before,
            "case {case:?}: a refused CreateWorkload must store no workload"
        );

        // The refused create took nothing: the name it asked for is still available, and a
        // workload with a published industry type takes it.
        let accepted = client.create(create_request(&case, &json!("Other"))).await;
        created_id(&accepted, &format!("{case}-retried-with-a-published-type"));
    }
}

/// The member's published `@length` maximum and the value set are DIFFERENT rules, and each
/// row names which one refused it: at the maximum the length is satisfied, so the refusal is
/// the value-set rule; one character past it the boundary's own constraint walk refuses the
/// length before the handler's rule is reached.
#[tokio::test]
async fn the_length_maximum_and_the_value_set_are_refused_as_different_rules() {
    let client = Client::new();
    let sentence = expected_sentence();
    // The layer that reads the body states a length failure as the finding it found — the
    // string it measured, its length, and the published maximum — rather than as a sentence
    // naming the member.
    let too_long = "i".repeat(INDUSTRY_TYPE_MAX + 1);
    let length_clause = format!(
        "string \"{too_long}\" is too long (length: {}, maximum allowed: {INDUSTRY_TYPE_MAX})",
        INDUSTRY_TYPE_MAX + 1
    );

    for (case, value, expected_clause) in [
        (
            "at-the-maximum",
            "i".repeat(INDUSTRY_TYPE_MAX),
            sentence.as_str(),
        ),
        (
            "one-past-the-maximum",
            too_long.clone(),
            length_clause.as_str(),
        ),
    ] {
        let answer = client.create(create_request(case, &json!(value))).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a {}-character industry type must be refused, got {answer:?}",
            value.len()
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let message = answer
            .body
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(expected_clause),
            "case {case:?}: the refusal must state {expected_clause:?}, got {message:?}"
        );
        // The two rules are told apart: a length failure is NOT answered with the value-set
        // sentence, so this row would fail if the published bound were not enforced.
        if case == "one-past-the-maximum" {
            assert!(
                !message.contains(sentence.as_str()),
                "case {case:?}: a length failure must not be reported as the value-set rule, \
                 got {message:?}"
            );
        }
        assert_eq!(
            stored_workload_count(&client),
            0,
            "case {case:?}: a refused CreateWorkload must store no workload"
        );
    }
}

/// An `UpdateWorkload` whose `IndustryType` is outside the published set is refused and
/// applies NOTHING — not the industry type, and not the other members the same request
/// carried.
#[tokio::test]
async fn an_update_to_an_unpublished_industry_type_changes_nothing() {
    let client = Client::new();
    let sentence = expected_sentence();
    // The workload every row updates starts from a published value, so a refused update is
    // observable as the value it did NOT replace.
    const STORED: &str = "Healthcare";

    for (case, value) in out_of_set_values() {
        let created = client
            .create(create_request(&format!("update-{case}"), &json!(STORED)))
            .await;
        let workload_id = created_id(&created, &case);

        let answer = client
            .update(
                &workload_id,
                json!({
                    "IndustryType": value,
                    "Description": "an update that names an unpublished industry type",
                }),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: UpdateWorkload must refuse the industry type {value:?}, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields"),
            Some(&json!([{ "Name": "IndustryType", "Message": sentence }])),
            "case {case:?}: the refusal must report the offending member, got {answer:?}"
        );

        assert_eq!(
            reported_industry_type(&client, &workload_id, &case).await,
            Some(STORED.to_string()),
            "case {case:?}: a refused update must leave the stored industry type alone"
        );
        let stored = client.get(&workload_id).await;
        assert_eq!(
            stored
                .body
                .get("Workload")
                .and_then(|workload| workload.get("Description"))
                .and_then(Value::as_str),
            Some("a workload under test"),
            "case {case:?}: a refused update must apply none of its other members, got {stored:?}"
        );
    }
}
