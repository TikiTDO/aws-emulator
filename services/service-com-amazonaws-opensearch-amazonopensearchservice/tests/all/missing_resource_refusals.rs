// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN this service decides a request addresses nothing, and how
//! it says so.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness and the per-operation request helpers
//! come from the sibling suites; this one adds no second copy of either.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one call:
//!
//! * the resource a request ADDRESSES is resolved before any rule the handler itself
//!   applies to the request's own members, so a request naming a domain this account
//!   does not hold is the missing-resource refusal whatever else is wrong with it — over
//!   a table of malformed index documents and of maintenance requests naming no node,
//!   each row driven twice: against a domain that resolves to nothing, where the answer
//!   is the missing resource, and against a domain the account holds, where the SAME
//!   request earns the handler's own refusal, so resolving first does not suppress the
//!   rules that follow;
//! * a read of ONE insight refuses an ENTITY it cannot resolve as one missing resource,
//!   naming neither the entity nor the id — over a table of entities this account holds
//!   nothing for, each against several ids — while an entity whose type is outside the
//!   published set stays the malformed request it is, the insight an entity DOES hold is
//!   reported, and an id a resolved entity holds no insight under is answered with the
//!   guidance the service states about an insight's details;
//! * each of those refusals is answered the way every missing-resource refusal of this
//!   service is answered ([`assert_missing_resource`]), which is not the status the error
//!   shape's own `@httpError` declares.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::INSIGHT_ENTITY_TYPE_VALUES;

use super::control_plane_refusal_wording::message;
use super::data_source_registry::created_domain;
use super::domain_lifecycle::{error_code, Answer, Emulator};
use super::domain_maintenance::{start as start_maintenance, MAINTENANCE_TYPES};
use super::index_registry::{create as create_index, stored_names};
use super::index_schema_updates::update as update_index;
use super::insight_details::{describe_insight, domain_entity, guid, raise_on_domain, record};
use super::missing_resource_status::{absent_domains, assert_missing_resource};
use super::saved_object_migrations::get_migration;

/// The index name every row below writes under: inside `IndexName`'s own modelled
/// pattern, so no row is refused for its name instead of for what it is testing.
const INDEX: &str = "catalog";

/// A `NodeId` inside `NodeId`'s `@length(10, 40)` that no node census publishes.
const ABSENT_NODE_ID: &str = "0123456789abcdef0123456789abcdef";

/// The `IndexSchema` documents an index write's own rule refuses: the member is a
/// modelled document "defining index mappings, settings, and semantic enrichment
/// configuration" — a set of named sections — so the rows differ in the JSON value type
/// the document carries instead.
fn malformed_schemas() -> Vec<(&'static str, Value)> {
    vec![
        ("a number", json!(7)),
        ("a string", json!("mappings")),
        ("an array", json!([{ "mappings": {} }])),
        ("a boolean", json!(true)),
    ]
}

/// Assert `answer` is the missing-resource refusal of the domain `name`: the status and
/// the error code every missing-resource refusal of this service carries
/// ([`assert_missing_resource`]), naming the name the request carried.
fn assert_missing_domain(answer: &Answer, case: &str, name: &str) {
    assert_missing_resource(answer, case);
    let expected = format!("Domain not found: {name}");
    assert_eq!(
        message(answer),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the refusal of a request the handler's own rule rejects, with the
/// sentence that rule states.
fn assert_refused_by_its_own_rule(answer: &Answer, case: &str, sentence: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (400, "ValidationException"),
        "case {case}: expected 400 ValidationException, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        message(answer),
        sentence,
        "case {case}: expected {sentence:?}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_absent_domain_is_refused_before_an_index_write_judges_its_document() {
    let emulator = Emulator::new();
    let held = "index-precedence";
    created_domain(&emulator, held).await;
    let malformed = "IndexSchema must be a JSON object of index schema sections.";

    for (shape, schema) in malformed_schemas() {
        for absent in absent_domains() {
            let create = create_index(
                &emulator,
                absent,
                json!({ "IndexName": INDEX, "IndexSchema": schema }),
            )
            .await;
            assert_missing_domain(
                &create,
                &format!("CreateIndex({absent:?}, a schema that is {shape})"),
                absent,
            );

            let update = update_index(&emulator, absent, INDEX, &schema).await;
            assert_missing_domain(
                &update,
                &format!("UpdateIndex({absent:?}, a schema that is {shape})"),
                absent,
            );
        }

        // The SAME document against a domain this account DOES hold: resolving the
        // domain first is a question of ORDER, so the document's own rule still decides
        // the answer where the domain resolves.
        let create = create_index(
            &emulator,
            held,
            json!({ "IndexName": INDEX, "IndexSchema": schema }),
        )
        .await;
        assert_refused_by_its_own_rule(
            &create,
            &format!("CreateIndex({held:?}, a schema that is {shape})"),
            malformed,
        );
        let update = update_index(&emulator, held, INDEX, &schema).await;
        assert_refused_by_its_own_rule(
            &update,
            &format!("UpdateIndex({held:?}, a schema that is {shape})"),
            malformed,
        );
    }

    // Every call above was refused, so the domain that DOES exist holds no index: the
    // order the refusals are decided in writes nothing either way.
    let stored = stored_names(&emulator, held, "after the refused writes");
    assert!(
        stored.is_empty(),
        "case after the refused writes: expected {held:?} to hold no index, got {stored:?}"
    );
}

#[tokio::test]
async fn an_absent_domain_is_refused_before_the_maintenance_start_requires_a_node() {
    let emulator = Emulator::new();
    let held = "maintenance-precedence";
    created_domain(&emulator, held).await;
    let node_required = "NodeId is required to start domain maintenance";

    // One row per `MaintenanceType` the model publishes, so the rule is asserted over
    // the action as well as over the domain.
    for action in MAINTENANCE_TYPES {
        for absent in absent_domains() {
            let answer = start_maintenance(&emulator, absent, action, None).await;
            assert_missing_domain(
                &answer,
                &format!("StartDomainMaintenance({absent:?}, {action}, naming no node)"),
                absent,
            );
        }

        // The same request against a domain this account holds: the action runs on ONE
        // node, so the requirement the handler itself states is what refuses it there.
        let answer = start_maintenance(&emulator, held, action, None).await;
        assert_refused_by_its_own_rule(
            &answer,
            &format!("StartDomainMaintenance({held:?}, {action}, naming no node)"),
            node_required,
        );
    }
}

#[tokio::test]
async fn a_read_of_one_insight_names_neither_half_of_an_entity_it_cannot_resolve() {
    let emulator = Emulator::new();
    let held = "insight-address";
    created_domain(&emulator, held).await;
    let resource_not_found = "Resource not found";

    // The ids differ in what the field that tells one `GUID` from another spells, so the
    // rule is asserted over the id as well as over the entity.
    let ids = [
        guid("000000000000"),
        guid("ffffffffffff"),
        guid("0a1b2c3d4e5f"),
    ];

    for absent in absent_domains() {
        for id in &ids {
            let answer = describe_insight(&emulator, domain_entity(absent), id, json!({})).await;
            let case = format!("DescribeInsightDetails(DomainName {absent:?}, {id})");
            assert_missing_resource(&answer, &case);
            let said = message(&answer);
            assert_eq!(
                said, resource_not_found,
                "case {case}: expected {resource_not_found:?}, got {:?}",
                answer.body
            );
            // Neither half of the address the request carried is disclosed: a caller
            // cannot tell an entity this account does not hold from an insight that
            // entity does not hold.
            assert!(
                !said.contains(absent) && !said.contains(id.as_str()),
                "case {case}: expected the refusal to name neither half of the address, got \
                 {said:?}"
            );
        }
    }

    // An entity whose TYPE is outside the published set names no kind of resource at
    // all, so it stays the malformed request it is rather than becoming a missing one.
    for entity_type in ["Domain", "domainname", "Cluster"] {
        assert!(
            !INSIGHT_ENTITY_TYPE_VALUES.contains(&entity_type),
            "case DescribeInsightDetails({entity_type:?}): the row must name no published \
             entity type, and {INSIGHT_ENTITY_TYPE_VALUES:?} does"
        );
        let answer = describe_insight(
            &emulator,
            json!({ "Type": entity_type, "Value": held }),
            &ids[0],
            json!({}),
        )
        .await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case DescribeInsightDetails({entity_type:?}): expected 400 ValidationException, \
             got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    // Only the ENTITY half of the address decides whether the read resolves: the
    // insight this entity DOES hold is reported, and an id it holds no insight under is
    // ANSWERED — with the guidance the service states about an insight's details, which
    // is what the recorded wire answer for such a read carries (the detail read of
    // `opensearch-domain-insights-operations`), so the id is not resolved the way the
    // entity is.
    let held_id = guid("00000000000a");
    raise_on_domain(
        &emulator,
        held,
        record(&held_id, SystemTime::now(), Vec::new()),
    );
    let answer = describe_insight(&emulator, domain_entity(held), &held_id, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeInsightDetails(DomainName {held:?}, {held_id}): expected 200 for the \
         insight the entity holds, got {} with {:?}",
        answer.status, answer.body
    );
    for id in &ids {
        let answer = describe_insight(&emulator, domain_entity(held), id, json!({})).await;
        let case = format!("DescribeInsightDetails(DomainName {held:?}, {id})");
        assert_eq!(
            answer.status, 200,
            "case {case}: an id a RESOLVED entity holds no insight under is answered, so \
             expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["Fields"][0]["Name"].as_str(),
            Some("Message"),
            "case {case}: expected the guidance field the service states, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn each_refusal_the_recording_covers_is_answered_on_the_recorded_status() {
    let emulator = Emulator::new();
    let absent = "status-absent-domain";

    // One row per operation whose missing-resource refusal a recorded wire answer
    // covers, each addressing something this account holds nothing for: a domain, the
    // application a `migrationId` names, and the entity an insight read addresses.
    let rows: Vec<(String, Answer)> = vec![
        (
            format!("CreateIndex({absent:?})"),
            create_index(
                &emulator,
                absent,
                json!({ "IndexName": INDEX, "IndexSchema": { "mappings": {} } }),
            )
            .await,
        ),
        (
            format!("UpdateIndex({absent:?})"),
            update_index(&emulator, absent, INDEX, &json!({ "mappings": {} })).await,
        ),
        (
            format!("StartDomainMaintenance({absent:?})"),
            start_maintenance(&emulator, absent, MAINTENANCE_TYPES[0], Some(ABSENT_NODE_ID)).await,
        ),
        (
            "GetMigration(an application this account does not hold)".to_string(),
            get_migration(&emulator, "notheldhere-000000000000").await,
        ),
        (
            "DescribeInsightDetails(an entity this account does not hold)".to_string(),
            describe_insight(
                &emulator,
                domain_entity(absent),
                &guid("000000000000"),
                json!({}),
            )
            .await,
        ),
    ];

    for (case, answer) in rows {
        assert_missing_resource(&answer, &case);
    }
}
