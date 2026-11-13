// SPDX-License-Identifier: CC0-1.0

//! A managed interface endpoint's subnet and security-group lists are SETS: a write that
//! names one id twice is refused, end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: every list member of a VPC endpoint write names a SET of ids — the
//! subnets and the security groups the endpoint holds, which an accepted write never leaves
//! holding one id twice — so a member that names one id more than once describes no one set
//! and is refused as invalid input, with the clause the validation layer renders for a
//! repeated element of a list, before anything is stored or changed. The same rule the
//! tagging members already hold (`tag_key_uniqueness.rs`), on the members that carry ids.
//!
//! The tables span what the rule generalises over: which operation writes
//! (`CreateVpcEndpoint` and `UpdateVpcEndpoint`), which of the five list members carries the
//! repeat, both modelled `SubnetId` widths, how many copies the list names (a pair, a
//! non-adjacent pair, and a whole list of copies up to each bound), whether the repeated id
//! is one the endpoint already holds, and whether other entries of the same list are
//! distinct.

use serde_json::{json, Value};

use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
    MAX_SECURITY_GROUP_IDS, MAX_SUBNET_IDS,
};

use crate::collection_lifecycle::{call, emulator, text, CLIENT_ERROR};
use crate::vpc_endpoint::{await_active, create_endpoint, ids, read_one};

/// A VPC whose id satisfies `VpcId`'s `^vpc-[0-9a-z]*$`.
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// A subnet id of the SHORT modelled width: `SubnetId` is
/// `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="SubnetIds")`).
fn subnet(index: usize) -> String {
    format!("subnet-{index:08x}")
}

/// A subnet id of the LONG modelled width — the 17-hex-digit branch of `SubnetId`'s
/// pattern.
fn long_subnet(index: usize) -> String {
    format!("subnet-{index:017x}")
}

/// A security group id inside `SecurityGroupId`'s `^[\w+\-]+$`
/// (`smithy_query(query="shape", shape="SecurityGroupIds")`).
fn group(index: usize) -> String {
    format!("sg-{index:08x}")
}

/// The refusal this service words for a member that names `value` more than once: the clause
/// its validation layer renders for a repeated element of a list, pinned for the tagging
/// members in `tag_key_uniqueness.rs`.
fn repeated_element_message(member: &str, value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: Member must have unique values"
    )
}

/// One row of the refusal table: a write whose `member` names `repeated` twice.
struct Repeat {
    case: &'static str,
    operation: &'static str,
    /// The member path the refusal names, which is the member that carried the repeat.
    member: &'static str,
    /// The request, without the `name` a create row is given below.
    request: Value,
    /// The id the request names more than once — the FIRST such id when several do.
    repeated: String,
}

/// Assert that `entries` holds no id twice — the invariant every stored list satisfies.
fn assert_set(case: &str, member: &str, entries: &[String]) {
    let mut sorted = entries.to_vec();
    sorted.sort();
    let distinct = sorted.len();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        distinct,
        "case {case}: expected {member} to hold every id once, got {entries:?}"
    );
}

#[tokio::test]
async fn a_repeated_id_is_refused_wherever_a_vpc_endpoint_write_names_one() {
    let router = emulator();

    // The endpoint the update rows address: settled, so no row is refused for the status it
    // is in instead of for the list it carries.
    let held_subnets = vec![subnet(0), subnet(1)];
    let held_groups = vec![group(0), group(1)];
    let detail = create_endpoint(
        &router,
        "the endpoint",
        json!({
            "name": "set-rule-endpoint",
            "vpcId": VPC_ID,
            "subnetIds": held_subnets,
            "securityGroupIds": held_groups,
        }),
    )
    .await;
    let id = text(&detail, "id");
    await_active(&router, "the endpoint", &id).await;

    let mut rows: Vec<Repeat> = vec![
        Repeat {
            case: "a create naming one subnet in two adjacent entries",
            operation: "CreateVpcEndpoint",
            member: "subnetIds",
            request: json!({ "vpcId": VPC_ID, "subnetIds": [subnet(1), subnet(1)] }),
            repeated: subnet(1),
        },
        Repeat {
            case: "a create repeating its first subnet after a distinct one",
            operation: "CreateVpcEndpoint",
            member: "subnetIds",
            request: json!({
                "vpcId": VPC_ID,
                "subnetIds": [subnet(0), subnet(1), subnet(0)],
            }),
            repeated: subnet(0),
        },
        Repeat {
            case: "a create naming a long-width subnet twice",
            operation: "CreateVpcEndpoint",
            member: "subnetIds",
            request: json!({ "vpcId": VPC_ID, "subnetIds": [long_subnet(2), long_subnet(2)] }),
            repeated: long_subnet(2),
        },
        Repeat {
            // The issue's own shape: a list AT the stored bound carrying one real subnet,
            // which no length check can catch.
            case: "a create naming one subnet as many times as an endpoint may hold subnets",
            operation: "CreateVpcEndpoint",
            member: "subnetIds",
            request: json!({ "vpcId": VPC_ID, "subnetIds": vec![subnet(3); MAX_SUBNET_IDS] }),
            repeated: subnet(3),
        },
        Repeat {
            case: "a create naming two subnets twice each",
            operation: "CreateVpcEndpoint",
            member: "subnetIds",
            request: json!({
                "vpcId": VPC_ID,
                "subnetIds": [subnet(4), subnet(4), subnet(5), subnet(5)],
            }),
            repeated: subnet(4),
        },
        Repeat {
            case: "a create naming one security group twice, its subnets distinct",
            operation: "CreateVpcEndpoint",
            member: "securityGroupIds",
            request: json!({
                "vpcId": VPC_ID,
                "subnetIds": [subnet(0), subnet(1)],
                "securityGroupIds": [group(2), group(2)],
            }),
            repeated: group(2),
        },
        Repeat {
            case: "a create naming one security group as many times as the published maximum",
            operation: "CreateVpcEndpoint",
            member: "securityGroupIds",
            request: json!({
                "vpcId": VPC_ID,
                "subnetIds": [subnet(0)],
                "securityGroupIds": vec![group(3); MAX_SECURITY_GROUP_IDS],
            }),
            repeated: group(3),
        },
        Repeat {
            case: "an add list naming one new subnet twice",
            operation: "UpdateVpcEndpoint",
            member: "addSubnetIds",
            request: json!({ "id": id, "addSubnetIds": [subnet(6), subnet(6)] }),
            repeated: subnet(6),
        },
        Repeat {
            case: "an add list naming a subnet the endpoint already holds twice",
            operation: "UpdateVpcEndpoint",
            member: "addSubnetIds",
            request: json!({ "id": id, "addSubnetIds": [subnet(0), subnet(0)] }),
            repeated: subnet(0),
        },
        Repeat {
            // Inside the bound on one request's list, so only the set rule refuses it: the
            // copies are not what the bounds count.
            case: "an add list of as many copies of one subnet as an endpoint may hold subnets",
            operation: "UpdateVpcEndpoint",
            member: "addSubnetIds",
            request: json!({ "id": id, "addSubnetIds": vec![subnet(7); MAX_SUBNET_IDS] }),
            repeated: subnet(7),
        },
        Repeat {
            case: "an add list naming a long-width subnet twice",
            operation: "UpdateVpcEndpoint",
            member: "addSubnetIds",
            request: json!({ "id": id, "addSubnetIds": [long_subnet(8), long_subnet(8)] }),
            repeated: long_subnet(8),
        },
        Repeat {
            case: "a remove list naming one held subnet twice",
            operation: "UpdateVpcEndpoint",
            member: "removeSubnetIds",
            request: json!({ "id": id, "removeSubnetIds": [subnet(1), subnet(1)] }),
            repeated: subnet(1),
        },
        Repeat {
            case: "a remove list repeating one held subnet around a distinct one",
            operation: "UpdateVpcEndpoint",
            member: "removeSubnetIds",
            request: json!({
                "id": id,
                "removeSubnetIds": [subnet(1), subnet(0), subnet(1)],
            }),
            repeated: subnet(1),
        },
        Repeat {
            case: "an add list naming one new security group twice",
            operation: "UpdateVpcEndpoint",
            member: "addSecurityGroupIds",
            request: json!({ "id": id, "addSecurityGroupIds": [group(4), group(4)] }),
            repeated: group(4),
        },
        Repeat {
            case: "a remove list naming one held security group twice",
            operation: "UpdateVpcEndpoint",
            member: "removeSecurityGroupIds",
            request: json!({ "id": id, "removeSecurityGroupIds": [group(0), group(0)] }),
            repeated: group(0),
        },
    ];

    // Every create row gets its own `VpcEndpointName` (`^[a-z][a-z0-9-]+$`, 3..32), so each
    // name is still free afterwards for the check below that nothing was stored under it.
    let mut refused_names: Vec<String> = Vec::new();
    for (index, row) in rows.iter_mut().enumerate() {
        if row.operation == "CreateVpcEndpoint" {
            let name = format!("set-rule-create-{index}");
            row.request["name"] = json!(name);
            refused_names.push(name);
        }
    }

    for row in &rows {
        let (status, code, answer) = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {}: expected {CLIENT_ERROR} ValidationException from {}, got {status} {code} with {answer}",
            row.case,
            row.operation
        );
        assert_eq!(
            text(&answer, "message"),
            repeated_element_message(row.member, &row.repeated),
            "case {}: expected the repeated-element refusal at {}, got {answer}",
            row.case,
            row.member
        );
    }

    // Nothing the refused updates named reached the endpoint.
    let detail = read_one(&router, "the refused updates", &id).await;
    assert_eq!(
        ids(&detail, "subnetIds"),
        held_subnets,
        "case the refused updates: expected the subnets the create stored, got {detail}"
    );
    assert_eq!(
        ids(&detail, "securityGroupIds"),
        held_groups,
        "case the refused updates: expected the security groups the create stored, got {detail}"
    );

    // Nothing the refused creates named was stored either: each name is free, so the same
    // name with distinct ids is accepted rather than conflicting with a half-created
    // endpoint.
    for name in &refused_names {
        let case = format!("the name {name} after its refused create");
        let created = create_endpoint(
            &router,
            &case,
            json!({ "name": name, "vpcId": VPC_ID, "subnetIds": [subnet(9)] }),
        )
        .await;
        // `CreateVpcEndpointDetail` models id, name and status only
        // (`smithy_query(query="shape", shape="CreateVpcEndpointDetail")`), so what the
        // endpoint carries is read back through `BatchGetVpcEndpoint`.
        let detail = read_one(&router, &case, &text(&created, "id")).await;
        assert_eq!(
            ids(&detail, "subnetIds"),
            vec![subnet(9)],
            "case {case}: expected the one subnet this create named, got {detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// The accepted side: distinct lists are stored as sent, stay sets across updates, and the
// bounds count the ids the endpoint really holds.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn distinct_ids_are_stored_as_sent_and_the_bounds_count_them_once() {
    let router = emulator();

    let created = create_endpoint(
        &router,
        "the distinct create",
        json!({
            "name": "set-rule-accepted",
            "vpcId": VPC_ID,
            "subnetIds": [subnet(0), subnet(1)],
            "securityGroupIds": [group(0)],
        }),
    )
    .await;
    let id = text(&created, "id");
    // `CreateVpcEndpointDetail` models id, name and status only
    // (`smithy_query(query="shape", shape="CreateVpcEndpointDetail")`), so the stored lists
    // are read back through `BatchGetVpcEndpoint`.
    let stored = read_one(&router, "the distinct create", &id).await;
    assert_eq!(
        ids(&stored, "subnetIds"),
        vec![subnet(0), subnet(1)],
        "case the distinct create: expected the subnets it named, got {stored}"
    );
    assert_eq!(
        ids(&stored, "securityGroupIds"),
        vec![group(0)],
        "case the distinct create: expected the security group it named, got {stored}"
    );
    assert_set(
        "the distinct create",
        "subnetIds",
        &ids(&stored, "subnetIds"),
    );
    await_active(&router, "the distinct create", &id).await;

    // An id the endpoint already holds, named ONCE beside a new one, is the idempotent add
    // the set rule leaves alone: it is held once afterwards, not twice.
    let (status, code, answer) = call(
        &router,
        "UpdateVpcEndpoint",
        json!({ "id": id, "addSubnetIds": [subnet(1), subnet(2)] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the idempotent add: expected 200 from UpdateVpcEndpoint, got {status} {code} with {answer}"
    );
    await_active(&router, "the idempotent add", &id).await;
    let detail = read_one(&router, "the idempotent add", &id).await;
    assert_eq!(
        ids(&detail, "subnetIds"),
        vec![subnet(0), subnet(1), subnet(2)],
        "case the idempotent add: expected the held subnets once each plus the new one, got {detail}"
    );

    // One removal takes away exactly the one subnet it names, leaving the rest: a stored
    // list that held an id once cannot lose several entries to a single removal.
    let (status, code, answer) = call(
        &router,
        "UpdateVpcEndpoint",
        json!({ "id": id, "removeSubnetIds": [subnet(1)] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the single removal: expected 200 from UpdateVpcEndpoint, got {status} {code} with {answer}"
    );
    await_active(&router, "the single removal", &id).await;
    let detail = read_one(&router, "the single removal", &id).await;
    assert_eq!(
        ids(&detail, "subnetIds"),
        vec![subnet(0), subnet(2)],
        "case the single removal: expected the subnets it did not name, got {detail}"
    );

    // The subnet bound counts the ids an endpoint really holds: an endpoint carrying one
    // subnet takes every remaining one up to the bound, and only then is an add refused.
    let created = create_endpoint(
        &router,
        "the bound case",
        json!({ "name": "set-rule-bound", "vpcId": VPC_ID, "subnetIds": [subnet(10)] }),
    )
    .await;
    let bounded = text(&created, "id");
    await_active(&router, "the bound case", &bounded).await;
    let filling: Vec<String> = (11..10 + MAX_SUBNET_IDS).map(subnet).collect();
    let (status, code, answer) = call(
        &router,
        "UpdateVpcEndpoint",
        json!({ "id": bounded, "addSubnetIds": filling }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the bound case: expected 200 from UpdateVpcEndpoint, got {status} {code} with {answer}"
    );
    await_active(&router, "the bound case", &bounded).await;
    let detail = read_one(&router, "the bound case", &bounded).await;
    let held = ids(&detail, "subnetIds");
    assert_set("the bound case", "subnetIds", &held);
    assert_eq!(
        held.len(),
        MAX_SUBNET_IDS,
        "case the bound case: expected {MAX_SUBNET_IDS} subnets held, got {detail}"
    );
    let (status, code, answer) = call(
        &router,
        "UpdateVpcEndpoint",
        json!({ "id": bounded, "addSubnetIds": [subnet(20)] }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ValidationException"),
        "case one past the bound: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
    );
    assert!(
        text(&answer, "message")
            .contains(&format!("The endpoint must not have more than {MAX_SUBNET_IDS} subnets")),
        "case one past the bound: expected the subnet-bound refusal, got {answer}"
    );
}
