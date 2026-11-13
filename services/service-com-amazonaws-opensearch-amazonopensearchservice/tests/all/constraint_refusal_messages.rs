// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE this service refuses a request whose input breaks a
//! constraint with: the `400 ValidationException` message a client reads the member path,
//! the echoed value and the requirement out of.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second
//! copy of it.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one call:
//!
//! 1. a `@pattern` requirement QUOTES the expression the service advertises, which is the
//!    model's text without the whole-value pair the model wraps it in — asserted on five
//!    members of five operations, over three different expressions;
//! 2. a member whose live validation surface the model has not caught up with states THAT
//!    surface: the expression the service validates it against, and a length bound it
//!    states a clause of its own for;
//! 3. an enumerated type's value set is stated in the order this front door walks its own
//!    hash table over the admitted values — neither the model's declaration order nor
//!    lexicographic — asserted over four enums, two of which the model declares in an
//!    order of its own and one of which has two values in one bucket;
//! 4. a role ARN that cannot be read as one replaces the whole constraint report, and
//!    only when it is the role ARN that cannot be read;
//! 5. a member whose shape the boundary cannot check — a `GUID`, whose `@length` the
//!    boundary enforces and whose `@pattern` it does not — is refused with the SAME
//!    constraint report a member the boundary does check is;
//! 6. the catalog an unsupported engine version is listed against is the published one,
//!    newest release first;
//! 7. `StartMigration` states its own report over the three members it validates, in its
//!    own order, and serves a request it refuses nothing in.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    INSIGHT_FEEDBACK_THUMBS_VALUES, INSIGHT_SORT_ORDER_VALUES, MAINTENANCE_STATUS_VALUES,
    MAINTENANCE_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::published_engine_versions;
use service_common::validation_message::hashed_enum_order;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// A `DomainName` the model's pattern admits, so a refusal under test is about the member
/// the row exercises and nothing else.
const WELL_FORMED_DOMAIN: &str = "constraint-messages";

/// An IAM role ARN inside `DirectQueryDataSourceRoleArn`'s own `@length(32..200)` and
/// `@pattern` (`smithy_query(query="shape",shape="DirectQueryDataSourceRoleArn")`), so a
/// row carrying it is refused for the member it is exercising rather than for the role.
const WELL_FORMED_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/OpenSearchDirectQuery";

/// A `GUID` of the right length whose groups are not hexadecimal: the shape the request
/// boundary admits (`@length(36..36)`) and the `@pattern` refuses.
const MALFORMED_GUID: &str = "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz";

/// A `GUID` the pattern admits, for the rows that pin what is NOT refused as malformed.
const WELL_FORMED_GUID: &str = "0f0e0d0c-0b0a-4009-8008-070605040302";

/// The expression `GUID`'s clause quotes: the model's `@pattern`
/// (`smithy_query(query="shape",shape="GUID")`) without the whole-value pair.
const GUID_EXPRESSION: &str =
    r"\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}";

/// The `message` the answer states, with the case named on failure.
pub(crate) fn message(answer: &Answer, case: &str) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a refusal message, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// Assert that `request` is refused `400 ValidationException` stating `expected`.
async fn refuses(
    emulator: &Emulator,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected: &str,
) {
    let answer = emulator.call(method, uri, body).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    let stated = message(&answer, case);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected {expected:?}, got {stated:?}"
    );
}

/// One clause of a report: the value echoed, the member path it is stated at and the
/// requirement it broke.
pub(crate) fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The whole report over `clauses`, in the envelope AWS states one in.
pub(crate) fn report(clauses: &[String]) -> String {
    match clauses {
        [one] => format!("1 validation error detected: {one}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// `Member must satisfy regular expression pattern: <expression>`, the framework's own
/// requirement sentence for a `@pattern`.
pub(crate) fn expression(stated: &str) -> String {
    format!("Member must satisfy regular expression pattern: {stated}")
}

/// `Member must satisfy enum value set: [<values>]`.
///
/// Reached by a sibling suite as `super::constraint_refusal_messages::value_set`, so the
/// crate states that sentence in ONE place.
pub(crate) fn value_set(stated: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", stated.join(", "))
}

/// A `DataSourceType` union naming the S3 Glue Data Catalog under `role_arn`.
fn s3_glue(role_arn: &str) -> Value {
    json!({ "S3GlueDataCatalog": { "RoleArn": role_arn } })
}

/// A `DirectQueryDataSourceType` union naming CloudWatch Logs under `role_arn`.
fn cloud_watch(role_arn: &str) -> Value {
    json!({ "CloudWatchLog": { "RoleArn": role_arn } })
}

/// One row of a ONE-clause table: the request to make, and the clause the answer has to
/// state.
struct OneClause {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The value the clause echoes.
    value: &'static str,
    /// The member path the clause is stated at.
    path: &'static str,
    /// The requirement the clause states, as the framework words it.
    requirement: String,
}

/// Drive every row of a one-clause table and assert the whole message of each.
async fn each_states_one_clause(emulator: &Emulator, rows: Vec<OneClause>) {
    for row in rows {
        let expected = report(&[clause(row.value, row.path, &row.requirement)]);
        refuses(
            emulator,
            row.case,
            row.method,
            &row.uri,
            row.body,
            &expected,
        )
        .await;
    }
}

/// RULE 1: a `@pattern` requirement quotes the expression the service advertises — the
/// model's text with the whole-value pair it wraps that expression in dropped.
#[tokio::test]
async fn a_pattern_requirement_quotes_the_expression_without_its_whole_value_pair() {
    let emulator = Emulator::new();

    // The rows differ in the expression, in the member, in where the value travels (a URI
    // label, a body member, a member nested in a structure) and in the operation.
    let rows: Vec<OneClause> = vec![
        OneClause {
            // `DomainName`: `@pattern("^[a-z][a-z0-9\\-]+$")`.
            case: "a domain name in a body member",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain"),
            body: Some(json!({ "DomainName": "Probe_BAD" })),
            value: "Probe_BAD",
            path: "domainName",
            requirement: expression(r"[a-z][a-z0-9\-]+"),
        },
        OneClause {
            case: "a domain name in a URI label",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/Probe_BAD/domainMaintenances"),
            body: None,
            value: "Probe_BAD",
            path: "domainName",
            requirement: expression(r"[a-z][a-z0-9\-]+"),
        },
        OneClause {
            // `DirectQueryDataSourceName`: `@pattern("^[a-z][a-z0-9_]+$")`.
            case: "a direct query data source name",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/directQueryDataSource/probe-BAD"),
            body: None,
            value: "probe-BAD",
            path: "dataSourceName",
            requirement: expression("[a-z][a-z0-9_]+"),
        },
        OneClause {
            // `DataSourceName`, the same expression on another operation's body member.
            case: "a domain data source name",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/{WELL_FORMED_DOMAIN}/dataSource"),
            body: Some(json!({
                "Name": "probe-BAD",
                "DataSourceType": s3_glue(WELL_FORMED_ROLE_ARN),
            })),
            value: "probe-BAD",
            path: "name",
            requirement: expression("[a-z][a-z0-9_]+"),
        },
        OneClause {
            // `InsightEntityValue`: `@pattern("^([a-z][a-z0-9\\-]+|\\d{12})$")` — an
            // expression whose alternation the model already grouped, so the pair comes
            // off and the group stays.
            case: "an insight entity value inside a structure",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/insights"),
            body: Some(json!({ "Entity": { "Type": "DomainName", "Value": "Probe_BAD" } })),
            value: "Probe_BAD",
            path: "entity.value",
            requirement: expression(r"([a-z][a-z0-9\-]+|\d{12})"),
        },
    ];

    each_states_one_clause(&emulator, rows).await;
}

/// RULE 2: a member whose live validation surface the model has not caught up with states
/// THAT surface — the expression the service validates it against, and the length bound it
/// states a clause of its own for, in that order.
#[tokio::test]
async fn a_member_the_model_lags_states_the_surface_the_service_validates() {
    let emulator = Emulator::new();

    /// The expression an application `id` is validated against and quoted with.
    const ID_EXPRESSION: &str = r"^(?!-)[a-z0-9\-]{3,30}$";
    /// The expression an `applicationId` is quoted with.
    const APPLICATION_ID_EXPRESSION: &str = "[a-z0-9-]{3,30}";
    /// The bound an `applicationId` states its own clause for.
    const APPLICATION_ID_LENGTH: &str = "Member must have length less than or equal to 30";
    /// An `applicationId` past that bound: 31 characters.
    const OVER_LONG_APPLICATION_ID: &str = "thirty-one-characters-long-x-id";
    /// One the bound admits and the expression does not.
    const MALFORMED_APPLICATION_ID: &str = "probe_BAD";

    // (case, method, uri, body, the whole message). The rows differ in the member, in the
    // operation, in how the value travels and in how many of the member's requirements
    // one value breaks.
    let rows: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "an application id read by a GET",
            "GET",
            format!("{PREFIX}/opensearch/application/{WELL_FORMED_GUID}"),
            None,
            report(&[clause(
                WELL_FORMED_GUID,
                "id",
                &expression(ID_EXPRESSION),
            )]),
        ),
        (
            "the same member on a DELETE",
            "DELETE",
            format!("{PREFIX}/opensearch/application/probe_BAD"),
            None,
            report(&[clause("probe_BAD", "id", &expression(ID_EXPRESSION))]),
        ),
        (
            "the same member on a PUT carrying a body",
            "PUT",
            format!("{PREFIX}/opensearch/application/probe_BAD"),
            Some(json!({})),
            report(&[clause("probe_BAD", "id", &expression(ID_EXPRESSION))]),
        ),
        (
            "an applicationId the expression refuses and the bound admits",
            "GET",
            format!("{PREFIX}/opensearch/app-migrations?applicationId={MALFORMED_APPLICATION_ID}"),
            None,
            report(&[clause(
                MALFORMED_APPLICATION_ID,
                "applicationId",
                &expression(APPLICATION_ID_EXPRESSION),
            )]),
        ),
        (
            "an applicationId past the bound as well, stated as two clauses",
            "GET",
            format!(
                "{PREFIX}/opensearch/app-migrations?applicationId={OVER_LONG_APPLICATION_ID}"
            ),
            None,
            report(&[
                clause(
                    OVER_LONG_APPLICATION_ID,
                    "applicationId",
                    &expression(APPLICATION_ID_EXPRESSION),
                ),
                clause(
                    OVER_LONG_APPLICATION_ID,
                    "applicationId",
                    APPLICATION_ID_LENGTH,
                ),
            ]),
        ),
        (
            // The capability operations state the capability's own name before the id of
            // the application that scopes it, and this model declares the id first — so
            // this row pins the ORDER of the clauses as much as their set.
            "a capability name and an application id together",
            "GET",
            format!(
                "{PREFIX}/opensearch/application/{OVER_LONG_APPLICATION_ID}/capability/BAD_NAME"
            ),
            None,
            report(&[
                clause(
                    "BAD_NAME",
                    "capabilityName",
                    &expression("^[a-zA-Z0-9-]+$"),
                ),
                clause(
                    OVER_LONG_APPLICATION_ID,
                    "applicationId",
                    &expression(APPLICATION_ID_EXPRESSION),
                ),
                clause(
                    OVER_LONG_APPLICATION_ID,
                    "applicationId",
                    APPLICATION_ID_LENGTH,
                ),
            ]),
        ),
        (
            "the same pair on the operation that deregisters a capability",
            "DELETE",
            format!(
                "{PREFIX}/opensearch/application/{MALFORMED_APPLICATION_ID}/capability/\
                 deregister/BAD_NAME"
            ),
            None,
            report(&[
                clause(
                    "BAD_NAME",
                    "capabilityName",
                    &expression("^[a-zA-Z0-9-]+$"),
                ),
                clause(
                    MALFORMED_APPLICATION_ID,
                    "applicationId",
                    &expression(APPLICATION_ID_EXPRESSION),
                ),
            ]),
        ),
    ];

    for (case, method, uri, body, expected) in rows {
        refuses(&emulator, case, method, &uri, body, &expected).await;
    }
}

/// RULE 3: an enumerated type's value set is stated in the order this front door walks its
/// own hash table over the admitted values, whatever order the model declares its members
/// in and whatever order codegen hands the walk them in.
#[tokio::test]
async fn an_enum_value_set_is_stated_in_the_order_the_front_door_walks_it() {
    let emulator = Emulator::new();

    // Every set is the member's WHOLE declared set, read off the generated constant the
    // model produced (`smithy_query(query="shape",shape=…)`); the ORDER is the rule's one
    // home (`service_common::validation_message::hashed_enum_order`), so what each row
    // pins is the membership and that the clause is listed by that rule rather than
    // sorted. Two of the four are declared in an order of the model's own, and the last
    // one's two values share a bucket — which is what pins that a collision is listed the
    // way the table's own chain is read rather than alphabetically.
    let rows: Vec<OneClause> = vec![
        OneClause {
            // `InsightFeedbackThumbs`, declared `[Up, Down]`.
            case: "a two-value set the model declares unsorted",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/insight-feedback"),
            body: Some(json!({
                "Entity": { "Type": "DomainName", "Value": WELL_FORMED_DOMAIN },
                "InsightId": WELL_FORMED_GUID,
                "Thumbs": "Sideways",
            })),
            value: "Sideways",
            path: "thumbs",
            requirement: value_set(&hashed_enum_order(INSIGHT_FEEDBACK_THUMBS_VALUES)),
        },
        OneClause {
            // `MaintenanceType`, declared
            // `[REBOOT_NODE, RESTART_SEARCH_PROCESS, RESTART_DASHBOARD]`.
            case: "a three-value set the model declares unsorted",
            method: "GET",
            uri: format!(
                "{PREFIX}/opensearch/domain/{WELL_FORMED_DOMAIN}/domainMaintenances\
                 ?action=RESTART_EVERYTHING"
            ),
            body: None,
            value: "RESTART_EVERYTHING",
            path: "action",
            requirement: value_set(&hashed_enum_order(MAINTENANCE_TYPE_VALUES)),
        },
        OneClause {
            // `MaintenanceStatus`, declared
            // `[PENDING, IN_PROGRESS, COMPLETED, FAILED, TIMED_OUT]`.
            case: "a five-value set the model declares in lifecycle order",
            method: "GET",
            uri: format!(
                "{PREFIX}/opensearch/domain/{WELL_FORMED_DOMAIN}/domainMaintenances\
                 ?status=HALF_DONE"
            ),
            body: None,
            value: "HALF_DONE",
            path: "status",
            requirement: value_set(&hashed_enum_order(MAINTENANCE_STATUS_VALUES)),
        },
        OneClause {
            // `InsightSortOrder`, declared `[ASC, DESC]` — two values in one bucket.
            case: "a set whose two values share a bucket",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/insights"),
            body: Some(json!({
                "Entity": { "Type": "DomainName", "Value": WELL_FORMED_DOMAIN },
                "SortOrder": "SIDEWAYS",
            })),
            value: "SIDEWAYS",
            path: "sortOrder",
            requirement: value_set(&hashed_enum_order(INSIGHT_SORT_ORDER_VALUES)),
        },
    ];

    // The order is the rule's, and it is NOT the sort: a set listed alphabetically would
    // satisfy the rows above only if the two agreed, so one row states both.
    let bucketed = hashed_enum_order(MAINTENANCE_STATUS_VALUES);
    let mut sorted = bucketed.clone();
    sorted.sort_unstable();
    assert_ne!(
        bucketed, sorted,
        "case MaintenanceStatus: the order under test must differ from the sort, got \
         {bucketed:?}"
    );

    each_states_one_clause(&emulator, rows).await;
}

/// RULE 4: a role ARN that cannot be read as one is refused with that one sentence and no
/// constraint report at all — and only a role ARN does that, so a request whose role is
/// readable keeps the report.
#[tokio::test]
async fn an_unreadable_role_arn_replaces_the_whole_constraint_report() {
    let emulator = Emulator::new();

    /// A value no IAM role ARN can be read out of, and long enough to be outside the
    /// member's expression rather than its length bound.
    const UNPARSEABLE_ROLE_ARN: &str = "arn:aws:iam::role/not-a-role-arn-at-all-whatsoever";
    /// One SHORTER than the member's declared minimum, which the same sentence answers:
    /// it is no readable role ARN either.
    const SHORT_ROLE_ARN: &str = "arn:aws:iam::role/x";

    // (case, method, uri, body, the ARN the sentence names). The rows differ in the
    // operation, in the union branch the role travels in, in why the ARN is unreadable
    // and in whether another member of the same request is invalid too.
    let rows: Vec<(&str, &str, String, Value, &str)> = vec![
        (
            "a direct query data source being added",
            "POST",
            format!("{PREFIX}/opensearch/directQueryDataSource"),
            json!({
                "DataSourceName": "probe_source",
                "DataSourceType": cloud_watch(UNPARSEABLE_ROLE_ARN),
            }),
            UNPARSEABLE_ROLE_ARN,
        ),
        (
            "the same add with an invalid data source name beside it",
            "POST",
            format!("{PREFIX}/opensearch/directQueryDataSource"),
            json!({
                "DataSourceName": "probe-BAD",
                "DataSourceType": cloud_watch(UNPARSEABLE_ROLE_ARN),
            }),
            UNPARSEABLE_ROLE_ARN,
        ),
        (
            "a direct query data source being updated, with a role ARN too short to read",
            "PUT",
            format!("{PREFIX}/opensearch/directQueryDataSource/probe_source"),
            json!({ "DataSourceType": cloud_watch(SHORT_ROLE_ARN) }),
            SHORT_ROLE_ARN,
        ),
        (
            "a domain data source being added",
            "POST",
            format!("{PREFIX}/opensearch/domain/{WELL_FORMED_DOMAIN}/dataSource"),
            json!({
                "Name": "probe_source",
                "DataSourceType": s3_glue(UNPARSEABLE_ROLE_ARN),
            }),
            UNPARSEABLE_ROLE_ARN,
        ),
        (
            "a domain data source being updated, with an invalid name beside it",
            "PUT",
            format!("{PREFIX}/opensearch/domain/{WELL_FORMED_DOMAIN}/dataSource/probe-BAD"),
            json!({ "DataSourceType": s3_glue(UNPARSEABLE_ROLE_ARN) }),
            UNPARSEABLE_ROLE_ARN,
        ),
    ];

    for (case, method, uri, body, role_arn) in rows {
        let expected = format!("Invalid Role Arn : {role_arn}");
        refuses(&emulator, case, method, &uri, Some(body), &expected).await;
    }

    // And the other way round: a role ARN that reads as one leaves the request's other
    // members to the constraint report.
    let expected = report(&[clause(
        "probe-BAD",
        "dataSourceName",
        &expression("[a-z][a-z0-9_]+"),
    )]);
    refuses(
        &emulator,
        "a readable role ARN beside an invalid data source name",
        "POST",
        &format!("{PREFIX}/opensearch/directQueryDataSource"),
        Some(json!({
            "DataSourceName": "probe-BAD",
            "DataSourceType": cloud_watch(WELL_FORMED_ROLE_ARN),
        })),
        &expected,
    )
    .await;
}

/// RULE 5: a constraint the request boundary cannot check — a `GUID`'s `@pattern`, which
/// only its `@length` reaches the boundary with — is stated as the SAME constraint report
/// a member the boundary does check is refused with.
#[tokio::test]
async fn a_constraint_checked_after_the_boundary_states_the_same_report() {
    let emulator = Emulator::new();

    // (case, method, uri, body, the member path the clause names).
    let rows: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "an offering id filtering a listing",
            "GET",
            format!(
                "{PREFIX}/opensearch/reservedInstanceOfferings?offeringId={MALFORMED_GUID}"
            ),
            None,
            "reservedInstanceOfferingId",
        ),
        (
            "an offering id naming what to purchase",
            "POST",
            format!("{PREFIX}/opensearch/purchaseReservedInstanceOffering"),
            Some(json!({
                "ReservedInstanceOfferingId": MALFORMED_GUID,
                "ReservationName": "probe-reservation",
                "InstanceCount": 1,
            })),
            "reservedInstanceOfferingId",
        ),
        (
            "a reserved instance id filtering a listing",
            "GET",
            format!("{PREFIX}/opensearch/reservedInstances?reservationId={MALFORMED_GUID}"),
            None,
            "reservedInstanceId",
        ),
        (
            // The id a detail read addresses one insight by, judged for its shape before
            // the address it belongs to is resolved — so the entity the row names needs
            // to hold nothing for the clause to be the answer.
            "an insight id naming the insight to detail",
            "POST",
            format!("{PREFIX}/opensearch/insight-details"),
            Some(json!({
                "Entity": { "Type": "Account" },
                "InsightId": MALFORMED_GUID,
            })),
            "insightId",
        ),
        (
            // The same id on the submission that RATES one insight: one shape rule for
            // both operations that address an insight by id.
            "an insight id naming the insight to rate",
            "POST",
            format!("{PREFIX}/opensearch/insight-feedback"),
            Some(json!({
                "Entity": { "Type": "DomainName", "Value": WELL_FORMED_DOMAIN },
                "InsightId": MALFORMED_GUID,
                "Thumbs": "Up",
            })),
            "insightId",
        ),
    ];

    for (case, method, uri, body, path) in rows {
        let expected = report(&[clause(
            MALFORMED_GUID,
            path,
            &expression(GUID_EXPRESSION),
        )]);
        refuses(&emulator, case, method, &uri, body, &expected).await;
    }

    // A value the expression admits is not refused as malformed: the listing it filters
    // simply holds no offering under it.
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/reservedInstanceOfferings?offeringId={WELL_FORMED_GUID}"),
            None,
        )
        .await;
    assert_ne!(
        answer.status, 400,
        "case a well-formed offering id: expected no constraint refusal, got {} with {:?}",
        answer.status, answer.body
    );
}

/// RULE 6: the catalog an unsupported engine version is listed against is the one this
/// service PUBLISHES, newest release first — the same order and the same set every other
/// answer hands the catalog over in.
#[tokio::test]
async fn an_unsupported_engine_version_lists_the_published_catalog() {
    let emulator = Emulator::new();

    // (case, uri, the release the request names) — the rows differ in the operation and
    // in which engine line the unsupported release names. Every value is inside
    // `VersionString`'s own pattern and length, so what refuses them is the catalog and
    // not the boundary.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a listing of instance type details",
            format!("{PREFIX}/opensearch/instanceTypeDetails/OpenSearch_9.9"),
            "OpenSearch_9.9",
        ),
        (
            "the limits of one instance type",
            format!("{PREFIX}/opensearch/instanceTypeLimits/OpenSearch_9.9/r6g.large.search"),
            "OpenSearch_9.9",
        ),
        (
            "a release of the older engine line",
            format!("{PREFIX}/opensearch/instanceTypeDetails/Elasticsearch_9.9"),
            "Elasticsearch_9.9",
        ),
    ];

    let published = published_engine_versions();
    assert_eq!(
        published.first().map(|version| version.starts_with("OpenSearch_")),
        Some(true),
        "the published catalog leads with the newest release, got {published:?}"
    );

    for (case, uri, version) in rows {
        let expected = format!(
            "Unsupported Version: {version}, allowed Versions are: [{}]",
            published.join(", ")
        );
        refuses(&emulator, case, "GET", &uri, None, &expected).await;
    }
}

/// RULE 7: `StartMigration` states its own report over the three members it validates —
/// the workspace type, the shape of the source ARN and the application id — in its own
/// order, and a request it refuses nothing in reaches the handler.
#[tokio::test]
async fn start_migration_states_its_own_report_over_the_members_it_validates() {
    let emulator = Emulator::new();

    /// The source ARN shape the operation validates against, as its clause quotes it.
    const SOURCE_EXPRESSION: &str =
        r"arn:aws[a-z\-]*:aoss:[a-z0-9\-]+:[0-9]{12}:collection/[a-z0-9]+";
    /// The application id expression this operation quotes.
    const APPLICATION_ID_EXPRESSION: &str = "[a-z0-9]+";
    /// The workspace types the operation lists, in the order it lists them.
    const WORKSPACE_TYPES: [&str; 5] = [
        "ALL",
        "OBSERVABILITY",
        "ESSENTIALS",
        "SECURITY_ANALYTICS",
        "SEARCH",
    ];
    /// A collection ARN the expression admits.
    const WELL_FORMED_SOURCE: &str = "arn:aws:aoss:us-east-1:123456789012:collection/probe";
    /// An ARN of the right vendor naming another resource.
    const WRONG_RESOURCE_SOURCE: &str = "arn:aws:es:us-east-1:123456789012:domain/probe";

    /// The body of a start naming `application_id`, `source` and `workspace_type`.
    fn start(application_id: &str, source: &str, workspace_type: Option<&str>) -> Value {
        let mut workspace = json!({ "createWorkspace": true, "name": "Imported" });
        match workspace_type {
            Some(stated) => workspace["type"] = json!(stated),
            None => workspace = json!({ "workspaceId": "ws-probe" }),
        }
        json!({
            "applicationId": application_id,
            "migrationOptions": { "source": { "datasourceArn": source }, "workspace": workspace },
        })
    }

    // (case, the body, the clauses stated). The rows differ in which of the three members
    // offends, and the last one in all three offending together — which is what pins the
    // ORDER the operation states them in.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "a workspace type outside the set",
            start("probeapplication", WELL_FORMED_SOURCE, Some("private")),
            vec![clause(
                "private",
                "migrationOptions.workspace.type",
                &value_set(&WORKSPACE_TYPES),
            )],
        ),
        (
            "a source ARN naming another resource",
            start("probeapplication", WRONG_RESOURCE_SOURCE, None),
            vec![clause(
                WRONG_RESOURCE_SOURCE,
                "migrationOptions.source.datasourceArn",
                &expression(SOURCE_EXPRESSION),
            )],
        ),
        (
            "a source that is no ARN at all",
            start("probeapplication", "probe-source", None),
            vec![clause(
                "probe-source",
                "migrationOptions.source.datasourceArn",
                &expression(SOURCE_EXPRESSION),
            )],
        ),
        (
            "an application id outside the expression",
            start("probe-application", WELL_FORMED_SOURCE, None),
            vec![clause(
                "probe-application",
                "applicationId",
                &expression(APPLICATION_ID_EXPRESSION),
            )],
        ),
        (
            "all three members offending at once",
            start("probe-application", WRONG_RESOURCE_SOURCE, Some("private")),
            vec![
                clause(
                    "private",
                    "migrationOptions.workspace.type",
                    &value_set(&WORKSPACE_TYPES),
                ),
                clause(
                    WRONG_RESOURCE_SOURCE,
                    "migrationOptions.source.datasourceArn",
                    &expression(SOURCE_EXPRESSION),
                ),
                clause(
                    "probe-application",
                    "applicationId",
                    &expression(APPLICATION_ID_EXPRESSION),
                ),
            ],
        ),
    ];

    for (case, body, clauses) in rows {
        let expected = report(&clauses);
        refuses(
            &emulator,
            case,
            "POST",
            &format!("{PREFIX}/opensearch/app-migrations"),
            Some(body),
            &expected,
        )
        .await;
    }

    // A request this validation refuses nothing in reaches the handler, which answers for
    // the application the id names — nothing here holds one, so the answer is that miss
    // rather than a constraint report.
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/app-migrations"),
            Some(start("probeapplication", WELL_FORMED_SOURCE, Some("SEARCH"))),
        )
        .await;
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case a start this validation admits: expected the handler's own answer, got {} \
         with {:?}",
        answer.status,
        answer.body
    );
}
