// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE of every `ResourceNotFoundException` this service answers with.
//!
//! The rule under test: when an operation reports that the resource an identifier names
//! is not there, it builds `message` from the REQUEST as
//! `"<ResourceType> <identifier> cannot be found."` — the resource kind in AWS's
//! CamelCase token, then the identifier the caller submitted verbatim (in whichever
//! modelled form it sent, bare id or ARN), then the fixed trailing sentence. The kind
//! token is NOT the lowercase ARN segment the resource's ARN and the exception's
//! `resourceType` member carry.
//!
//! Each case is a row of a TABLE spanning what the rule generalises over:
//!
//! * the resource KIND — service network, service, service-network/service association
//!   and listener (the last two are kinds the issue did not cite);
//! * the identifier FORM — the bare id and the resource ARN, the two forms every
//!   `*Identifier` member of this service models;
//! * WHY the resource is absent — never created, created and then deleted (the recorded
//!   post-delete read path), or reached through the wrong parent;
//! * the OPERATION that reports it — Get, Delete, Update and the two creates that
//!   resolve an identifier (`CreateListener`,
//!   `CreateServiceNetworkServiceAssociation`).
//!
//! Alongside the message every row re-checks what must hold with it: the 404 status, the
//! wire error code, and the exception's required `resourceId` / `resourceType` members.

use crate::vpclattice_service_slice::{as_label, expected_arn, Answer, Lattice};

/// AWS's resource-kind token for each kind this service's not-found bodies name, written
/// out here rather than read from the service, so no row can pass by agreeing with the
/// code under test.
///
/// Source: recorded vpc-lattice `ResourceNotFoundException` bodies, which spell the kind
/// as its CamelCase resource name — "ServiceNetwork sn-… cannot be found.", "Service
/// svc-… cannot be found." and, for a service-network/service association, "Association
/// snsa-… cannot be found.". No listener not-found body is recorded, so a listener
/// follows the same rule over its Smithy resource name,
/// `com.amazonaws.vpclattice#Listener`.
const LABEL_SERVICE_NETWORK: &str = "ServiceNetwork";
const LABEL_SERVICE: &str = "Service";
const LABEL_ASSOCIATION: &str = "Association";
const LABEL_LISTENER: &str = "Listener";

/// The ARN resource segments of those same kinds, from the modelled ARN patterns
/// (`ServiceNetworkArn`: `…:servicenetwork/sn-…`, `ServiceArn`: `…:service/svc-…`,
/// `ServiceNetworkServiceAssociationArn`: `…:servicenetworkserviceassociation/snsa-…`).
/// Used to build the ARN identifier form of each row — and deliberately NOT the token the
/// message is expected to carry.
const ARN_KIND_SERVICE_NETWORK: &str = "servicenetwork";
const ARN_KIND_SERVICE: &str = "service";
const ARN_KIND_ASSOCIATION: &str = "servicenetworkserviceassociation";

/// The id prefixes of those kinds, from the modelled id patterns (`^sn-[0-9a-z]{17}$`,
/// `^svc-[0-9a-z]{17}$`, `^snsa-[0-9a-z]{17}$`, `^listener-[0-9a-z]{17}$`).
const PREFIX_SERVICE_NETWORK: &str = "sn-";
const PREFIX_SERVICE: &str = "svc-";
const PREFIX_ASSOCIATION: &str = "snsa-";
const PREFIX_LISTENER: &str = "listener-";

/// The construction under test: the kind token, the submitted identifier, and the fixed
/// sentence that closes it.
fn expected_message(label: &str, identifier: &str) -> String {
    format!("{label} {identifier} cannot be found.")
}

/// A well-formed identifier of `prefix`'s kind that nothing ever created: the prefix plus
/// 17 `[0-9a-z]` characters, which is what every modelled id pattern of this service
/// demands (so the request reaches the handler instead of being refused at the boundary).
/// Shared with the suites beside this file, so the construction has one spelling.
pub(crate) fn absent_id(prefix: &str, fill: &str) -> String {
    let suffix: String = fill.chars().cycle().take(17).collect();
    format!("{prefix}{suffix}")
}

/// Assert one row: a 404 `ResourceNotFoundException` whose message is AWS's construction
/// over `label` and the identifier the request submitted. Shared with the suites beside
/// this file, so the assertion has one spelling.
pub(crate) fn assert_not_found(answer: &Answer, case: &str, label: &str, identifier: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected 404, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
        answer.error_code,
        answer.body
    );
    let message = answer.body["message"].as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: the exception must carry the required message member, got {}",
            answer.body
        )
    });
    assert_eq!(
        message,
        expected_message(label, identifier),
        "case {case:?}: expected the AWS not-found construction {:?}, got {message:?}",
        expected_message(label, identifier)
    );
    // The required structured members are still there, and `resourceId` names the thing
    // the request named: either the submitted identifier or the id it resolved to, both
    // of which the submitted identifier ends with.
    let resource_id = answer.body["resourceId"].as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: the exception must carry the required resourceId member, got {}",
            answer.body
        )
    });
    assert!(
        !resource_id.is_empty() && identifier.ends_with(resource_id),
        "case {case:?}: resourceId must name the requested resource {identifier:?}, \
         got {resource_id:?}"
    );
    assert!(
        answer.body["resourceType"]
            .as_str()
            .is_some_and(|kind| !kind.is_empty()),
        "case {case:?}: the exception must carry the required resourceType member, got {}",
        answer.body
    );
}

/// The two modelled identifier forms of one resource: its bare id and its ARN.
fn identifier_forms(arn_kind: &str, id: &str) -> [(&'static str, String); 2] {
    [
        ("by id", id.to_string()),
        ("by arn", expected_arn(arn_kind, id)),
    ]
}

/// A fill pattern per row, so no row can pass on a literal shared with another.
const FILLS: [&str; 3] = ["0", "z", "a1b2c3d4"];

/// Service networks: never-created and deleted identifiers, both forms, over every
/// operation that resolves a `serviceNetworkIdentifier`.
#[tokio::test]
async fn an_absent_service_network_reports_the_aws_construction() {
    let lattice = Lattice::new();

    let mut rows: Vec<(String, String)> = Vec::new();
    for fill in FILLS {
        let id = absent_id(PREFIX_SERVICE_NETWORK, fill);
        for (form, identifier) in identifier_forms(ARN_KIND_SERVICE_NETWORK, &id) {
            rows.push((format!("never created ({fill}) {form}"), identifier));
        }
    }

    // The recorded path: a service network that existed and was deleted. Read back by
    // both identifier forms.
    let created = lattice
        .create_service_network(serde_json::json!({ "name": "gone-net" }))
        .await;
    assert_eq!(
        created.status, 201,
        "case fixture: creating the service network expected 201, got {} / {}",
        created.status, created.body
    );
    let gone_id = created.str_field("id");
    let gone_arn = created.str_field("arn");
    let deleted = lattice
        .call("DELETE", &format!("/servicenetworks/{gone_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "case fixture: deleting the service network expected 204, got {} / {}",
        deleted.status, deleted.body
    );
    rows.push(("deleted by id".to_string(), gone_id));
    rows.push(("deleted by arn".to_string(), gone_arn));

    for (why, identifier) in rows {
        let label = as_label(&identifier);
        for (operation, method, body) in [
            ("GetServiceNetwork", "GET", None),
            ("DeleteServiceNetwork", "DELETE", None),
            (
                "UpdateServiceNetwork",
                "PATCH",
                Some(serde_json::json!({ "authType": "NONE" })),
            ),
        ] {
            let case = format!("{operation}: {why}");
            let answer = lattice
                .call(method, &format!("/servicenetworks/{label}"), body)
                .await;
            assert_not_found(&answer, &case, LABEL_SERVICE_NETWORK, &identifier);
        }
    }
}

/// Services: the same table over every operation that resolves a `serviceIdentifier`,
/// including the two that reach a service to hang something off it.
#[tokio::test]
async fn an_absent_service_reports_the_aws_construction() {
    let lattice = Lattice::new();

    let mut rows: Vec<(String, String)> = Vec::new();
    for fill in FILLS {
        let id = absent_id(PREFIX_SERVICE, fill);
        for (form, identifier) in identifier_forms(ARN_KIND_SERVICE, &id) {
            rows.push((format!("never created ({fill}) {form}"), identifier));
        }
    }

    let created = lattice
        .create_service(serde_json::json!({ "name": "gone-svc" }))
        .await;
    assert_eq!(
        created.status, 201,
        "case fixture: creating the service expected 201, got {} / {}",
        created.status, created.body
    );
    let gone_id = created.str_field("id");
    let gone_arn = created.str_field("arn");
    let deleted = lattice
        .call("DELETE", &format!("/services/{gone_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case fixture: deleting the service expected 200, got {} / {}",
        deleted.status, deleted.body
    );
    rows.push(("deleted by id".to_string(), gone_id));
    rows.push(("deleted by arn".to_string(), gone_arn));

    for (why, identifier) in rows {
        let label = as_label(&identifier);
        // GET and DELETE take the identifier in the path; CreateListener resolves the
        // same identifier before it mints anything, and GetListener resolves it before
        // it looks at the listener.
        let answers = [
            (
                "GetService",
                lattice.call("GET", &format!("/services/{label}"), None).await,
            ),
            (
                "GetListener",
                lattice
                    .call(
                        "GET",
                        &format!(
                            "/services/{label}/listeners/{}",
                            absent_id(PREFIX_LISTENER, "0")
                        ),
                        None,
                    )
                    .await,
            ),
            (
                "CreateListener",
                lattice
                    .call(
                        "POST",
                        &format!("/services/{label}/listeners"),
                        Some(serde_json::json!({
                            "name": "orphan",
                            "protocol": "HTTP",
                            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
                        })),
                    )
                    .await,
            ),
            (
                "DeleteService",
                lattice
                    .call("DELETE", &format!("/services/{label}"), None)
                    .await,
            ),
        ];
        for (operation, answer) in &answers {
            let case = format!("{operation}: {why}");
            assert_not_found(answer, &case, LABEL_SERVICE, &identifier);
        }
    }
}

/// Associations: the kind the issue did not cite, and the one whose AWS token is neither
/// the ARN segment nor the full shape name but "Association".
#[tokio::test]
async fn an_absent_association_reports_the_aws_construction() {
    let lattice = Lattice::new();

    let mut rows: Vec<(String, String)> = Vec::new();
    for fill in FILLS {
        let id = absent_id(PREFIX_ASSOCIATION, fill);
        for (form, identifier) in identifier_forms(ARN_KIND_ASSOCIATION, &id) {
            rows.push((format!("never created ({fill}) {form}"), identifier));
        }
    }

    // A real association, deleted: the post-delete read path for this kind.
    let network = lattice
        .create_service_network(serde_json::json!({ "name": "assoc-net" }))
        .await;
    assert_eq!(
        network.status, 201,
        "case fixture: creating the service network expected 201, got {} / {}",
        network.status, network.body
    );
    let service = lattice
        .create_service(serde_json::json!({ "name": "assoc-svc" }))
        .await;
    assert_eq!(
        service.status, 201,
        "case fixture: creating the service expected 201, got {} / {}",
        service.status, service.body
    );
    let association = lattice
        .associate(&service.str_field("id"), &network.str_field("id"))
        .await;
    assert_eq!(
        association.status, 200,
        "case fixture: associating expected 200, got {} / {}",
        association.status, association.body
    );
    let gone_id = association.str_field("id");
    let gone_arn = association.str_field("arn");
    let removed = lattice
        .call(
            "DELETE",
            &format!("/servicenetworkserviceassociations/{gone_id}"),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case fixture: deleting the association expected 200, got {} / {}",
        removed.status, removed.body
    );
    rows.push(("deleted by id".to_string(), gone_id));
    rows.push(("deleted by arn".to_string(), gone_arn));

    for (why, identifier) in rows {
        let label = as_label(&identifier);
        for (operation, method) in [
            ("GetServiceNetworkServiceAssociation", "GET"),
            ("DeleteServiceNetworkServiceAssociation", "DELETE"),
        ] {
            let case = format!("{operation}: {why}");
            let answer = lattice
                .call(
                    method,
                    &format!("/servicenetworkserviceassociations/{label}"),
                    None,
                )
                .await;
            assert_not_found(&answer, &case, LABEL_ASSOCIATION, &identifier);
        }
    }
}

/// Listeners: a listener identifier that resolves to nothing under a service that DOES
/// exist, including a real listener reached through the wrong service. The kind token
/// here is the listener's, not its service's.
#[tokio::test]
async fn an_absent_listener_reports_the_aws_construction() {
    let lattice = Lattice::new();

    let owner = lattice
        .create_service(serde_json::json!({ "name": "listener-owner" }))
        .await;
    assert_eq!(
        owner.status, 201,
        "case fixture: creating the owning service expected 201, got {} / {}",
        owner.status, owner.body
    );
    let owner_id = owner.str_field("id");
    let other = lattice
        .create_service(serde_json::json!({ "name": "listener-other" }))
        .await;
    assert_eq!(
        other.status, 201,
        "case fixture: creating the other service expected 201, got {} / {}",
        other.status, other.body
    );
    let other_id = other.str_field("id");

    let listener = lattice
        .call(
            "POST",
            &format!("/services/{owner_id}/listeners"),
            Some(serde_json::json!({
                "name": "only-listener",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            })),
        )
        .await;
    assert_eq!(
        listener.status, 201,
        "case fixture: creating the listener expected 201, got {} / {}",
        listener.status, listener.body
    );
    let listener_id = listener.str_field("id");
    let listener_arn = listener.str_field("arn");

    // (why, the service the read goes through, the listener identifier it names)
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for fill in FILLS {
        let absent = absent_id(PREFIX_LISTENER, fill);
        rows.push((
            format!("never created ({fill}) by id"),
            owner_id.clone(),
            absent.clone(),
        ));
        // A listener ARN nests under its service's: `…:service/svc-…/listener/listener-…`
        // (`ListenerArn`'s modelled pattern).
        rows.push((
            format!("never created ({fill}) by arn"),
            owner_id.clone(),
            expected_arn(&format!("service/{owner_id}/listener"), &absent),
        ));
    }
    rows.push((
        "a real listener through the wrong service, by id".to_string(),
        other_id.clone(),
        listener_id,
    ));
    rows.push((
        "a real listener through the wrong service, by arn".to_string(),
        other_id,
        listener_arn,
    ));

    for (why, service_identifier, identifier) in rows {
        let case = format!("GetListener: {why}");
        let answer = lattice
            .call(
                "GET",
                &format!(
                    "/services/{service_identifier}/listeners/{}",
                    as_label(&identifier)
                ),
                None,
            )
            .await;
        assert_not_found(&answer, &case, LABEL_LISTENER, &identifier);
    }
}

/// `CreateServiceNetworkServiceAssociation` takes both identifiers in the BODY, so this
/// row set also shows the construction is not an artefact of an `@httpLabel`: whichever
/// of the two identifiers names nothing decides the kind token the message carries.
#[tokio::test]
async fn an_association_create_names_whichever_identifier_is_absent() {
    let lattice = Lattice::new();

    let network = lattice
        .create_service_network(serde_json::json!({ "name": "live-net" }))
        .await;
    assert_eq!(
        network.status, 201,
        "case fixture: creating the service network expected 201, got {} / {}",
        network.status, network.body
    );
    let live_network = network.str_field("id");
    let live_network_arn = network.str_field("arn");
    let service = lattice
        .create_service(serde_json::json!({ "name": "live-svc" }))
        .await;
    assert_eq!(
        service.status, 201,
        "case fixture: creating the service expected 201, got {} / {}",
        service.status, service.body
    );
    let live_service = service.str_field("id");
    let live_service_arn = service.str_field("arn");

    // (why, serviceIdentifier, serviceNetworkIdentifier, the kind token and the
    // identifier the answer must name). The service is resolved first, so a row with
    // both absent names the service.
    let absent_service = absent_id(PREFIX_SERVICE, "0");
    let absent_service_arn = expected_arn(ARN_KIND_SERVICE, &absent_id(PREFIX_SERVICE, "z"));
    let absent_network = absent_id(PREFIX_SERVICE_NETWORK, "0");
    let absent_network_arn = expected_arn(
        ARN_KIND_SERVICE_NETWORK,
        &absent_id(PREFIX_SERVICE_NETWORK, "z"),
    );
    let rows: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "absent service by id",
            &absent_service,
            &live_network,
            LABEL_SERVICE,
            &absent_service,
        ),
        (
            "absent service by arn",
            &absent_service_arn,
            &live_network_arn,
            LABEL_SERVICE,
            &absent_service_arn,
        ),
        (
            "absent service network by id",
            &live_service,
            &absent_network,
            LABEL_SERVICE_NETWORK,
            &absent_network,
        ),
        (
            "absent service network by arn",
            &live_service_arn,
            &absent_network_arn,
            LABEL_SERVICE_NETWORK,
            &absent_network_arn,
        ),
        (
            "both absent names the service",
            &absent_service,
            &absent_network,
            LABEL_SERVICE,
            &absent_service,
        ),
    ];

    for (why, service_identifier, network_identifier, label, identifier) in rows {
        let case = format!("CreateServiceNetworkServiceAssociation: {why}");
        let answer = lattice.associate(service_identifier, network_identifier).await;
        assert_not_found(&answer, &case, label, identifier);
    }
}
