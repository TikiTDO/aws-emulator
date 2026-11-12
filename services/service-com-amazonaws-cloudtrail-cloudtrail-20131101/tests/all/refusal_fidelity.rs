// SPDX-License-Identifier: CC0-1.0

//! What a refusal SAYS, across the trail, event-selector, Insights and
//! event-configuration surfaces.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees: the HTTP status, the
//! error code it branches on, and the `Message` it reads.
//!
//! The rules under test, each held over a SET of inputs rather than for one request:
//!
//! * a trail refusal's subject is the trail the operation RESOLVED the request to:
//!   the fully-qualified trail ARN for every operation that resolves one Region's
//!   trail to drive or configure it, and the identifier as the caller wrote it for the
//!   operations that take it as a name to look up (`GetTrail`, `GetInsightSelectors`,
//!   `GetEventConfiguration`, `PutEventConfiguration`);
//! * `CreateTrail`'s duplicate-name refusal names the trail the request asked for and
//!   the ACCOUNT that already holds one of that name;
//! * an invalid trail name is refused for the ONE rule it broke, with the rules
//!   applied in the order the service applies them — length, then the edges, then
//!   adjacency, then the character set;
//! * an event-selector refusal states the documented remedy for the bound it broke,
//!   and a value outside a modelled constraint inside a selector is the modelled
//!   `InvalidEventSelectorsException`, not a generic `ValidationException`;
//! * an Insights-not-enabled refusal names the resource the call addressed plus the
//!   documented tail;
//! * `DescribeTrails` SELECTS: an entry that names no trail of the account — a name
//!   nothing carries, or a value that can name no trail at all — narrows the answer
//!   instead of failing the request;
//! * `PutEventConfiguration` judges the request's OWN parameters before it resolves
//!   the resource they are for, so a call that states no configuration is refused on
//!   the parameters whatever the registry holds, and a resource's absence is reported
//!   once the parameters hold.
//!
//! The event data store registry is seeded through the service's public `state` module
//! because `CreateEventDataStore` is still a scaffold stub; every call under test goes
//! through the router.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    EVENT_CATEGORY_AGGREGATION_VALUES, INSIGHT_TYPE_VALUES, READ_WRITE_TYPE_VALUES, TEMPLATE_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, trail_arn, EventDataStoreRecord, EventDataStoreState,
    ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT, DATA_RESOURCES_PER_TRAIL_LIMIT,
    EVENT_SELECTORS_PER_TRAIL_LIMIT, MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE,
    TRAIL_NAME_MAX_LEN, TRAIL_NAME_MIN_LEN,
};

use super::emulator::{Emulator, HOME_REGION};

/// The status CloudTrail answers a not-found resource with on this awsJson1_1 API,
/// where the model annotates `@httpError(404)`
/// (`state::TRAIL_NOT_FOUND_STATUS`).
const NOT_FOUND_STATUS: u16 = 400;

/// An Insights statement every trail accepts, for the cases that need a well-formed
/// `PutInsightSelectors` request body around the identifier under test.
fn insights_statement() -> Value {
    json!([{ "InsightType": INSIGHT_TYPE_VALUES[0] }])
}

/// A well-formed `AggregationConfigurations` member, for the cases that need a
/// well-formed `PutEventConfiguration` trail request around the identifier under test.
/// Both values come from the model's own enums (`Template`, `EventCategoryAggregation`).
fn aggregations() -> Value {
    json!([{
        "Templates": [TEMPLATE_VALUES[0]],
        "EventCategory": EVENT_CATEGORY_AGGREGATION_VALUES[0],
    }])
}

/// Create `name` in `region` over the wire, as a client does.
async fn create_trail(emulator: &Emulator, region: &str, name: &str) {
    emulator
        .call_in(
            "CreateTrail",
            region,
            &json!({ "Name": name, "S3BucketName": "refusal-bucket" }),
        )
        .await
        .expect_ok(&format!("CreateTrail {name} in {region}"));
}

/// `count` basic event selectors, each logging management events only.
fn selectors(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                json!({
                    "ReadWriteType": READ_WRITE_TYPE_VALUES[index % READ_WRITE_TYPE_VALUES.len()],
                    "IncludeManagementEvents": true,
                })
            })
            .collect(),
    )
}

/// One basic event selector carrying `count` data resources.
fn selectors_with_data_resources(count: usize) -> Value {
    let resources: Vec<Value> = (0..count)
        .map(|index| json!({ "Type": "AWS::S3::Object", "Values": [format!("arn:aws:s3:::b{index}/")] }))
        .collect();
    json!([{ "IncludeManagementEvents": false, "DataResources": resources }])
}

/// One advanced event selector whose field selectors carry `count` condition values in
/// total.
///
/// The mandatory `eventCategory` field selector is one of them — every advanced event
/// selector states which category of events it selects — so the rest of the count is
/// spread over a `resources.ARN` condition.
fn advanced_with_values(count: usize) -> Value {
    let values: Vec<String> = (0..count.saturating_sub(1))
        .map(|index| format!("arn:aws:s3:::b{index}/"))
        .collect();
    let mut field_selectors = vec![json!({"Field": "eventCategory", "Equals": ["Data"]})];
    if !values.is_empty() {
        field_selectors.push(json!({"Field": "resources.ARN", "StartsWith": values}));
    }
    json!([{ "Name": "generated", "FieldSelectors": field_selectors }])
}

/// Every operation that resolves a trail identifier to ONE Region's trail in order to
/// drive or configure it, as a request naming `identifier`: these name the resolved
/// trail ARN in a not-found refusal.
fn arn_naming_requests(identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("UpdateTrail", json!({ "Name": identifier })),
        ("GetTrailStatus", json!({ "Name": identifier })),
        ("StartLogging", json!({ "Name": identifier })),
        ("StopLogging", json!({ "Name": identifier })),
        ("DeleteTrail", json!({ "Name": identifier })),
        ("GetEventSelectors", json!({ "TrailName": identifier })),
        (
            "PutEventSelectors",
            json!({ "TrailName": identifier, "EventSelectors": selectors(1) }),
        ),
        (
            "PutInsightSelectors",
            json!({ "TrailName": identifier, "InsightSelectors": insights_statement() }),
        ),
    ]
}

/// Every operation that takes the trail identifier as a name to LOOK UP, as a request
/// naming `identifier`: these name that identifier as the request wrote it. Which
/// subject each operation states is pinned per operation by its own recorded refusal;
/// `tests/all/trail_not_found_identifier_echo.rs` holds the rule over both groups.
fn name_echoing_requests(identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetTrail", json!({ "Name": identifier })),
        ("GetInsightSelectors", json!({ "TrailName": identifier })),
        ("GetEventConfiguration", json!({ "TrailName": identifier })),
        (
            "PutEventConfiguration",
            json!({ "TrailName": identifier, "AggregationConfigurations": aggregations() }),
        ),
    ]
}

/// A well-formed identifier that names no trail is `TrailNotFoundException` (HTTP 400)
/// whose subject is the trail the operation resolved: its fully-qualified ARN for
/// every operation that resolves one Region's trail to drive or configure it, whichever
/// form the caller wrote, and the identifier as given for the operations that take it
/// as a name to look up. The rows differ in the identifier's FORM (bare name, trail
/// ARN), its length (each published bound) and the Region it pins.
#[tokio::test]
async fn a_trail_not_found_refusal_names_the_trail_the_operation_resolved() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    // One stored trail, so a not-found is "not this trail" and not "no trails".
    create_trail(&emulator, HOME_REGION, "refusal-present").await;

    // (case, the identifier, the trail ARN it resolves to)
    let rows: Vec<(String, String, String)> = vec![
        (
            "a bare name nothing carries".to_string(),
            "refusal-absent".to_string(),
            trail_arn(HOME_REGION, "refusal-absent"),
        ),
        (
            format!("a bare name at the documented minimum of {TRAIL_NAME_MIN_LEN}"),
            "a".repeat(TRAIL_NAME_MIN_LEN),
            trail_arn(HOME_REGION, &"a".repeat(TRAIL_NAME_MIN_LEN)),
        ),
        (
            format!("a bare name at the documented maximum of {TRAIL_NAME_MAX_LEN}"),
            "z".repeat(TRAIL_NAME_MAX_LEN),
            trail_arn(HOME_REGION, &"z".repeat(TRAIL_NAME_MAX_LEN)),
        ),
        (
            "a bare name carrying every allowed punctuation".to_string(),
            "a.b_c-d9".to_string(),
            trail_arn(HOME_REGION, "a.b_c-d9"),
        ),
        (
            "a trail ARN in the caller's own Region".to_string(),
            trail_arn(HOME_REGION, "refusal-absent"),
            trail_arn(HOME_REGION, "refusal-absent"),
        ),
    ];

    for (case, identifier, arn) in rows {
        let expected = format!("Unknown trail: {arn} for the user: {account}");
        for (operation, request) in arn_naming_requests(&identifier) {
            let case = format!("{operation} with {case}");
            let reply = emulator.call(operation, &request).await;
            reply.expect_error(&case, NOT_FOUND_STATUS, "TrailNotFoundException");
            assert_eq!(
                reply.message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }

        // The operations whose recorded refusal names the caller's identifier as
        // written rather than resolving an ARN for it.
        let echoed = format!("Unknown trail: {identifier} for the user: {account}");
        for (operation, request) in name_echoing_requests(&identifier) {
            let case = format!("{operation} with {case}");
            let reply = emulator.call(operation, &request).await;
            reply.expect_error(&case, NOT_FOUND_STATUS, "TrailNotFoundException");
            assert_eq!(
                reply.message(),
                echoed,
                "case {case}: expected {echoed:?}, got {:?}",
                reply.message()
            );
        }
    }

    // The stored trail survived every refusal.
    let present = emulator
        .call("GetTrail", &json!({ "Name": "refusal-present" }))
        .await;
    assert_eq!(
        present.expect_ok("GetTrail refusal-present")["Trail"]["Name"],
        json!("refusal-present"),
        "case GetTrail refusal-present: the stored trail did not survive the refusals, \
         got {present:?}"
    );
}

/// A trail ARN from another Region resolves to THAT Region's trail, so the refusal
/// names it there — the subject is what the identifier resolved to, not where the
/// call was made.
#[tokio::test]
async fn a_trail_not_found_refusal_names_the_region_the_identifier_pinned() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    for region in ["eu-west-1", "ap-south-1", HOME_REGION] {
        let arn = trail_arn(region, "refusal-absent");
        let expected = format!("Unknown trail: {arn} for the user: {account}");
        let case = format!("GetTrailStatus with a trail ARN in {region}");
        let reply = emulator
            .call("GetTrailStatus", &json!({ "Name": arn }))
            .await;
        reply.expect_error(&case, NOT_FOUND_STATUS, "TrailNotFoundException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }
}

/// `CreateTrail` refuses a name this Region already holds with
/// `TrailAlreadyExistsException` (HTTP 400) naming the trail and the ACCOUNT, and the
/// stored trail is untouched. The rows differ in the name's form and length.
#[tokio::test]
async fn a_duplicate_trail_name_is_refused_naming_the_account() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    let names = [
        "refusal-duplicate".to_string(),
        "a".repeat(TRAIL_NAME_MIN_LEN),
        "z".repeat(TRAIL_NAME_MAX_LEN),
        "a.b_c-d9".to_string(),
        "0123456789".to_string(),
    ];

    for name in names {
        create_trail(&emulator, HOME_REGION, &name).await;
        let expected = format!("Trail {name} already exists for customer: {account}");
        let reply = emulator
            .call(
                "CreateTrail",
                &json!({ "Name": name, "S3BucketName": "other-bucket" }),
            )
            .await;
        let case = format!("CreateTrail {name} twice");
        reply.expect_error(&case, 400, "TrailAlreadyExistsException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message()
        );

        // The refusal is a conflict, never an overwrite: the first trail's bucket
        // is still the stored one.
        let stored = emulator.call("GetTrail", &json!({ "Name": name })).await;
        assert_eq!(
            stored.expect_ok(&case)["Trail"]["S3BucketName"],
            json!("refusal-bucket"),
            "case {case}: the refused create overwrote the stored trail, got {stored:?}"
        );

        // Another Region's registry is a different one: the same name is free there.
        create_trail(&emulator, "eu-west-1", &name).await;
    }
}

/// An invalid trail name is refused for the ONE rule it broke, with the rules applied
/// in the order the service applies them: a name that breaks several is refused for
/// the first. The rows differ in which rule is broken and in how many of them are
/// broken at once.
#[tokio::test]
async fn an_invalid_trail_name_reports_the_one_rule_it_broke() {
    let emulator = Emulator::new();
    let too_short = |length: usize| {
        format!(
            "Trail name too short. Minimum allowed length: {TRAIL_NAME_MIN_LEN} characters. \
             Specified name length: {length} characters."
        )
    };
    let charset = "Trail name or ARN can only contain ASCII letters (a-z, A-Z), numbers (0-9), \
                   periods (.), underscores (_), or dashes (-)."
        .to_string();
    let adjacent = "Trail name or ARN cannot have adjacent periods (.), hyphens (-), or \
                    underscores (_)."
        .to_string();

    // (case, the name, the refusal it must carry)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "one character under the documented minimum",
            "ab".to_string(),
            too_short(2),
        ),
        ("a single character", "a".to_string(), too_short(1)),
        (
            "one character over the documented maximum",
            "a".repeat(TRAIL_NAME_MAX_LEN + 1),
            format!(
                "Trail name too long. Maximum allowed length: {TRAIL_NAME_MAX_LEN} characters. \
                 Specified name length: {} characters.",
                TRAIL_NAME_MAX_LEN + 1
            ),
        ),
        (
            // Under the minimum AND outside the character set: the length rule is
            // applied first, so that is the rule the refusal names.
            "too short and outside the character set",
            "a!".to_string(),
            too_short(2),
        ),
        (
            "a leading dash",
            "-leading".to_string(),
            "Trail name must start with a letter or number.".to_string(),
        ),
        (
            // Outside the character set at the FIRST character: the edge rule is
            // applied before the character-set rule.
            "a leading character outside the character set",
            "!leading".to_string(),
            "Trail name must start with a letter or number.".to_string(),
        ),
        (
            "a trailing period",
            "trailing.".to_string(),
            "Trail name must end with a letter or number.".to_string(),
        ),
        (
            // The recorded case: `bad name!` breaks the terminal-character rule and
            // the character set, and the refusal names the terminal rule.
            "a trailing character outside the character set",
            "bad name!".to_string(),
            "Trail name must end with a letter or number.".to_string(),
        ),
        (
            "the model's own adjacent-punctuation example",
            "my--namespace".to_string(),
            adjacent.clone(),
        ),
        (
            "the model's other adjacent-punctuation example",
            "my-_namespace".to_string(),
            adjacent.clone(),
        ),
        (
            "adjacent periods",
            "my..namespace".to_string(),
            adjacent.clone(),
        ),
        (
            "a space inside an otherwise well-formed name",
            "has space".to_string(),
            charset.clone(),
        ),
        (
            "a non-ASCII letter inside an otherwise well-formed name",
            "hás-accent".to_string(),
            charset.clone(),
        ),
        (
            "the model's own IP-address example",
            "192.168.5.4".to_string(),
            "Trail names must not be formatted as an IP address.".to_string(),
        ),
    ];

    for (case, name, expected) in rows {
        // `CreateTrail` validates the name it is asked to mint...
        let reply = emulator
            .call(
                "CreateTrail",
                &json!({ "Name": name, "S3BucketName": "refusal-bucket" }),
            )
            .await;
        let create_case = format!("CreateTrail with {case}");
        reply.expect_error(&create_case, 400, "InvalidTrailNameException");
        assert_eq!(
            reply.message(),
            expected,
            "case {create_case}: expected {expected:?}, got {:?}",
            reply.message()
        );

        // ...and so does every operation that reads a trail identifier, through the
        // same rules in the same order.
        for (operation, request) in [
            ("GetTrail", json!({ "Name": name })),
            ("DeleteTrail", json!({ "Name": name })),
            ("GetEventSelectors", json!({ "TrailName": name })),
            (
                "PutEventSelectors",
                json!({ "TrailName": name, "EventSelectors": selectors(1) }),
            ),
        ] {
            let case = format!("{operation} with {case}");
            let reply = emulator.call(operation, &request).await;
            reply.expect_error(&case, 400, "InvalidTrailNameException");
            assert_eq!(
                reply.message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }
}

/// A selector set past a published bound, and a request carrying both kinds of
/// selector, are refused with `InvalidEventSelectorsException` (HTTP 400) stating the
/// documented remedy for the rule they broke. Each bound is also exercised AT its
/// published value, where the request succeeds.
#[tokio::test]
async fn an_event_selector_refusal_states_the_documented_remedy() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "refusal-selectors").await;

    let count_remedy = format!(
        "Specify a valid number of selectors (1 to {EVENT_SELECTORS_PER_TRAIL_LIMIT}) for \
         your trail"
    );
    let resources_remedy = format!(
        "Specify a valid number of data resources (1 to {DATA_RESOURCES_PER_TRAIL_LIMIT}) for \
         an event selector"
    );
    let values_remedy = format!(
        "Specify up to {ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT} values for all conditions \
         in all advanced event selectors for a trail"
    );

    // (case, the members the request carries, the refusal it must carry)
    let mut rows: Vec<(String, Value, String)> = vec![
        (
            "both kinds of selector at once".to_string(),
            json!({
                "EventSelectors": selectors(1),
                "AdvancedEventSelectors": advanced_with_values(1),
            }),
            "You can select events by using either EventSelectors or AdvancedEventSelectors, \
             but not both."
                .to_string(),
        ),
        (
            format!(
                "{} data resources, one past the documented maximum",
                DATA_RESOURCES_PER_TRAIL_LIMIT + 1
            ),
            json!({
                "EventSelectors": selectors_with_data_resources(DATA_RESOURCES_PER_TRAIL_LIMIT + 1)
            }),
            resources_remedy,
        ),
        (
            format!(
                "{} advanced condition values, one past the documented maximum",
                ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT + 1
            ),
            json!({
                "AdvancedEventSelectors":
                    advanced_with_values(ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT + 1)
            }),
            values_remedy,
        ),
    ];
    // Every count past the selector bound carries the same remedy, which names the
    // bound and not the count that broke it.
    for count in [
        EVENT_SELECTORS_PER_TRAIL_LIMIT + 1,
        EVENT_SELECTORS_PER_TRAIL_LIMIT + 2,
        EVENT_SELECTORS_PER_TRAIL_LIMIT * 3,
    ] {
        rows.push((
            format!("{count} event selectors, past the documented maximum"),
            json!({ "EventSelectors": selectors(count) }),
            count_remedy.clone(),
        ));
    }

    for (case, members, expected) in rows {
        let mut request = json!({ "TrailName": "refusal-selectors" });
        for (member, value) in members.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let reply = emulator.call("PutEventSelectors", &request).await;
        reply.expect_error(&case, 400, "InvalidEventSelectorsException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }

    // Each bound itself is accepted, so the rows above refuse a bound and not a
    // whole class.
    for (case, request) in [
        (
            format!("{EVENT_SELECTORS_PER_TRAIL_LIMIT} event selectors"),
            json!({
                "TrailName": "refusal-selectors",
                "EventSelectors": selectors(EVENT_SELECTORS_PER_TRAIL_LIMIT),
            }),
        ),
        (
            format!("{DATA_RESOURCES_PER_TRAIL_LIMIT} data resources"),
            json!({
                "TrailName": "refusal-selectors",
                "EventSelectors": selectors_with_data_resources(DATA_RESOURCES_PER_TRAIL_LIMIT),
            }),
        ),
        (
            format!("{ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT} advanced condition values"),
            json!({
                "TrailName": "refusal-selectors",
                "AdvancedEventSelectors":
                    advanced_with_values(ADVANCED_SELECTOR_VALUES_PER_TRAIL_LIMIT),
            }),
        ),
    ] {
        emulator
            .call("PutEventSelectors", &request)
            .await
            .expect_ok(&format!("PutEventSelectors with {case}, the documented bound"));
    }
}

/// A value inside an event selector that is outside its modelled constraint is the
/// modelled `InvalidEventSelectorsException` (HTTP 400) — the error the model's own
/// doc for that shape names this case under ("Specify a valid value for a parameter.
/// For example, specifying the ReadWriteType parameter with a value of read-only is
/// not valid") — and never the generic `ValidationException`, which the operation
/// does not declare. The rows differ in the value, in which selector of the set
/// carries it, and in which selector member is at fault.
#[tokio::test]
async fn a_selector_value_outside_its_modelled_constraint_is_an_event_selector_refusal() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "refusal-values").await;
    // The trail starts out carrying a selector set of its own, so a refusal that
    // wrongly wrote would show up as a changed read.
    emulator
        .call(
            "PutEventSelectors",
            &json!({
                "TrailName": "refusal-values",
                "EventSelectors": [{"ReadWriteType": READ_WRITE_TYPE_VALUES[0]}],
            }),
        )
        .await
        .expect_ok("the configuration the refusals must not disturb");
    let configured = emulator
        .call("GetEventSelectors", &json!({"TrailName": "refusal-values"}))
        .await
        .expect_ok("the configured selectors")
        .clone();

    // (case, the members the request carries) — every row is a value the model's own
    // constraints reject, so the boundary refuses it before the handler runs.
    let rows: Vec<(String, Value)> = vec![
        (
            // The model's own example of an invalid `ReadWriteType`.
            "the model's own invalid ReadWriteType example".to_string(),
            json!({ "EventSelectors": [{"ReadWriteType": "read-only"}] }),
        ),
        (
            "a ReadWriteType in the wrong case".to_string(),
            json!({ "EventSelectors": [{"ReadWriteType": "all"}] }),
        ),
        (
            "an empty ReadWriteType".to_string(),
            json!({ "EventSelectors": [{"ReadWriteType": ""}] }),
        ),
        (
            "an invalid ReadWriteType on the second of three selectors".to_string(),
            json!({ "EventSelectors": [
                {"ReadWriteType": READ_WRITE_TYPE_VALUES[0]},
                {"ReadWriteType": "WriteMostly"},
                {"ReadWriteType": READ_WRITE_TYPE_VALUES[1]},
            ] }),
        ),
        (
            // `SelectorField` is modelled with a `@pattern`, so a field name with a
            // space in it is a constraint violation inside the advanced member.
            "an advanced field name outside its modelled pattern".to_string(),
            json!({ "AdvancedEventSelectors": [{
                "FieldSelectors": [{"Field": "not a field", "Equals": ["Management"]}],
            }] }),
        ),
    ];

    for (case, members) in rows {
        let mut request = json!({ "TrailName": "refusal-values" });
        for (member, value) in members.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let reply = emulator.call("PutEventSelectors", &request).await;
        reply.expect_error(&case, 400, "InvalidEventSelectorsException");
        assert!(
            !reply.message().is_empty(),
            "case {case}: a refusal must say what was wrong with the request, got {reply:?}"
        );

        let read = emulator
            .call("GetEventSelectors", &json!({"TrailName": "refusal-values"}))
            .await;
        assert_eq!(
            read.expect_ok(&case),
            &configured,
            "case {case}: a refused request changed the configured selectors, got {read:?}"
        );
    }

    // Every value the model's enum DOES declare is accepted, so the rows above
    // refuse the value set and not the member.
    for value in READ_WRITE_TYPE_VALUES {
        emulator
            .call(
                "PutEventSelectors",
                &json!({
                    "TrailName": "refusal-values",
                    "EventSelectors": [{"ReadWriteType": value}],
                }),
            )
            .await
            .expect_ok(&format!("PutEventSelectors with ReadWriteType {value}"));
    }
}

/// A resource with no Insights enabled is refused by the read with
/// `InsightNotEnabledException` (HTTP 400) naming that resource plus the documented
/// tail. The rows differ in the resource kind, in the identifier's form, and in
/// whether Insights were never enabled or were turned off again.
#[tokio::test]
async fn an_insights_not_enabled_refusal_names_the_resource_and_the_remedy() {
    let emulator = Emulator::new();
    let store_id = "11111111-1111-4111-8111-111111111111";
    create_trail(&emulator, HOME_REGION, "insights-never").await;
    create_trail(&emulator, HOME_REGION, "insights-off").await;
    emulator.state().lock().event_data_stores.insert(
        store_id.to_string(),
        EventDataStoreRecord {
            id: store_id.to_string(),
            name: format!("lake-{store_id}"),
            region: HOME_REGION.to_string(),
            status: EventDataStoreState::Enabled,
            ..EventDataStoreRecord::default()
        },
    );
    // Insights enabled and then turned off again by an empty statement, which is how
    // the model says a caller turns them off.
    emulator
        .call(
            "PutInsightSelectors",
            &json!({ "TrailName": "insights-off", "InsightSelectors": insights_statement() }),
        )
        .await
        .expect_ok("enable Insights on insights-off");
    emulator
        .call(
            "PutInsightSelectors",
            &json!({ "TrailName": "insights-off", "InsightSelectors": [] }),
        )
        .await
        .expect_ok("turn Insights off again on insights-off");

    let tail = "does not have Insights enabled. Edit the trail settings to enable Insights, \
                and then try the operation again.";

    // (case, the request the read is made with, the subject the refusal names)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a trail by name, Insights never enabled",
            json!({ "TrailName": "insights-never" }),
            format!("Trail {}", trail_arn(HOME_REGION, "insights-never")),
        ),
        (
            "a trail by ARN, Insights never enabled",
            json!({ "TrailName": trail_arn(HOME_REGION, "insights-never") }),
            format!("Trail {}", trail_arn(HOME_REGION, "insights-never")),
        ),
        (
            "a trail whose Insights were turned off again",
            json!({ "TrailName": "insights-off" }),
            format!("Trail {}", trail_arn(HOME_REGION, "insights-off")),
        ),
        (
            "an event data store by id",
            json!({ "EventDataStore": store_id }),
            format!(
                "Event data store {}",
                event_data_store_arn(HOME_REGION, store_id)
            ),
        ),
        (
            "an event data store by ARN",
            json!({ "EventDataStore": event_data_store_arn(HOME_REGION, store_id) }),
            format!(
                "Event data store {}",
                event_data_store_arn(HOME_REGION, store_id)
            ),
        ),
    ];

    for (case, request, subject) in rows {
        let expected = format!("{subject} {tail}");
        let reply = emulator.call("GetInsightSelectors", &request).await;
        reply.expect_error(case, 400, "InsightNotEnabledException");
        assert_eq!(
            reply.message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message()
        );
    }
}

/// `DescribeTrails` SELECTS the trails to report: an entry that names no trail of the
/// caller's account narrows the answer instead of failing the request — including an
/// entry that can name no trail at all. The rows differ in which kinds of entry the
/// list mixes.
#[tokio::test]
async fn describe_trails_omits_every_entry_that_names_no_trail() {
    let emulator = Emulator::new();
    create_trail(&emulator, HOME_REGION, "described-one").await;
    create_trail(&emulator, HOME_REGION, "described-two").await;
    let one = trail_arn(HOME_REGION, "described-one");

    // Values that can name no trail at all: an ARN that is not a trail ARN, and
    // names outside the documented name rules.
    let unresolvable = [
        "arn:aws:cloudtrail:bad".to_string(),
        "arn:aws:s3:::a-bucket".to_string(),
        format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/x"),
        "my--namespace".to_string(),
        "ab".to_string(),
        "192.168.5.4".to_string(),
        "has space".to_string(),
    ];

    // (case, trailNameList, the trail names the answer must carry — always
    //  name-sorted, however the request ordered them)
    let mut rows: Vec<(String, Vec<String>, Vec<&str>)> = vec![
        (
            "one stored name".to_string(),
            vec!["described-one".to_string()],
            vec!["described-one"],
        ),
        (
            "a stored name and a stored ARN, in reverse order".to_string(),
            vec!["described-two".to_string(), one.clone()],
            vec!["described-one", "described-two"],
        ),
        (
            "a stored name repeated".to_string(),
            vec!["described-one".to_string(), "described-one".to_string()],
            vec!["described-one"],
        ),
        (
            "a stored name beside a name nothing carries".to_string(),
            vec!["described-one".to_string(), "no-such-trail".to_string()],
            vec!["described-one"],
        ),
        (
            "a stored name beside a well-formed ARN naming no trail".to_string(),
            vec![
                "described-one".to_string(),
                trail_arn("eu-west-1", "described-one"),
            ],
            vec!["described-one"],
        ),
        (
            "nothing but entries that name no trail".to_string(),
            unresolvable.to_vec(),
            Vec::new(),
        ),
    ];
    // One row per kind of unresolvable entry, each beside a stored name: the stored
    // trail is still reported.
    for entry in &unresolvable {
        rows.push((
            format!("a stored name beside {entry:?}"),
            vec!["described-one".to_string(), entry.clone()],
            vec!["described-one"],
        ));
    }

    for (case, list, expected) in rows {
        let reply = emulator
            .call("DescribeTrails", &json!({ "trailNameList": list }))
            .await;
        let body = reply.expect_ok(&case);
        let names: Vec<String> = body["trailList"]
            .as_array()
            .map(|trails| {
                trails
                    .iter()
                    .map(|trail| trail["Name"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names, expected,
            "case {case}: expected {expected:?}, got {names:?}"
        );
    }
}

/// `PutEventConfiguration` judges the request's OWN parameters BEFORE it resolves the
/// resource it was given, which is the order the recorded CloudTrail response shows: a
/// call that does not state the configuration to apply is refused on the parameters
/// whether or not a store of that identifier exists, and the store's absence is
/// reported only once the parameters hold. The rows differ in which parameter the
/// request is missing or cannot combine.
#[tokio::test]
async fn put_event_configuration_judges_its_parameters_before_its_resource() {
    let emulator = Emulator::new();
    let absent_store = "44444444-4444-4444-8444-444444444444";
    let absent_arn = event_data_store_arn(HOME_REGION, absent_store);
    let selectors = json!([{ "Type": "RequestContext", "Equals": ["ctx:Key0"] }]);
    let store_not_found = format!("The event data store {absent_arn} was not found.");

    // (case, the members the request carries beside the absent store, the error code
    // and the refusal text it is answered with).
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "neither the size nor the selectors",
            json!({}),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        (
            "the size but no selectors member",
            json!({ "MaxEventSize": "Large" }),
            "ValidationException",
            MISSING_EVENT_DATA_STORE_CONFIGURATION_MESSAGE.to_string(),
        ),
        (
            "context key selectors with a Standard maximum event size",
            json!({ "MaxEventSize": "Standard", "ContextKeySelectors": selectors }),
            "InvalidParameterCombinationException",
            "MaxEventSize must be Large when ContextKeySelectors are set, got Standard."
                .to_string(),
        ),
        (
            "a trail-level member",
            json!({
                "MaxEventSize": "Large",
                "ContextKeySelectors": selectors,
                "AggregationConfigurations": aggregations(),
            }),
            "InvalidParameterCombinationException",
            "AggregationConfigurations configures a trail, so it cannot be set with \
             EventDataStore."
                .to_string(),
        ),
        (
            // The one row whose parameters HOLD: with nothing left to refuse about the
            // request, the store it names is resolved and its absence reported.
            "a combination a stored store would accept",
            json!({ "MaxEventSize": "Large", "ContextKeySelectors": [] }),
            "EventDataStoreNotFoundException",
            store_not_found.clone(),
        ),
    ];

    for (case, members, code, expected) in rows {
        for identifier in [absent_store.to_string(), absent_arn.clone()] {
            let mut request = json!({ "EventDataStore": identifier });
            for (member, value) in members.as_object().expect("the row is an object") {
                request[member] = value.clone();
            }
            let case = format!("PutEventConfiguration on an absent store with {case}");
            let reply = emulator.call("PutEventConfiguration", &request).await;
            reply.expect_error(&case, NOT_FOUND_STATUS, code);
            assert_eq!(
                reply.message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                reply.message()
            );
        }
    }

    // The three siblings that address the same store carry no configuration members at
    // all, so each answers the not-found of the store it was given.
    for (operation, request) in [
        (
            "EnableFederation",
            json!({
                "EventDataStore": absent_arn,
                "FederationRoleArn": "arn:aws:iam::000000000000:role/federation",
            }),
        ),
        ("DisableFederation", json!({ "EventDataStore": absent_arn })),
        (
            "GetEventConfiguration",
            json!({ "EventDataStore": absent_arn }),
        ),
    ] {
        let reply = emulator.call(operation, &request).await;
        reply.expect_error(operation, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
        assert_eq!(
            reply.message(),
            store_not_found,
            "case {operation}: expected {store_not_found:?}, got {:?}",
            reply.message()
        );
    }

    // The parameter rules hold for a store that EXISTS too: the order they are judged
    // in does not retire them.
    let store_id = "11111111-1111-4111-8111-111111111111";
    emulator.state().lock().event_data_stores.insert(
        store_id.to_string(),
        EventDataStoreRecord {
            id: store_id.to_string(),
            name: format!("lake-{store_id}"),
            region: HOME_REGION.to_string(),
            status: EventDataStoreState::Enabled,
            ..EventDataStoreRecord::default()
        },
    );
    emulator
        .call(
            "PutEventConfiguration",
            &json!({
                "EventDataStore": store_id,
                "MaxEventSize": "Standard",
                "ContextKeySelectors": selectors,
            }),
        )
        .await
        .expect_error(
            "PutEventConfiguration on a stored store with an invalid combination",
            400,
            "InvalidParameterCombinationException",
        );
}
