// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE status this service answers a missing resource on,
//! across every operation that can decide a request addresses nothing.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness and the shared helpers come from the
//! sibling suites; this one adds no second copy of either.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one call:
//!
//! * a refusal that carries this service's missing-resource error shape is answered on
//!   [`RECORDED_NOT_FOUND_STATUS`] — one status for every operation, HTTP method,
//!   resource kind and path shape, whatever status that shape's own `@httpError`
//!   declares — with the shape's own error code in `x-amzn-errortype` and its own
//!   message in the body, neither of which this rule touches;
//! * the status is decided by the error SHAPE alone: a refusal of the same operations
//!   that carries another modelled shape — the already-exists, the conflict, the
//!   generic envelope, the client-input error — keeps the status the model gives that
//!   shape, so nothing but the missing-resource refusal moved.

use serde_json::{json, Value};

use super::control_plane_refusal_wording::message;
use super::domain_lifecycle::{
    create_baseline, error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::insight_details::guid;

/// A `ConnectionId` no cross-cluster connection of this account carries. Its shape is
/// the one `CreateOutboundConnection` mints (`cc-` and a suffix), so the request reaches
/// the handler rather than the request boundary.
const ABSENT_CONNECTION_ID: &str = "cc-nosuchconnection";

/// A `VpcEndpointId` no endpoint of this account carries, in the form this service mints
/// (`aos-` and a hexadecimal suffix).
const ABSENT_VPC_ENDPOINT_ID: &str = "aos-0123456789abcdef";

/// An `ApplicationId` this account holds no application under, inside `id`'s own length.
const ABSENT_APPLICATION_ID: &str = "noapplicationhere";

/// An `IndexName` no index of a domain carries.
const ABSENT_INDEX: &str = "catalog-absent";

/// An `ActionID` no scheduled action of a domain carries.
const ABSENT_ACTION_ID: &str = "ssu-absent";

/// A `TargetVersion` inside `VersionString`'s own pattern, so an upgrade of an absent
/// domain is refused for the DOMAIN rather than for the release it names.
const TARGET_VERSION: &str = "OpenSearch_2.19";

/// Domain names inside `DomainName`'s own constraint (`@length(3, 28)` and
/// `^[a-z][a-z0-9\-]+$`) that this account holds no domain under: the shortest the model
/// admits, the longest, and one of ordinary length — so each rule is asserted over the
/// shape of the name as well as over the operation.
///
/// Reached by the sibling suite that asserts WHEN a request addresses nothing
/// ([`super::missing_resource_refusals`]), so the crate holds ONE table of names no domain
/// answers to.
pub(crate) fn absent_domains() -> [&'static str; 3] {
    ["abc", "a-domain-name-at-its-upper-b", "never-created-domain"]
}

/// One row: the operation it drives, and the request that addresses something this
/// account holds nothing for.
struct Row {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// The rows keyed on a DOMAIN this account does not hold, one per operation of this
/// service that resolves one. The method and the URI of each are the model's own
/// (`smithy_query(query="digest")`), so no row names a path shape of its own.
fn domain_rows(name: &str) -> Vec<Row> {
    vec![
        Row {
            operation: "DescribeDomain",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{name}"),
            body: None,
        },
        Row {
            operation: "DescribeDomainConfig",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{name}/config"),
            body: None,
        },
        Row {
            operation: "DescribeDomainAutoTunes",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{name}/autoTunes"),
            body: None,
        },
        Row {
            operation: "DeleteDomain",
            method: "DELETE",
            uri: format!("{PREFIX}/opensearch/domain/{name}"),
            body: None,
        },
        Row {
            operation: "UpdateDomainConfig",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/{name}/config"),
            body: Some(json!({ "ClusterConfig": { "InstanceCount": 2 } })),
        },
        Row {
            operation: "CancelDomainConfigChange",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/{name}/config/cancel"),
            body: Some(json!({})),
        },
        Row {
            operation: "ListScheduledActions",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{name}/scheduledActions"),
            body: None,
        },
        Row {
            operation: "UpdateScheduledAction",
            method: "PUT",
            uri: format!("{PREFIX}/opensearch/domain/{name}/scheduledAction/update"),
            body: Some(json!({
                "ActionID": ABSENT_ACTION_ID,
                "ActionType": "SERVICE_SOFTWARE_UPDATE",
                "ScheduleAt": "NOW",
            })),
        },
        Row {
            operation: "ListDataSources",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{name}/dataSource"),
            body: None,
        },
        Row {
            operation: "GetIndex",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/{name}/index/{ABSENT_INDEX}"),
            body: None,
        },
        Row {
            operation: "DeleteIndex",
            method: "DELETE",
            uri: format!("{PREFIX}/opensearch/domain/{name}/index/{ABSENT_INDEX}"),
            body: None,
        },
        Row {
            operation: "GetUpgradeHistory",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/upgradeDomain/{name}/history"),
            body: None,
        },
        Row {
            operation: "GetUpgradeStatus",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/upgradeDomain/{name}/status"),
            body: None,
        },
        Row {
            operation: "UpgradeDomain",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/upgradeDomain"),
            body: Some(json!({ "DomainName": name, "TargetVersion": TARGET_VERSION })),
        },
        Row {
            operation: "StartServiceSoftwareUpdate",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/serviceSoftwareUpdate/start"),
            body: Some(json!({ "DomainName": name })),
        },
        Row {
            operation: "CancelServiceSoftwareUpdate",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/serviceSoftwareUpdate/cancel"),
            body: Some(json!({ "DomainName": name })),
        },
        Row {
            operation: "RollbackServiceSoftwareUpdate",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/serviceSoftwareUpdate/rollback"),
            body: Some(json!({ "DomainName": name })),
        },
        Row {
            operation: "GetCompatibleVersions",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/compatibleVersions?domainName={name}"),
            body: None,
        },
        Row {
            operation: "InsightFeedback",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/insight-feedback"),
            body: Some(json!({
                "Entity": { "Type": "DomainName", "Value": name },
                "InsightId": guid("000000000000"),
                "Thumbs": "Up",
            })),
        },
    ]
}

/// The rows keyed on a resource that is NOT a domain: a cross-cluster connection, a VPC
/// endpoint, an application, and the account-wide default-application setting nothing is
/// in force for. Each addresses something this account holds nothing for, so the rule is
/// asserted over the KIND of resource as well as over the operation.
fn other_resource_rows() -> Vec<Row> {
    vec![
        Row {
            operation: "AcceptInboundConnection",
            method: "PUT",
            uri: format!(
                "{PREFIX}/opensearch/cc/inboundConnection/{ABSENT_CONNECTION_ID}/accept"
            ),
            body: None,
        },
        Row {
            operation: "RejectInboundConnection",
            method: "PUT",
            uri: format!(
                "{PREFIX}/opensearch/cc/inboundConnection/{ABSENT_CONNECTION_ID}/reject"
            ),
            body: None,
        },
        Row {
            operation: "DeleteInboundConnection",
            method: "DELETE",
            uri: format!("{PREFIX}/opensearch/cc/inboundConnection/{ABSENT_CONNECTION_ID}"),
            body: None,
        },
        Row {
            operation: "DeleteOutboundConnection",
            method: "DELETE",
            uri: format!("{PREFIX}/opensearch/cc/outboundConnection/{ABSENT_CONNECTION_ID}"),
            body: None,
        },
        Row {
            operation: "DeleteVpcEndpoint",
            method: "DELETE",
            uri: format!("{PREFIX}/opensearch/vpcEndpoints/{ABSENT_VPC_ENDPOINT_ID}"),
            body: None,
        },
        Row {
            operation: "GetApplication",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/application/{ABSENT_APPLICATION_ID}"),
            body: None,
        },
        Row {
            operation: "UpdateApplication",
            method: "PUT",
            uri: format!("{PREFIX}/opensearch/application/{ABSENT_APPLICATION_ID}"),
            body: Some(json!({})),
        },
        Row {
            operation: "GetDefaultApplicationSetting",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/defaultApplicationSetting"),
            body: None,
        },
    ]
}

/// Assert `answer` is a missing-resource refusal answered the one way this service
/// answers them: on [`RECORDED_NOT_FOUND_STATUS`], carrying the shape's own error code
/// and a message of its own.
///
/// The crate's ONE statement of that pair: the sibling suite that asserts WHICH resource a
/// refusal is about ([`super::missing_resource_refusals`]) calls this and then asserts the
/// sentence, so no suite pins a status or a code of its own.
pub(crate) fn assert_missing_resource(answer: &Answer, case: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {case}: expected {RECORDED_NOT_FOUND_STATUS} ResourceNotFoundException, got {} \
         with {:?}",
        answer.status,
        answer.body
    );
    let said = message(answer);
    assert!(
        !said.is_empty(),
        "case {case}: expected the refusal to state why, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn every_operation_answers_a_domain_this_account_does_not_hold_on_one_status() {
    let emulator = Emulator::new();
    // A domain this account DOES hold, so what each row below exercises is the name it
    // names rather than an empty store.
    create_baseline(&emulator, "status-held-domain").await;

    for name in absent_domains() {
        for row in domain_rows(name) {
            let answer = emulator.call(row.method, &row.uri, row.body).await;
            assert_missing_resource(
                &answer,
                &format!("{} {} ({name:?})", row.method, row.operation),
            );
        }
    }

    // Every call above was refused, and none of them minted the domain it could not
    // resolve: the reads still find nothing under those names.
    for name in absent_domains() {
        let read = emulator.describe(name).await;
        assert_missing_resource(&read, &format!("DescribeDomain({name:?}, after the refusals)"));
    }
    // And the domain that IS held is still there, so the refusals are about the name.
    let held = emulator.describe("status-held-domain").await;
    assert_eq!(
        held.status, 200,
        "case DescribeDomain(status-held-domain): expected the held domain to still resolve, got \
         {} with {:?}",
        held.status, held.body
    );
}

#[tokio::test]
async fn every_kind_of_resource_this_account_does_not_hold_is_answered_on_one_status() {
    let emulator = Emulator::new();

    for row in other_resource_rows() {
        let answer = emulator.call(row.method, &row.uri, row.body).await;
        assert_missing_resource(&answer, &format!("{} {}", row.method, row.operation));
    }

    // The same rule inside a resource this account DOES hold: an index name no index of
    // a held domain carries is a missing resource too, so what moves the status is the
    // error shape and not which half of the address resolved.
    let held = "status-index-domain";
    create_baseline(&emulator, held).await;
    for (method, operation) in [("GET", "GetIndex"), ("DELETE", "DeleteIndex")] {
        let answer = emulator
            .call(
                method,
                &format!("{PREFIX}/opensearch/domain/{held}/index/{ABSENT_INDEX}"),
                None,
            )
            .await;
        assert_missing_resource(&answer, &format!("{method} {operation} (a held domain)"));
    }
}

#[tokio::test]
async fn a_refusal_carrying_another_modelled_shape_keeps_the_status_the_model_gives_it() {
    let emulator = Emulator::new();
    let held = "status-other-shapes";
    create_baseline(&emulator, held).await;
    let application = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(json!({ "name": "status-other-app" })),
        )
        .await;
    assert_eq!(
        application.status, 200,
        "case CreateApplication(status-other-app): expected 200, got {} with {:?}",
        application.status, application.body
    );

    // (case, the request, the status the model gives the shape it is refused with, that
    // shape). One row per OTHER modelled error shape this service refuses with, so the
    // table spans the shapes the missing-resource status must not have moved: the
    // already-exists, the conflict, the generic envelope and the client-input error.
    let rows: Vec<(&str, Answer, u16, &str)> = vec![
        (
            "CreateDomain for a name this account already holds",
            emulator.create(json!({ "DomainName": held })).await,
            409,
            "ResourceAlreadyExistsException",
        ),
        (
            "CreateApplication for a name this account already uses",
            emulator
                .call(
                    "POST",
                    &format!("{PREFIX}/opensearch/application"),
                    Some(json!({ "name": "status-other-app" })),
                )
                .await,
            409,
            "ConflictException",
        ),
        (
            "DescribeDomainHealth for a domain this account does not hold",
            emulator
                .call(
                    "GET",
                    &format!("{PREFIX}/opensearch/domain/never-created-domain/health"),
                    None,
                )
                .await,
            400,
            "BaseException",
        ),
        (
            "GetDataSource for a name a held domain carries no data source under",
            emulator
                .call(
                    "GET",
                    &format!("{PREFIX}/opensearch/domain/{held}/dataSource/absent-source"),
                    None,
                )
                .await,
            400,
            "ValidationException",
        ),
    ];

    for (case, answer, status, code) in rows {
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (status, code),
            "case {case}: expected {status} {code}, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}
