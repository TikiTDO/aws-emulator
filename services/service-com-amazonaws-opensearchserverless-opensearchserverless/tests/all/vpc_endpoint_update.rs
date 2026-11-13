// SPDX-License-Identifier: CC0-1.0

//! Updating an OpenSearch Serverless-managed interface VPC endpoint, end to end over the
//! wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: `UpdateVpcEndpoint` applies its add and remove lists to the STORED
//! endpoint its `id` names as a change to a SET of ids — an id already held is not held
//! twice and one not held is unaffected by a removal — reports that record in the
//! non-terminal status the change is applied in with the moment it was modified, and leaves
//! what a later `BatchGetVpcEndpoint` answers; a result outside the modelled bounds on those
//! lists, an id naming no endpoint and a contradictory request are refused as invalid input,
//! an endpoint in a status it may not be changed from is refused as a conflict, and a
//! replayed `clientToken` answers the record its first call left without applying anything
//! twice. The tables span what the rule generalises over: which of the four lists the
//! request names, the counts on each side, both modelled `SubnetId` widths, a removal down
//! to each published bound and one past it, and an add of an id the endpoint already holds.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};
use crate::vpc_endpoint::{await_active, create_endpoint, ids, read_one, ACTIVE, PENDING};

/// `SecurityGroupIds`' own published maximum, from
/// `smithy_query(query="shape", shape="SecurityGroupIds")` -> `@length(min: 1, max: 5)`.
const MAX_SECURITY_GROUP_IDS: usize = 5;

/// `SubnetIds`' own published minimum, from
/// `smithy_query(query="shape", shape="SubnetIds")` -> `@length(min: 1)`.
const MIN_SUBNET_IDS: usize = 1;

/// The most subnets an endpoint may carry, and the most one request may name. `SubnetIds`
/// publishes no maximum (`smithy_query(query="shape", shape="SubnetIds")` -> `@length(min:
/// 1)` and nothing else), so this is the emulator's own bound — one subnet per Availability
/// Zone, of which an AWS Region has at most six — and not a modelled one.
const MAX_SUBNET_IDS: usize = 6;

/// Eight distinct subnets of the short modelled width, enough to sit on both sides of
/// [`MAX_SUBNET_IDS`].
const EIGHT_SUBNETS: [&str; 8] = [
    "subnet-00000001",
    "subnet-00000002",
    "subnet-00000003",
    "subnet-00000004",
    "subnet-00000005",
    "subnet-00000006",
    "subnet-00000007",
    "subnet-00000008",
];

/// Call `UpdateVpcEndpoint` with `request`, answering its status, error code and body.
async fn update_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    request: Value,
) -> (u16, String, Value) {
    call(router, "UpdateVpcEndpoint", request).await
}

/// Update the endpoint `request` addresses, asserting it was accepted, and answer its
/// `UpdateVpcEndpointDetail` — the member name the model gives that output
/// (`smithy_query(query="shape", shape="UpdateVpcEndpointResponse")`).
async fn accepted_update(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: Value,
) -> Value {
    let (status, code, answer) = update_endpoint(router, request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from UpdateVpcEndpoint, got {status} {code} with {answer}"
    );
    at(&answer, "UpdateVpcEndpointDetail").clone()
}

/// A JSON array of `entries`.
fn list(entries: &[&str]) -> Value {
    Value::Array(entries.iter().map(|entry| json!(entry)).collect())
}

/// Create an `ACTIVE` endpoint named `name` carrying `subnets` and `groups`, answering its
/// id.
async fn active_endpoint(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    name: &str,
    subnets: &[&str],
    groups: &[&str],
) -> String {
    let mut request = json!({
        "name": name,
        "vpcId": "vpc-0123456789abcdef0",
        "subnetIds": list(subnets),
    });
    if !groups.is_empty() {
        request["securityGroupIds"] = list(groups);
    }
    let detail = create_endpoint(router, case, request).await;
    let id = text(&detail, "id");
    // An endpoint may only be CHANGED once it has settled, so every row starts from the
    // status the clock leaves it in rather than from the create's own.
    await_active(router, case, &id).await;
    id
}

// ---------------------------------------------------------------------------
// The update/read round trip, over a table that spans which lists the request names, the
// counts on each side, both modelled `SubnetId` widths, and an add of an id already held.
// ---------------------------------------------------------------------------

/// One row: the endpoint the update starts from, the request's add/remove lists, and the
/// lists the endpoint is left carrying.
struct Row {
    case: &'static str,
    subnets: &'static [&'static str],
    groups: &'static [&'static str],
    add_subnets: &'static [&'static str],
    remove_subnets: &'static [&'static str],
    add_groups: &'static [&'static str],
    remove_groups: &'static [&'static str],
    expect_subnets: &'static [&'static str],
    expect_groups: &'static [&'static str],
}

#[tokio::test]
async fn an_updated_vpc_endpoint_is_what_the_batch_read_reports() {
    let router = emulator();

    // `SubnetId` is `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` and `SecurityGroupId` is
    // `^[\w+\-]+$` (smithy_query shapes `SubnetIds` / `SecurityGroupIds`). The rows differ
    // along which of the four lists the request names, the counts on each side, the subnet
    // width, whether a removal empties the security groups, and whether an id named is one
    // the endpoint already holds or does not hold at all.
    let rows: &[Row] = &[
        Row {
            case: "one subnet added to an endpoint carrying one, no groups",
            subnets: &["subnet-1a2b3c4d"],
            groups: &[],
            add_subnets: &["subnet-22222222"],
            remove_subnets: &[],
            add_groups: &[],
            remove_groups: &[],
            expect_subnets: &["subnet-1a2b3c4d", "subnet-22222222"],
            expect_groups: &[],
        },
        Row {
            case: "two long-width subnets added and one of the held ones removed",
            subnets: &["subnet-11111111", "subnet-22222222"],
            groups: &["sg-held"],
            add_subnets: &["subnet-0123456789abcdef0", "subnet-abcdef01234567890"],
            remove_subnets: &["subnet-11111111"],
            add_groups: &[],
            remove_groups: &[],
            expect_subnets: &[
                "subnet-22222222",
                "subnet-0123456789abcdef0",
                "subnet-abcdef01234567890",
            ],
            expect_groups: &["sg-held"],
        },
        Row {
            case: "security groups added up to the published maximum of five",
            subnets: &["subnet-1a2b3c4d"],
            groups: &[],
            add_subnets: &[],
            remove_subnets: &[],
            add_groups: &["sg-1", "sg-2", "sg-3", "sg-4", "sg+5"],
            remove_groups: &[],
            expect_subnets: &["subnet-1a2b3c4d"],
            expect_groups: &["sg-1", "sg-2", "sg-3", "sg-4", "sg+5"],
        },
        Row {
            case: "every security group removed, leaving none",
            subnets: &["subnet-1a2b3c4d"],
            groups: &["sg-one", "sg-two"],
            add_subnets: &[],
            remove_subnets: &[],
            add_groups: &[],
            remove_groups: &["sg-one", "sg-two"],
            expect_subnets: &["subnet-1a2b3c4d"],
            expect_groups: &[],
        },
        Row {
            case: "an id already held added and one never held removed",
            subnets: &["subnet-cafebabe"],
            groups: &["sg-held"],
            add_subnets: &["subnet-cafebabe"],
            remove_subnets: &["subnet-deadbeef"],
            add_groups: &["sg-held"],
            remove_groups: &["sg-absent"],
            expect_subnets: &["subnet-cafebabe"],
            expect_groups: &["sg-held"],
        },
        Row {
            case: "all four lists in one request",
            subnets: &["subnet-11111111", "subnet-33333333"],
            groups: &["sg-old", "sg-keep"],
            add_subnets: &["subnet-44444444"],
            remove_subnets: &["subnet-33333333"],
            add_groups: &["sg-new"],
            remove_groups: &["sg-old"],
            expect_subnets: &["subnet-11111111", "subnet-44444444"],
            expect_groups: &["sg-keep", "sg-new"],
        },
        Row {
            case: "no list at all, which changes no membership",
            subnets: &["subnet-55555555"],
            groups: &["sg-keep"],
            add_subnets: &[],
            remove_subnets: &[],
            add_groups: &[],
            remove_groups: &[],
            expect_subnets: &["subnet-55555555"],
            expect_groups: &["sg-keep"],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("updated-endpoint-{index}");
        let id = active_endpoint(&router, case, &name, row.subnets, row.groups).await;
        let before = read_one(&router, case, &id).await;
        let created_date = at(&before, "createdDate").as_i64().unwrap_or_default();

        let mut request = json!({ "id": id });
        for (member, entries) in [
            ("addSubnetIds", row.add_subnets),
            ("removeSubnetIds", row.remove_subnets),
            ("addSecurityGroupIds", row.add_groups),
            ("removeSecurityGroupIds", row.remove_groups),
        ] {
            // A list the row leaves empty is a member the request does NOT carry: each one
            // is `@length(min: 1)`, so an empty list is outside the model.
            if !entries.is_empty() {
                request[member] = list(entries);
            }
        }
        let detail = accepted_update(&router, case, request).await;

        assert_eq!(
            text(&detail, "id"),
            id,
            "case {case}: expected the update to report the endpoint it addressed, got {detail}"
        );
        assert_eq!(
            text(&detail, "name"),
            name,
            "case {case}: expected the stored name, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            PENDING,
            "case {case}: expected the non-terminal {PENDING} the change is applied in, got {detail}"
        );
        assert_eq!(
            ids(&detail, "subnetIds"),
            row.expect_subnets,
            "case {case}: expected the subnets the update leaves, got {detail}"
        );
        // `SecurityGroupIds` is `@length(min: 1)` on the wire, so an endpoint left carrying
        // none reports the member as absent rather than as an empty list.
        if row.expect_groups.is_empty() {
            assert_eq!(
                at(&detail, "securityGroupIds"),
                &Value::Null,
                "case {case}: expected no securityGroupIds, got {detail}"
            );
        } else {
            assert_eq!(
                ids(&detail, "securityGroupIds"),
                row.expect_groups,
                "case {case}: expected the security groups the update leaves, got {detail}"
            );
        }
        let last_modified = at(&detail, "lastModifiedDate").as_i64().unwrap_or_default();
        assert!(
            last_modified >= created_date && created_date > 0,
            "case {case}: expected an epoch-millisecond lastModifiedDate no earlier than the \
             createdDate {created_date}, got {detail}"
        );

        // The read straight after the update still observes the transient status, so the
        // span the endpoint settles over is never zero.
        let pending = read_one(&router, case, &id).await;
        assert_eq!(
            text(&pending, "status"),
            PENDING,
            "case {case}: expected the read straight after the update to report {PENDING}, got {pending}"
        );

        // The endpoint settles back on its own, and the membership the update left is what
        // the read reports — nothing else about the endpoint moved.
        let active = await_active(&router, case, &id).await;
        assert_eq!(
            text(&active, "status"),
            ACTIVE,
            "case {case}: expected the endpoint to settle into {ACTIVE}, got {active}"
        );
        assert_eq!(
            ids(&active, "subnetIds"),
            row.expect_subnets,
            "case {case}: expected the read to report the subnets the update left, got {active}"
        );
        if row.expect_groups.is_empty() {
            assert_eq!(
                at(&active, "securityGroupIds"),
                &Value::Null,
                "case {case}: expected the read to report no securityGroupIds, got {active}"
            );
        } else {
            assert_eq!(
                ids(&active, "securityGroupIds"),
                row.expect_groups,
                "case {case}: expected the read to report the security groups the update left, got {active}"
            );
        }
        assert_eq!(
            text(&active, "vpcId"),
            text(&before, "vpcId"),
            "case {case}: expected the update to leave the vpcId alone, got {active}"
        );
        assert_eq!(
            at(&active, "createdDate").as_i64().unwrap_or_default(),
            created_date,
            "case {case}: expected a stable createdDate across the update, got {active}"
        );
    }
}

// ---------------------------------------------------------------------------
// A second update applied on top of the first, so the membership is the store's and not one
// request's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn successive_updates_each_apply_to_what_the_last_one_left() {
    let router = emulator();

    let id = active_endpoint(
        &router,
        "the endpoint",
        "successive-updates",
        &["subnet-11111111"],
        &[],
    )
    .await;

    // Each step names the lists it changes and the membership the endpoint is left with.
    let steps: &[(&str, Value, &[&str], &[&str])] = &[
        (
            "the first update adds a subnet and a group",
            json!({ "addSubnetIds": ["subnet-22222222"], "addSecurityGroupIds": ["sg-one"] }),
            &["subnet-11111111", "subnet-22222222"],
            &["sg-one"],
        ),
        (
            "the second update removes the subnet the first added",
            json!({ "removeSubnetIds": ["subnet-22222222"] }),
            &["subnet-11111111"],
            &["sg-one"],
        ),
        (
            "the third update swaps the group",
            json!({ "addSecurityGroupIds": ["sg-two"], "removeSecurityGroupIds": ["sg-one"] }),
            &["subnet-11111111"],
            &["sg-two"],
        ),
    ];

    let mut previous_modified = 0;
    for (case, change, expect_subnets, expect_groups) in steps {
        let mut request = change.clone();
        request["id"] = json!(id);
        let detail = accepted_update(&router, case, request).await;
        assert_eq!(
            ids(&detail, "subnetIds"),
            *expect_subnets,
            "case {case}: expected the subnets this step leaves, got {detail}"
        );
        assert_eq!(
            ids(&detail, "securityGroupIds"),
            *expect_groups,
            "case {case}: expected the security groups this step leaves, got {detail}"
        );
        let last_modified = at(&detail, "lastModifiedDate").as_i64().unwrap_or_default();
        assert!(
            last_modified >= previous_modified,
            "case {case}: expected a lastModifiedDate no earlier than the previous update's \
             {previous_modified}, got {detail}"
        );
        previous_modified = last_modified;

        let active = await_active(&router, case, &id).await;
        assert_eq!(
            ids(&active, "subnetIds"),
            *expect_subnets,
            "case {case}: expected the read to report the subnets this step left, got {active}"
        );
        assert_eq!(
            ids(&active, "securityGroupIds"),
            *expect_groups,
            "case {case}: expected the read to report the groups this step left, got {active}"
        );
    }
}

// ---------------------------------------------------------------------------
// The requests an update refuses: a result outside the modelled bounds, an id naming no
// endpoint, a contradictory request, and the modelled constraints on the input itself.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_outside_the_rule_is_refused_and_stores_nothing() {
    let router = emulator();

    let id = active_endpoint(
        &router,
        "the endpoint",
        "refused-updates",
        &["subnet-11111111", "subnet-22222222"],
        &["sg-one", "sg-two", "sg-three"],
    )
    .await;
    let before = read_one(&router, "the endpoint", &id).await;

    // Well-formed `VpcEndpointId`s that name no stored endpoint: one never minted, and one
    // whose suffix is empty — both satisfy `^vpce-[0-9a-z]*$`, so they reach the handler.
    // `UpdateVpcEndpoint` models NO not-found error, so the endpoint's absence is reported
    // as the state conflict it is, naming the id the request addressed.
    for absent in ["vpce-00000000000000000", "vpce-"] {
        let (status, code, answer) = update_endpoint(
            &router,
            json!({ "id": absent, "addSubnetIds": ["subnet-33333333"] }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case an id naming no endpoint ({absent}): expected {CLIENT_ERROR} \
             ConflictException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            format!("The VpcEndpoint with Id '{absent}' is not exist."),
            "case an id naming no endpoint ({absent}): expected the refusal to name the id, \
             got {answer}"
        );
    }

    // The rows below sit one past a published bound of the RESULT, contradict themselves, or
    // break a modelled constraint on the input itself.
    let cases: Vec<(String, Value)> = vec![
        (
            format!("a removal leaving fewer than {MIN_SUBNET_IDS} subnet"),
            json!({ "id": id, "removeSubnetIds": ["subnet-11111111", "subnet-22222222"] }),
        ),
        (
            format!("a result one group over the maximum of {MAX_SECURITY_GROUP_IDS}"),
            json!({ "id": id, "addSecurityGroupIds": ["sg-4", "sg-5", "sg-6"] }),
        ),
        (
            "the same subnet added and removed".to_string(),
            json!({
                "id": id,
                "addSubnetIds": ["subnet-33333333"],
                "removeSubnetIds": ["subnet-33333333"],
            }),
        ),
        (
            "the same security group added and removed".to_string(),
            json!({
                "id": id,
                "addSecurityGroupIds": ["sg-four"],
                "removeSecurityGroupIds": ["sg-four"],
            }),
        ),
        (
            "no id at all".to_string(),
            json!({ "addSubnetIds": ["subnet-33333333"] }),
        ),
        (
            "an id outside the modelled pattern".to_string(),
            json!({ "id": "vpce_11111111", "addSubnetIds": ["subnet-33333333"] }),
        ),
        (
            "an added subnet of neither modelled width".to_string(),
            json!({ "id": id, "addSubnetIds": ["subnet-123"] }),
        ),
        (
            format!("one list one entry under the minimum of {MIN_SUBNET_IDS}"),
            json!({ "id": id, "addSubnetIds": [] }),
        ),
        (
            format!("one list one entry over the maximum of {MAX_SECURITY_GROUP_IDS}"),
            json!({ "id": id, "addSecurityGroupIds": ["sg-1", "sg-2", "sg-3", "sg-4", "sg-5", "sg-6"] }),
        ),
    ];

    for (case, request) in cases {
        let (status, code, answer) = update_endpoint(&router, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // Every refusal above wrote nothing: the endpoint still carries what it was created
    // with, and it never left the status it was in.
    let after = read_one(&router, "after every refusal", &id).await;
    assert_eq!(
        ids(&after, "subnetIds"),
        ids(&before, "subnetIds"),
        "case after every refusal: expected the stored subnets to be untouched, got {after}"
    );
    assert_eq!(
        ids(&after, "securityGroupIds"),
        ids(&before, "securityGroupIds"),
        "case after every refusal: expected the stored security groups to be untouched, got {after}"
    );
    assert_eq!(
        text(&after, "status"),
        ACTIVE,
        "case after every refusal: expected the endpoint to still be {ACTIVE}, got {after}"
    );
}

// ---------------------------------------------------------------------------
// The bound that keeps a subnet list finite — the one `SubnetIds` itself publishes no
// maximum for: at it, one entry past it in the result, one entry past it in the request's
// own list, and one past it on the create that stores the endpoint.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_subnet_list_is_held_to_the_bound_that_keeps_it_finite() {
    let router = emulator();

    // One short of the bound, grown to it: accepted.
    let held = &EIGHT_SUBNETS[..MAX_SUBNET_IDS - 1];
    let id = active_endpoint(&router, "at the bound", "subnet-bound", held, &[]).await;
    let added = EIGHT_SUBNETS[MAX_SUBNET_IDS - 1];
    let detail = accepted_update(
        &router,
        "at the bound",
        json!({ "id": id, "addSubnetIds": [added] }),
    )
    .await;
    assert_eq!(
        ids(&detail, "subnetIds").len(),
        MAX_SUBNET_IDS,
        "case at the bound: expected the endpoint to carry {MAX_SUBNET_IDS} subnets, got {detail}"
    );
    await_active(&router, "at the bound", &id).await;

    // One past the bound, in the RESULT and in the request's own list, and one past it on
    // the create that would store such an endpoint in the first place.
    let one_past_in_request: Vec<&str> = EIGHT_SUBNETS[..=MAX_SUBNET_IDS].to_vec();
    let cases: Vec<(String, &str, Value)> = vec![
        (
            format!("a result one subnet over the bound of {MAX_SUBNET_IDS}"),
            "UpdateVpcEndpoint",
            json!({ "id": id, "addSubnetIds": [EIGHT_SUBNETS[MAX_SUBNET_IDS]] }),
        ),
        (
            format!("an add list one entry over the bound of {MAX_SUBNET_IDS}"),
            "UpdateVpcEndpoint",
            json!({ "id": id, "addSubnetIds": list(&one_past_in_request) }),
        ),
        (
            format!("a remove list one entry over the bound of {MAX_SUBNET_IDS}"),
            "UpdateVpcEndpoint",
            json!({ "id": id, "removeSubnetIds": list(&one_past_in_request) }),
        ),
        (
            format!("a create naming one subnet over the bound of {MAX_SUBNET_IDS}"),
            "CreateVpcEndpoint",
            json!({
                "name": "subnet-bound-create",
                "vpcId": "vpc-0123456789abcdef0",
                "subnetIds": list(&one_past_in_request),
            }),
        ),
    ];
    for (case, operation, request) in cases {
        let (status, code, answer) = call(&router, operation, request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {case}: expected {CLIENT_ERROR} ValidationException from {operation}, got {status} {code} with {answer}"
        );
    }

    // A refusal a handler makes is worded exactly as the identical one the boundary
    // validator makes — the sentence the suites beside this one pin for a modelled
    // constraint failure (see `client_error_status.rs` for a scalar member and
    // `reported_pattern.rs` for a list) — naming the member, the value the request carried
    // as that layer renders it, and the constraint it failed. For a LIST that value is the
    // item count, not the entries.
    let (_, _, answer) = update_endpoint(
        &router,
        json!({ "id": id, "addSubnetIds": list(&one_past_in_request) }),
    )
    .await;
    assert_eq!(
        text(&answer, "message"),
        format!(
            "1 validation error detected: Value '{} items' at 'addSubnetIds' failed to \
             satisfy constraint: Member must have length less than or equal to \
             {MAX_SUBNET_IDS}",
            one_past_in_request.len()
        ),
        "case the wording of a list-length refusal: expected the validation layer's own sentence, got {answer}"
    );
    let overlapping = EIGHT_SUBNETS[0];
    let (_, _, answer) = update_endpoint(
        &router,
        json!({
            "id": id,
            "addSubnetIds": [overlapping],
            "removeSubnetIds": [overlapping],
        }),
    )
    .await;
    assert_eq!(
        text(&answer, "message"),
        format!(
            "1 validation error detected: Value '{overlapping}' at 'addSubnetIds' failed to \
             satisfy constraint: Member must not also be named in 'removeSubnetIds'"
        ),
        "case the wording of a contradictory refusal: expected the member, the id the request carried and the constraint, got {answer}"
    );

    // Nothing above was stored: the endpoint still carries exactly the bound.
    let active = read_one(&router, "after the refusals", &id).await;
    assert_eq!(
        ids(&active, "subnetIds").len(),
        MAX_SUBNET_IDS,
        "case after the refusals: expected the refusals to store nothing, got {active}"
    );
    assert_eq!(
        text(&active, "status"),
        ACTIVE,
        "case after the refusals: expected the endpoint to still be {ACTIVE}, got {active}"
    );
}

// ---------------------------------------------------------------------------
// An endpoint in a status it may not be changed from.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_of_an_endpoint_in_a_non_terminal_status_is_a_conflict() {
    let router = emulator();

    // Straight after the create the endpoint is still being brought up, and straight after
    // an accepted update it is applying that change: both are non-terminal statuses, and
    // both refuse the next change until the clock settles them.
    let created = create_endpoint(
        &router,
        "the create",
        json!({
            "name": "conflicting-update",
            "vpcId": "vpc-0123456789abcdef0",
            "subnetIds": ["subnet-11111111"],
        }),
    )
    .await;
    let id = text(&created, "id");

    let cases: &[&str] = &[
        "while the endpoint is still being brought up",
        "while an accepted update is being applied",
    ];
    for (index, case) in cases.iter().enumerate() {
        if index == 1 {
            // Settle the create, then accept ONE update so the next one lands on an
            // endpoint in the status that change is applied in.
            await_active(&router, case, &id).await;
            accepted_update(
                &router,
                case,
                json!({ "id": id, "addSubnetIds": ["subnet-22222222"] }),
            )
            .await;
        }
        let (status, code, answer) =
            update_endpoint(&router, json!({ "id": id, "addSubnetIds": ["subnet-33333333"] }))
                .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case {case}: expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
        assert!(
            text(&answer, "message").contains("cannot be updated"),
            "case {case}: expected the refusal to say the endpoint cannot be updated, got {answer}"
        );
    }

    // The refused changes stored nothing: only the one accepted update is in the endpoint.
    let active = await_active(&router, "after the conflicts", &id).await;
    assert_eq!(
        ids(&active, "subnetIds"),
        vec!["subnet-11111111".to_string(), "subnet-22222222".to_string()],
        "case after the conflicts: expected only the accepted update to be stored, got {active}"
    );
}

// ---------------------------------------------------------------------------
// The `@idempotencyToken` replay: a spent token answers the record its first call left.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_answers_the_record_its_first_call_left() {
    let router = emulator();

    let id = active_endpoint(
        &router,
        "the endpoint",
        "token-update",
        &["subnet-11111111"],
        &[],
    )
    .await;

    let request = json!({
        "id": id,
        "addSubnetIds": ["subnet-22222222"],
        "clientToken": "an-update-token",
    });
    let first = accepted_update(&router, "the first call", request.clone()).await;
    let expected = vec![
        "subnet-11111111".to_string(),
        "subnet-22222222".to_string(),
    ];
    assert_eq!(
        ids(&first, "subnetIds"),
        expected,
        "case the first call: expected the subnet it added, got {first}"
    );

    // The retry answers the endpoint the first call left, whatever change it carries: a
    // replay is the same request arriving twice, not a second change.
    let cases: Vec<(&str, Value)> = vec![
        ("the identical retry", request.clone()),
        (
            "the token carrying another change",
            json!({
                "id": id,
                "removeSubnetIds": ["subnet-22222222"],
                "addSecurityGroupIds": ["sg-one"],
                "clientToken": "an-update-token",
            }),
        ),
    ];
    for (case, retry) in cases {
        let detail = accepted_update(&router, case, retry).await;
        assert_eq!(
            ids(&detail, "subnetIds"),
            expected,
            "case {case}: expected the membership the first call left, got {detail}"
        );
        assert_eq!(
            at(&detail, "securityGroupIds"),
            &Value::Null,
            "case {case}: expected the replay to have added no security group, got {detail}"
        );
    }

    // The store agrees with the replays, and a FRESH token on the same endpoint is a real
    // change rather than another replay.
    let active = await_active(&router, "after the replays", &id).await;
    assert_eq!(
        ids(&active, "subnetIds"),
        expected,
        "case after the replays: expected the first call's membership only, got {active}"
    );
    let detail = accepted_update(
        &router,
        "a fresh token",
        json!({
            "id": id,
            "addSecurityGroupIds": ["sg-one"],
            "clientToken": "another-update-token",
        }),
    )
    .await;
    assert_eq!(
        ids(&detail, "securityGroupIds"),
        vec!["sg-one".to_string()],
        "case a fresh token: expected the change it carries to be applied, got {detail}"
    );
}
