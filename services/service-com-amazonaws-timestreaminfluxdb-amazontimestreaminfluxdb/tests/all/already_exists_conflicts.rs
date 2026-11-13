// SPDX-License-Identifier: CC0-1.0

//! The refusal a create gets when the name it asks for is already taken, over the wire.
//!
//! RULE: every resource family this service holds a per-Region unique-name index on —
//! DB instances, DB clusters and DB parameter groups — refuses a second create under a
//! taken name with HTTP 409 `ConflictException`, whose `resourceId` is the id of the
//! record that HOLDS the name, whose `resourceType` is that family's type, and whose
//! message is this service's already-exists sentence: the fixed text `The `, ` with id [`,
//! `], name [`, `], customer [`, `] already exists`, with the resource spelled as the
//! model names its shape and the three variable parts taken from this emulator's state and
//! the request — the existing record's id, the name the request asked for, and the account
//! the records belong to.
//!
//! A client reconciling create-if-absent logic parses the existing record's id out of that
//! sentence to adopt the record instead of failing, so the id is asserted as part of the
//! message and not only as the `resourceId` member.
//!
//! The rows span what the rule generalises over: the resource FAMILY (three of them, each
//! with its own create, its own id member and its own `resourceType`) and the NAME's form
//! and length (the modelled minimum 3, a hyphenated name, and the longest name every one
//! of the three families accepts).

use serde_json::{json, Value};
use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{call, ok, router_and_engine, ACCOUNT};

/// A router whose account already holds the AWS Marketplace subscription the one cluster
/// deployment type is gated behind, so a cluster row reaches the name index rather than
/// that gate (the gate itself is
/// [`crate::db_cluster_marketplace_subscription`]'s rule). A buyer subscribes in AWS
/// Marketplace, out of band of every operation here.
fn router() -> MultiServiceRouter {
    let (router, engine) = router_and_engine();
    influxdb::state::subscribe_account_to_marketplace_offering(
        &*engine,
        influxdb::state::READ_REPLICAS_OFFERING,
    )
    .expect("the shared engine holds this service's store");
    router
}

/// One resource family: how a create of it is asked for, and how its refusal names it.
struct Family {
    /// The operation that creates one.
    operation: &'static str,
    /// The request body the create needs beside `name`, per the model's `@required`
    /// members.
    body: fn(&str) -> Value,
    /// The output member carrying the service-generated id of the created record.
    id_member: &'static str,
    /// How the already-exists sentence spells the resource: the shape name the model gives
    /// it (`smithy_query(query="digest")`).
    spelling: &'static str,
    /// The `resourceType` the refusal reports, a value of this service's own reporting
    /// vocabulary (`crate::state::RESOURCE_TYPE_*`).
    resource_type: &'static str,
}

/// The three families, each created with exactly the members its model marks `@required`.
fn families() -> Vec<Family> {
    vec![
        Family {
            operation: "CreateDbParameterGroup",
            body: |name| json!({ "name": name }),
            id_member: "id",
            spelling: "DbParameterGroup",
            resource_type: "DB_PARAMETER_GROUP",
        },
        Family {
            operation: "CreateDbInstance",
            body: |name| {
                json!({
                    "name": name,
                    "password": "supersecret1",
                    "dbInstanceType": "db.influx.medium",
                    "allocatedStorage": 20,
                    "vpcSubnetIds": ["subnet-aaa"],
                    "vpcSecurityGroupIds": ["sg-aaa"],
                })
            },
            id_member: "id",
            spelling: "DbInstance",
            resource_type: "DB_INSTANCE",
        },
        Family {
            operation: "CreateDbCluster",
            body: |name| {
                json!({
                    "name": name,
                    "dbInstanceType": "db.influx.medium",
                    "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
                    "vpcSecurityGroupIds": ["sg-aaa"],
                })
            },
            id_member: "dbClusterId",
            spelling: "DbCluster",
            resource_type: "DB_CLUSTER",
        },
    ]
}

/// The names the rule is asserted over: `DbInstanceName` and `DbClusterName` are length
/// 3..40 and `DbParameterGroupName` 3..64, all three under the pattern
/// `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$` — so the longest name every family accepts is
/// 40 characters.
fn names() -> Vec<String> {
    vec![
        "Abc".to_string(),
        "taken-name-9".to_string(),
        format!("N{}", "a".repeat(39)),
    ]
}

/// The id a create answered with.
fn id_of(case: &str, member: &str, body: &Value) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member:?}, got {body}"))
        .to_string()
}

#[tokio::test]
async fn a_taken_name_is_refused_with_the_already_exists_sentence() {
    for family in families() {
        for name in names() {
            // One emulator per case, so a name taken in one row cannot reach the next.
            let router = router();
            let case = format!("{} under name {name:?}", family.operation);

            let first = ok(
                &case,
                call(&router, family.operation, (family.body)(&name)).await,
            );
            let existing_id = id_of(&case, family.id_member, &first);

            let (status, body) = call(&router, family.operation, (family.body)(&name)).await;
            assert_eq!(
                status, 409,
                "case {case:?}: a taken name must conflict, got {status} with {body}"
            );
            assert!(
                body["__type"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with("ConflictException"),
                "case {case:?}: expected a ConflictException, got {body}"
            );
            let expected = format!(
                "The {} with id [{existing_id}], name [{name}], customer [{ACCOUNT}] \
                 already exists",
                family.spelling
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected {expected:?}, got {:?}",
                body["message"]
            );
            assert_eq!(
                body["resourceId"].as_str(),
                Some(existing_id.as_str()),
                "case {case:?}: the refusal must name the record that holds the name, got \
                 {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(family.resource_type),
                "case {case:?}: unexpected resourceType in {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_sentence_names_the_record_that_holds_the_name_not_the_one_refused() {
    // Two groups: the refusal of a create under the FIRST group's name must carry the
    // first group's id, which is what tells a client which record to adopt — so the id in
    // the sentence tracks the record that holds the name rather than any fixed value.
    let router = router();
    let mut ids = Vec::new();
    for name in ["Alpha", "Beta"] {
        let created = ok(
            name,
            call(&router, "CreateDbParameterGroup", json!({ "name": name })).await,
        );
        ids.push(id_of(name, "id", &created));
    }

    for (name, existing_id) in ["Alpha", "Beta"].iter().zip(&ids) {
        let (status, body) = call(
            &router,
            "CreateDbParameterGroup",
            json!({ "name": name, "description": "a second attempt" }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected HTTP 409, got {status} with {body}"
        );
        let expected = format!(
            "The DbParameterGroup with id [{existing_id}], name [{name}], \
             customer [{ACCOUNT}] already exists"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected.as_str()),
            "case {name:?}: expected {expected:?}, got {:?}",
            body["message"]
        );
    }

    // The refused creates changed nothing: both groups still read back as created.
    for (name, id) in ["Alpha", "Beta"].iter().zip(&ids) {
        let fetched = ok(
            name,
            call(&router, "GetDbParameterGroup", json!({ "identifier": id })).await,
        );
        assert_eq!(
            fetched["name"].as_str(),
            Some(*name),
            "case {name:?}: expected the stored record, got {fetched}"
        );
    }
}
