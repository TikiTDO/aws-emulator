// SPDX-License-Identifier: CC0-1.0

//! IAM's decision in front of the Event-API (`/v2/apis…`) and domain-association
//! (`/v1/domainnames/{domainName}/apiassociation`) surface, end to end over HTTP.
//!
//! The rule under test: the identity-based decision is taken by the layer in FRONT of this
//! service, so every action it publishes is judged — whether or not the operation behind it
//! lists `AccessDeniedException` among its own modelled errors. A caller whose policies
//! allow nothing, and a caller whose policies allow the action on ANOTHER resource, are
//! refused with that modelled refusal (HTTP 403) and IAM's implicit-deny sentence about
//! the action and the resource the REQUEST addresses, before the operation reads or writes
//! anything; a caller the policy allows reaches the operation and gets its own answer.
//!
//! The table spans what the rule generalises over: the operation (a collection read, a
//! create, a read of one resource, an update, a delete, and the three calls on a domain's
//! API association), the method (GET, POST, DELETE), the kind of resource the request
//! addresses (the any-resource wildcard of a call that names none, an API's ARN, a custom
//! domain's ARN) and whether the operation's own error set declares the refusal — the
//! collection read and the two creates do not, and are judged all the same.
//!
//! The principals are made with IAM's own operations over the same engine, so the
//! documents this service reads are the ones IAM wrote; the harness is the
//! identity-authorization suite's.

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::service::authorization::{
    ACTION_PREFIX, ANY_RESOURCE,
};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;

use crate::identity_authorization::{
    allow, arn, default_deny_message, Emulator, ACCESS_DENIED, ACCESS_DENIED_STATUS, ACCOUNT_KEY,
};

/// The name the Event API the table addresses is created under, which a refused `UpdateApi`
/// must leave in place.
const API_NAME: &str = "Events_gate";

/// The name a served `UpdateApi` renames it to.
const RENAMED: &str = "Events_gate_renamed";

/// An API id of the modelled form (26 characters) that nothing created: the resource a
/// policy is written about to show that the RESOURCE the request addresses is what the
/// action is judged against.
const ABSENT_API: &str = "zzzzzzzzzzzzzzzzzzzzzzzzzz";

/// One Event API configuration whose only authorization provider is an API key.
fn event_config() -> Value {
    let modes = json!([{"authType": "API_KEY"}]);
    json!({
        "authProviders": [{"authType": "API_KEY"}],
        "connectionAuthModes": modes,
        "defaultPublishAuthModes": modes,
        "defaultSubscribeAuthModes": modes,
    })
}

/// One request the gate decides about: the call itself, the action and the resource IAM
/// judges it by, and the status the OPERATION answers a caller the policy allows — which is
/// how a served call is told from a refused one.
struct Row {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    resource: String,
    served_status: u16,
}

/// The table, in an order a served pass can be replayed in: the calls that remove what the
/// later rows address come last.
fn rows(api_id: &str, held_domain: &str, absent_domain: &str) -> Vec<Row> {
    vec![
        Row {
            operation: "ListApis",
            method: "GET",
            path: "/v2/apis".to_string(),
            body: None,
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
        Row {
            operation: "CreateApi",
            method: "POST",
            path: "/v2/apis".to_string(),
            body: Some(json!({"name": "Events_gate_created", "eventConfig": event_config()})),
            resource: arn(ANY_RESOURCE),
            served_status: 200,
        },
        Row {
            operation: "GetApi",
            method: "GET",
            path: format!("/v2/apis/{api_id}"),
            body: None,
            resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
        Row {
            operation: "ListChannelNamespaces",
            method: "GET",
            path: format!("/v2/apis/{api_id}/channelNamespaces"),
            body: None,
            resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
        Row {
            operation: "CreateChannelNamespace",
            method: "POST",
            path: format!("/v2/apis/{api_id}/channelNamespaces"),
            body: Some(json!({"name": "gated"})),
            resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
        Row {
            operation: "GetApiAssociation",
            method: "GET",
            path: format!("/v1/domainnames/{absent_domain}/apiassociation"),
            body: None,
            resource: arn(&state::domain_name_resource(absent_domain)),
            // The gate fires BEFORE the lookup, so an authorized caller is the only one
            // that learns the domain was never created.
            served_status: 404,
        },
        Row {
            operation: "DisassociateApi",
            method: "DELETE",
            path: format!("/v1/domainnames/{absent_domain}/apiassociation"),
            body: None,
            resource: arn(&state::domain_name_resource(absent_domain)),
            served_status: 404,
        },
        Row {
            operation: "AssociateApi",
            method: "POST",
            path: format!("/v1/domainnames/{held_domain}/apiassociation"),
            body: Some(json!({"apiId": api_id})),
            resource: arn(&state::domain_name_resource(held_domain)),
            served_status: 200,
        },
        Row {
            operation: "UpdateApi",
            method: "POST",
            path: format!("/v2/apis/{api_id}"),
            body: Some(json!({"name": RENAMED, "eventConfig": event_config()})),
            resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
        Row {
            operation: "DeleteApi",
            method: "DELETE",
            path: format!("/v2/apis/{api_id}"),
            body: None,
            resource: arn(&state::api_resource(api_id)),
            served_status: 200,
        },
    ]
}

/// RULE: every action this service publishes is judged against the caller's identity-based
/// policies and the resource the request addresses, before the operation runs.
#[tokio::test]
async fn every_published_action_is_judged_before_it_runs() {
    let emulator = Emulator::new();
    let held_domain = "held-gate.example.com";
    let absent_domain = "absent-gate.example.com";

    // The fixture is made with the account's own credential, which holds no IAM policies.
    let created = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v2/apis",
            Some(&json!({"name": API_NAME, "eventConfig": event_config()})),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"fixture\": expected CreateApi 200, got {} {:?}",
        created.status, created.body
    );
    let api_id = created.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let held = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            "/v1/domainnames",
            Some(&json!({
                "domainName": held_domain,
                "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/gate-0001",
            })),
        )
        .await;
    assert_eq!(
        held.status, 200,
        "case \"fixture\": expected CreateDomainName 200, got {} {:?}",
        held.status, held.body
    );

    let table = rows(&api_id, held_domain, absent_domain);
    let nothing_allowed = emulator.user("gate_denied", None).await;

    // A caller whose policies allow nothing is refused every one of them.
    for row in &table {
        let reply = emulator
            .call(
                &nothing_allowed.access_key_id,
                row.method,
                &row.path,
                row.body.as_ref(),
            )
            .await;
        let case = row.operation;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED),
            "case {case:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED} for a caller \
             whose policies allow nothing, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.message(),
            default_deny_message(&nothing_allowed.arn, row.operation, &row.resource),
            "case {case:?}: expected IAM's implicit-deny sentence about {} on {}, got {:?}",
            row.operation,
            row.resource,
            reply.message()
        );
    }

    // Every refusal was taken before the operation ran, so nothing of what the table's
    // mutations would have done happened: the API keeps its name, holds no namespace, the
    // domain carries no association and no second API was minted.
    let read = emulator
        .call(ACCOUNT_KEY, "GET", &format!("/v2/apis/{api_id}"), None)
        .await;
    assert_eq!(
        (read.status, read.body["api"]["name"].as_str()),
        (200, Some(API_NAME)),
        "case \"after the refusals\": expected the API untouched, got {} {:?}",
        read.status,
        read.body
    );
    let namespaces = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/v2/apis/{api_id}/channelNamespaces"),
            None,
        )
        .await;
    assert_eq!(
        namespaces.body["channelNamespaces"],
        json!([]),
        "case \"after the refusals\": expected no namespace created, got {:?}",
        namespaces.body
    );
    let association = emulator
        .call(
            ACCOUNT_KEY,
            "GET",
            &format!("/v1/domainnames/{held_domain}/apiassociation"),
            None,
        )
        .await;
    assert_eq!(
        association.status, 404,
        "case \"after the refusals\": expected the domain to carry no association, got {} \
         {:?}",
        association.status,
        association.body
    );
    let listed = emulator.call(ACCOUNT_KEY, "GET", "/v2/apis", None).await;
    assert_eq!(
        listed.body["apis"].as_array().map(Vec::len),
        Some(1),
        "case \"after the refusals\": expected the refused create to have minted nothing, \
         got {:?}",
        listed.body
    );

    // A policy that allows the action on ANOTHER resource is the same default deny: the
    // decision is about the resource the request addresses.
    let elsewhere = arn(&state::api_resource(ABSENT_API));
    for (index, row) in table.iter().enumerate() {
        let principal = emulator
            .user(
                &format!("gate_elsewhere_{index}"),
                Some(&allow(
                    &format!("{ACTION_PREFIX}:{}", row.operation),
                    &elsewhere,
                )),
            )
            .await;
        let reply = emulator
            .call(
                &principal.access_key_id,
                row.method,
                &row.path,
                row.body.as_ref(),
            )
            .await;
        let case = row.operation;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED),
            "case {case:?}: expected a grant on another resource to refuse, got {} {:?} \
             with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // The same calls on a policy that allows the action on the resource addressed reach the
    // operation, which answers them itself.
    for (index, row) in table.iter().enumerate() {
        let principal = emulator
            .user(
                &format!("gate_allowed_{index}"),
                Some(&allow(
                    &format!("{ACTION_PREFIX}:{}", row.operation),
                    &row.resource,
                )),
            )
            .await;
        let reply = emulator
            .call(
                &principal.access_key_id,
                row.method,
                &row.path,
                row.body.as_ref(),
            )
            .await;
        let case = row.operation;
        assert_eq!(
            reply.status, row.served_status,
            "case {case:?}: expected the operation's own {} answer for a caller the policy \
             allows, got {} {:?} with {:?}",
            row.served_status, reply.status, reply.error_code, reply.body
        );
        assert_ne!(
            reply.error_code, ACCESS_DENIED,
            "case {case:?}: expected the call to reach the operation, got {:?}",
            reply.body
        );
        // A served MUTATION reached the store: the rename the update carried is what the
        // API reports afterwards, so the gate let the write through rather than answering
        // for it. Read while the API is still there, since a later row deletes it.
        if row.operation == "UpdateApi" {
            let renamed = emulator
                .call(ACCOUNT_KEY, "GET", &format!("/v2/apis/{api_id}"), None)
                .await;
            assert_eq!(
                (renamed.status, renamed.body["api"]["name"].as_str()),
                (200, Some(RENAMED)),
                "case {case:?}: expected the served update to be what the API reports, got \
                 {} {:?}",
                renamed.status,
                renamed.body
            );
        }
    }

    // The served `DeleteApi` removed the API it addressed, leaving only the one the served
    // create minted.
    let gone = emulator
        .call(ACCOUNT_KEY, "GET", &format!("/v2/apis/{api_id}"), None)
        .await;
    assert_eq!(
        gone.status, 404,
        "case \"after the served pass\": expected the deleted API to be absent, got {} {:?}",
        gone.status,
        gone.body
    );
    let remaining = emulator.call(ACCOUNT_KEY, "GET", "/v2/apis", None).await;
    let names: Vec<&str> = remaining.body["apis"]
        .as_array()
        .map(|apis| {
            apis.iter()
                .filter_map(|api| api["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["Events_gate_created"],
        "case \"after the served pass\": expected only the API the served create minted, \
         got {:?}",
        remaining.body
    );
}
