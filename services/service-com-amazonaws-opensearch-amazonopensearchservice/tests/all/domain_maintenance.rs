// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service node maintenance surface:
//! `StartDomainMaintenance` and `GetDomainMaintenanceStatus`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one
//! call:
//!
//! * maintenance runs on ONE data node of the domain, named by `NodeId`: the id has
//!   to be one the node census publishes for a data node, and the data-node role has
//!   to be able to lose one of its nodes — so the eligible rows span the published
//!   node types (a data node, a master node, a warm node), the node counts on both
//!   sides of the smallest role a node can be taken out of, an id no census
//!   publishes, another domain's node and a request naming no node at all;
//! * an accepted action is a RECORD: the id the call hands out addresses it, it
//!   reports the action and node the request named, and the clock — not a reader —
//!   walks it from `PENDING` through `IN_PROGRESS` to the terminal status the service
//!   reports a finished action with, over every `MaintenanceType` value the model
//!   publishes;
//! * the listing reports those same records, each once and newest first, narrowed by
//!   `Action` and by `Status` — over every published value of each — and paged by
//!   every page size from one record per page up to more than the domain holds.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    MAINTAINABLE_DATA_NODES, MAINTENANCE_STATUS_IN_PROGRESS, MAINTENANCE_STATUS_PENDING,
    MAINTENANCE_STATUS_SUCCESS, NODE_TYPE_DATA, NODE_TYPE_MASTER, NODE_TYPE_ULTRAWARM,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The `MaintenanceType` values the model publishes (`smithy_query(query="shape",
/// shape="MaintenanceType")`), every one of which names a process on a node.
pub(crate) const MAINTENANCE_TYPES: [&str; 3] =
    ["REBOOT_NODE", "RESTART_SEARCH_PROCESS", "RESTART_DASHBOARD"];

/// A `NodeId` no census publishes, inside `NodeId`'s `@length(min: 10, max: 40)`.
const ABSENT_NODE_ID: &str = "0123456789abcdef0123456789abcdef";

/// The `MaintenanceStatus` value the MODEL carries for a finished action
/// (`smithy_query(query="shape", shape="MaintenanceStatus")` lists `COMPLETED`,
/// `FAILED`, `IN_PROGRESS`, `PENDING` and `TIMED_OUT`), which the service's own
/// terminal status is not: a filter on it therefore selects no action at all.
const MODELLED_FINISHED_STATUS: &str = "COMPLETED";

/// `POST /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance`.
pub(crate) async fn start(
    emulator: &Emulator,
    name: &str,
    action: &str,
    node_id: Option<&str>,
) -> Answer {
    let mut body = json!({ "Action": action });
    if let Some(node_id) = node_id {
        body["NodeId"] = json!(node_id);
    }
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{name}/domainMaintenance"),
            Some(body),
        )
        .await
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance?maintenanceId=`.
pub(crate) async fn status(emulator: &Emulator, name: &str, maintenance_id: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "{PREFIX}/opensearch/domain/{name}/domainMaintenance?maintenanceId={maintenance_id}"
            ),
            None,
        )
        .await
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/domainMaintenances{query}`.
pub(crate) async fn list(emulator: &Emulator, name: &str, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/domainMaintenances{query}"),
            None,
        )
        .await
}

/// `CreateDomain` with `cluster` as its `ClusterConfig`, asserting it was accepted,
/// and then waiting out the provisioning span.
///
/// Every case below reads or acts on a NODE of the domain, and a domain with work in
/// flight publishes no node census — so each row waits for the create to land before
/// the call under test, rather than asserting on a domain that is still coming up.
pub(crate) async fn create(emulator: &Emulator, name: &str, cluster: Value) {
    let answer = emulator
        .create(json!({ "DomainName": name, "ClusterConfig": cluster }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
}

/// The nodes of `name`, as `DescribeDomainNodes` publishes them: (NodeId, NodeType)
/// per node.
pub(crate) async fn census(emulator: &Emulator, name: &str) -> Vec<(String, String)> {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/nodes"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainNodes({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainNodesStatusList"]
        .as_array()
        .unwrap_or_else(|| panic!("case DescribeDomainNodes({name:?}): expected a node list"))
        .iter()
        .map(|node| {
            let case = format!("DescribeDomainNodes({name:?})");
            (
                string_member(node, "NodeId", &case),
                string_member(node, "NodeType", &case),
            )
        })
        .collect()
}

/// The first node of `node_type` the census of `name` publishes.
async fn node_of(emulator: &Emulator, name: &str, node_type: &str) -> String {
    census(emulator, name)
        .await
        .into_iter()
        .find(|(_, published)| published == node_type)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("the census of {name} publishes a {node_type} node"))
}

/// A cluster configuration with `data_nodes` data nodes, dedicated masters and warm
/// nodes turned on — so the census publishes a node of every `NodeType` this service
/// reports.
pub(crate) fn every_role(data_nodes: i32) -> Value {
    json!({
        "InstanceType": "r6g.large.search",
        "InstanceCount": data_nodes,
        "DedicatedMasterEnabled": true,
        "DedicatedMasterType": "m5.large.search",
        "DedicatedMasterCount": 3,
        "WarmEnabled": true,
        "WarmType": "ultrawarm1.medium.search",
        "WarmCount": 3,
    })
}

#[tokio::test]
async fn maintenance_is_started_only_on_an_eligible_data_node() {
    let emulator = Emulator::new();
    // The published minimum a data node can be taken out of, and one domain per
    // count below it.
    let maintainable = MAINTAINABLE_DATA_NODES;
    create(&emulator, "big-domain", every_role(maintainable)).await;
    create(&emulator, "bigger-domain", every_role(maintainable + 4)).await;
    create(&emulator, "small-domain", every_role(1)).await;
    create(&emulator, "two-node-domain", every_role(maintainable - 1)).await;

    // ACCEPTED: a data node of a role that can lose one, at the bound and above it.
    for name in ["big-domain", "bigger-domain"] {
        let node_id = node_of(&emulator, name, NODE_TYPE_DATA).await;
        let answer = start(
            &emulator,
            name,
            "RESTART_SEARCH_PROCESS",
            Some(node_id.as_str()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case StartDomainMaintenance({name:?}, data node): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let maintenance_id = string_member(
            &answer.body,
            "MaintenanceId",
            &format!("StartDomainMaintenance({name:?})"),
        );
        assert!(
            (1..=100).contains(&maintenance_id.len()),
            "case StartDomainMaintenance({name:?}): a RequestId is 1..=100 characters, got \
             {maintenance_id:?}"
        );
    }

    // REFUSED: every other node a caller could name. One row per way a NodeId fails
    // to name a data node this service can maintain.
    let master_node = node_of(&emulator, "big-domain", NODE_TYPE_MASTER).await;
    let warm_node = node_of(&emulator, "big-domain", NODE_TYPE_ULTRAWARM).await;
    let small_data_node = node_of(&emulator, "small-domain", NODE_TYPE_DATA).await;
    let two_node_data_node = node_of(&emulator, "two-node-domain", NODE_TYPE_DATA).await;
    let other_domains_node = node_of(&emulator, "bigger-domain", NODE_TYPE_DATA).await;
    for (name, node_id, case) in [
        ("big-domain", master_node.as_str(), "a dedicated master node"),
        ("big-domain", warm_node.as_str(), "a warm node"),
        ("big-domain", ABSENT_NODE_ID, "an id no census publishes"),
        (
            "big-domain",
            other_domains_node.as_str(),
            "another domain's data node",
        ),
        (
            "small-domain",
            small_data_node.as_str(),
            "the only data node of its role",
        ),
        (
            "two-node-domain",
            two_node_data_node.as_str(),
            "a data node of a role one short of the bound",
        ),
    ] {
        let answer = start(&emulator, name, "RESTART_SEARCH_PROCESS", Some(node_id)).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected the missing resource's HTTP {RECORDED_NOT_FOUND_STATUS}, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some("The requested node ID is invalid"),
            "case {case}: expected the service's wording for an ineligible node, got {:?}",
            answer.body
        );
    }

    // A request naming no node names nothing to act on, which is a malformed request
    // rather than a node that could not be found.
    let answer = start(&emulator, "big-domain", "RESTART_SEARCH_PROCESS", None).await;
    assert_eq!(
        answer.status, 400,
        "case no NodeId: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case no NodeId: expected ValidationException, got {:?}",
        answer.body
    );

    // A domain that was never created is the modelled not-found naming the domain.
    let answer = start(
        &emulator,
        "absent-domain",
        "REBOOT_NODE",
        Some(ABSENT_NODE_ID),
    )
    .await;
    assert_eq!(
        error_code(&answer),
        "ResourceNotFoundException",
        "case an unknown domain: expected ResourceNotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some("Domain not found: absent-domain"),
        "case an unknown domain: expected the miss to name the domain, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_accepted_action_is_read_back_and_walked_by_the_clock() {
    let emulator = Emulator::new();
    create(&emulator, "maintained-domain", every_role(MAINTAINABLE_DATA_NODES)).await;
    let nodes: Vec<String> = census(&emulator, "maintained-domain")
        .await
        .into_iter()
        .filter(|(_, node_type)| node_type == NODE_TYPE_DATA)
        .map(|(id, _)| id)
        .collect();

    // One action per published `MaintenanceType`, each on a different data node of
    // the domain, so the record reports what its own request named.
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut started: Vec<(String, String, String)> = Vec::new();
    for (index, action) in MAINTENANCE_TYPES.iter().enumerate() {
        let node_id = nodes[index % nodes.len()].clone();
        let answer = start(
            &emulator,
            "maintained-domain",
            action,
            Some(node_id.as_str()),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case StartDomainMaintenance({action}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let maintenance_id =
            string_member(&answer.body, "MaintenanceId", &format!("start {action}"));
        assert!(
            ids.insert(maintenance_id.clone()),
            "case StartDomainMaintenance({action}): every action has its own id, {maintenance_id} \
             repeated"
        );
        started.push((maintenance_id, (*action).to_string(), node_id));
    }

    // Straight after the call the action is still waiting: the span it takes is not
    // zero, so a caller observes the transient status.
    for (maintenance_id, action, node_id) in &started {
        let answer = status(&emulator, "maintained-domain", maintenance_id).await;
        let case = format!("GetDomainMaintenanceStatus({action})");
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            (
                answer.body["Status"].as_str(),
                answer.body["Action"].as_str(),
                answer.body["NodeId"].as_str(),
            ),
            (
                Some(MAINTENANCE_STATUS_PENDING),
                Some(action.as_str()),
                Some(node_id.as_str()),
            ),
            "case {case}: expected the action and node the request named, still pending, got {:?}",
            answer.body
        );
        assert!(
            answer.body["CreatedAt"].is_number() && answer.body["UpdatedAt"].is_number(),
            "case {case}: expected CreatedAt/UpdatedAt timestamps, got {:?}",
            answer.body
        );

        // A read does not move the record: the second answer is the first.
        let again = status(&emulator, "maintained-domain", maintenance_id).await;
        assert_eq!(
            again.body, answer.body,
            "case {case}: a read must not change the action: {:?} then {:?}",
            answer.body, again.body
        );
    }

    // The clock, not the reader, walks the action: `PENDING` then `IN_PROGRESS` then
    // the terminal status, each step dated.
    let (maintenance_id, action, _) = &started[0];
    let created = status(&emulator, "maintained-domain", maintenance_id)
        .await
        .body["CreatedAt"]
        .clone();
    let mut seen: Vec<String> = vec![MAINTENANCE_STATUS_PENDING.to_string()];
    for _ in 0..2 {
        tokio::time::sleep(SETTLE_WAIT).await;
        let answer = status(&emulator, "maintained-domain", maintenance_id).await;
        let case = format!("GetDomainMaintenanceStatus({action}, settled)");
        let reported = string_member(&answer.body, "Status", &case);
        if seen.last() != Some(&reported) {
            seen.push(reported);
        }
        assert_eq!(
            answer.body["CreatedAt"], created,
            "case {case}: the instant the action was created does not move, got {:?} after {:?}",
            answer.body["CreatedAt"], created
        );
    }
    assert_eq!(
        seen.last().map(String::as_str),
        Some(MAINTENANCE_STATUS_SUCCESS),
        "the action finishes once its spans have passed, got {seen:?}"
    );
    assert!(
        seen.iter().all(|status| [
            MAINTENANCE_STATUS_PENDING,
            MAINTENANCE_STATUS_IN_PROGRESS,
            MAINTENANCE_STATUS_SUCCESS
        ]
        .contains(&status.as_str())),
        "every status is one this service reports, got {seen:?}"
    );

    // An id this domain holds no action for, and an id another domain's action
    // carries, both name no resource.
    create(&emulator, "other-domain", every_role(MAINTAINABLE_DATA_NODES)).await;
    for (name, id, case) in [
        ("maintained-domain", ABSENT_NODE_ID, "an id nobody minted"),
        (
            "other-domain",
            maintenance_id.as_str(),
            "another domain's action",
        ),
    ] {
        let answer = status(&emulator, name, id).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected the missing resource's HTTP {RECORDED_NOT_FOUND_STATUS}, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_listing_reports_every_action_once_and_narrows_by_action_and_status() {
    let emulator = Emulator::new();
    create(&emulator, "listed-domain", every_role(MAINTAINABLE_DATA_NODES)).await;
    let nodes: Vec<String> = census(&emulator, "listed-domain")
        .await
        .into_iter()
        .filter(|(_, node_type)| node_type == NODE_TYPE_DATA)
        .map(|(id, _)| id)
        .collect();

    // Two actions per published `MaintenanceType`, so a filter on one action has
    // several to report and several to leave out.
    let mut started: Vec<(String, String)> = Vec::new();
    for action in MAINTENANCE_TYPES.iter().chain(MAINTENANCE_TYPES.iter()) {
        let node_id = nodes[started.len() % nodes.len()].clone();
        let answer = start(&emulator, "listed-domain", action, Some(node_id.as_str())).await;
        assert_eq!(
            answer.status, 200,
            "case StartDomainMaintenance({action}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        started.push((
            string_member(&answer.body, "MaintenanceId", action),
            (*action).to_string(),
        ));
    }

    // An unfiltered listing reports every action, each once; the newest first, so
    // the ids are the started ones reversed.
    let answer = list(&emulator, "listed-domain", "").await;
    assert_eq!(
        answer.status, 200,
        "case ListDomainMaintenances(listed-domain): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.body["DomainMaintenances"]
        .as_array()
        .unwrap_or_else(|| panic!("case the whole listing: expected a list, got {:?}", answer.body))
        .clone();
    let ids: Vec<String> = reported
        .iter()
        .map(|details| string_member(details, "MaintenanceId", "the whole listing"))
        .collect();
    let expected: Vec<String> = started
        .iter()
        .rev()
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(
        ids, expected,
        "case the whole listing: expected every action once, newest first, got {ids:?}"
    );
    // Every row is the projection `GetDomainMaintenanceStatus` answers with, plus
    // the domain the action belongs to.
    for details in &reported {
        let id = string_member(details, "MaintenanceId", "the whole listing");
        let read = status(&emulator, "listed-domain", &id).await;
        for member in ["Action", "NodeId", "Status", "CreatedAt", "UpdatedAt"] {
            assert_eq!(
                details[member], read.body[member],
                "case {id}: the listing's {member} must be the one the status read reports \
                 ({:?}), got {:?}",
                read.body[member], details[member]
            );
        }
        assert_eq!(
            details["DomainName"].as_str(),
            Some("listed-domain"),
            "case {id}: every row names the domain the action ran on, got {details:?}"
        );
    }

    // `Action` and `Status` each narrow the listing to the actions carrying the
    // value named, and a value nothing carries selects nothing.
    for action in MAINTENANCE_TYPES {
        let answer = list(&emulator, "listed-domain", &format!("?action={action}")).await;
        let rows = answer.body["DomainMaintenances"]
            .as_array()
            .unwrap_or_else(|| panic!("case action={action}: expected a list, got {:?}", answer.body))
            .clone();
        assert_eq!(
            rows.len(),
            2,
            "case action={action}: expected the two actions started under it, got {rows:?}"
        );
        assert!(
            rows.iter()
                .all(|details| details["Action"].as_str() == Some(action)),
            "case action={action}: every row carries the action asked for, got {rows:?}"
        );
    }
    for (query, expected, case) in [
        (
            format!("?status={MAINTENANCE_STATUS_PENDING}"),
            started.len(),
            "every action is still pending",
        ),
        (
            format!("?status={MODELLED_FINISHED_STATUS}"),
            0,
            "no action carries the model's finished status",
        ),
        (
            format!("?action={}&status={MAINTENANCE_STATUS_PENDING}", MAINTENANCE_TYPES[0]),
            2,
            "both filters at once",
        ),
    ] {
        let answer = list(&emulator, "listed-domain", &query).await;
        let rows = answer.body["DomainMaintenances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a list, got {:?}", answer.body))
            .len();
        assert_eq!(
            rows, expected,
            "case {case}: expected {expected} rows for {query}, got {rows}"
        );
    }

    // Every page size walks the listing exactly once, in the one order, plus the
    // uncapped default; a token this operation did not hand out is refused.
    for max_results in [None, Some(1), Some(2), Some(5), Some(100)] {
        let case = format!("maxResults {max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = String::from("?");
            if let Some(size) = max_results {
                query.push_str(&format!("maxResults={size}&"));
            }
            if let Some(next) = token.as_deref() {
                query.push_str(&format!("nextToken={next}&"));
            }
            let answer = list(&emulator, "listed-domain", query.trim_end_matches(['?', '&'])).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = answer.body["DomainMaintenances"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a list, got {:?}", answer.body))
                .clone();
            if let Some(size) = max_results {
                assert!(
                    page.len() <= usize::try_from(size).unwrap_or(0),
                    "case {case}: a page holds no more than the maximum asked for, got {}",
                    page.len()
                );
            }
            for details in &page {
                seen.push(string_member(details, "MaintenanceId", &case));
            }
            pages += 1;
            assert!(
                pages <= started.len() + 1,
                "case {case}: paging terminates, got {pages} pages"
            );
            match answer.body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: the pages concatenated are every action, once each and in order"
        );
    }
    for (query, case) in [
        ("?nextToken=not-a-token", "a token nobody minted"),
        ("?maxResults=-1", "a negative maximum"),
        ("?maxResults=101", "a maximum past the modelled bound"),
    ] {
        let answer = list(&emulator, "listed-domain", query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // A domain with no action listed holds an empty list rather than a refusal, and
    // a name no domain is stored under is the generic envelope.
    create(&emulator, "quiet-domain", every_role(MAINTAINABLE_DATA_NODES)).await;
    let answer = list(&emulator, "quiet-domain", "").await;
    assert_eq!(
        (
            answer.status,
            answer.body["DomainMaintenances"].as_array().map(Vec::len)
        ),
        (200, Some(0)),
        "case a domain with no action: expected an empty listing, got {} with {:?}",
        answer.status,
        answer.body
    );
    let answer = list(&emulator, "absent-domain", "").await;
    assert_eq!(
        (answer.status, error_code(&answer).as_str()),
        (400, "BaseException"),
        "case an unknown domain: expected the generic envelope, got {} with {:?}",
        answer.status,
        answer.body
    );
}
