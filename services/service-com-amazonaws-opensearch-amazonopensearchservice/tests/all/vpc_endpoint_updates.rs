// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateVpcEndpoint`: what a modification of a service-managed
//! VPC endpoint applies, and what a second writer gets while the first has not landed.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the create, read and domain helpers from [`super::vpc_endpoints`]; this suite adds
//! only the `UpdateVpcEndpoint` call itself and the two preconditions it needs (an
//! endpoint that is idle, and one whose create has not landed).
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an update applies the members its `VpcOptions` CARRIES and leaves the endpoint's
//!   other options as they were ("the security groups and/or subnets to add, remove, or
//!   modify"), over a table of six updates differing in which members they name, how
//!   many values each names, and whether the value is new;
//! * the endpoint rests in `UPDATING` the instant the call returns and the clock owes it
//!   `ACTIVE` once the span has passed, with no call having asked for the move;
//! * a change that arrives while another change to the SAME endpoint has not landed yet
//!   is refused with the modelled `ConflictException` (HTTP 409) stating the change
//!   already in progress — whether that is an update or the create that minted the
//!   endpoint — so two racing writers settle on exactly one winner's options rather than
//!   a merge, and the refused request succeeds unchanged once the endpoint is idle again;
//! * an id that names no endpoint of this account is refused with the modelled
//!   `ResourceNotFoundException`, and nothing is changed.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ENDPOINT_INACCESSIBLE, VPC_ENDPOINT_CHANGE_CREATE, VPC_ENDPOINT_CHANGE_UPDATE,
    VPC_ENDPOINT_STATUS_ACTIVE, VPC_ENDPOINT_STATUS_CREATING, VPC_ENDPOINT_STATUS_UPDATING,
};

use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::vpc_endpoints::{create_endpoint, describe_endpoints, member, minted, vpc_domain};

/// The sentence the service refuses a change with while another change to the SAME
/// endpoint has not landed: fixed text stating the change already under way, and no
/// value of the request beside it. Spelled out here rather than taken from the crate, so
/// that a change to either part fails this suite.
fn conflict_message(change_in_flight: &str) -> String {
    format!("A {change_in_flight} operation is already in progress for this VPC endpoint.")
}

/// `UpdateVpcEndpoint`: `POST /2021-01-01/opensearch/vpcEndpoints/update`.
///
/// `pub(crate)` because the sibling suite for what status the per-domain listing reports
/// drives a modification through THIS wrapper rather than growing a second copy of it.
pub(crate) async fn update_endpoint(emulator: &Emulator, id: &str, vpc_options: &Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/vpcEndpoints/update"),
            Some(json!({ "VpcEndpointId": id, "VpcOptions": vpc_options })),
        )
        .await
}

/// `DescribeVpcEndpoints` for the read-back of ONE endpoint, through the suite's shared
/// read: the endpoint it reports, with the call asserted to have been served.
async fn read_endpoint(emulator: &Emulator, id: &str) -> Value {
    let answer = describe_endpoints(emulator, &[id]).await;
    assert_eq!(
        answer.status, 200,
        "reading endpoint {id:?}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["VpcEndpoints"][0].clone()
}

/// The subnets and security groups every endpoint in this suite is created with — what
/// an update that names only one of the two members must leave standing.
fn created_with() -> Value {
    json!({
        "SubnetIds": ["subnet-0aaa0000000000001"],
        "SecurityGroupIds": ["sg-0aaa0000000000001"],
    })
}

/// A VPC-provisioned domain and one `ACTIVE` endpoint on it, answering with the
/// endpoint's id: the state an update acts on.
async fn active_endpoint(emulator: &Emulator, domain: &str) -> String {
    let id = creating_endpoint(emulator, domain).await;
    // The create's own span falls due on its own; waiting for it is what leaves the
    // endpoint idle, which is the precondition an update is served from.
    tokio::time::sleep(SETTLE_WAIT).await;
    let read = read_endpoint(emulator, &id).await;
    assert_eq!(
        read["Status"],
        json!(VPC_ENDPOINT_STATUS_ACTIVE),
        "endpoint {id:?} on {domain:?}: expected it to settle ACTIVE before the update, got \
         {read:?}"
    );
    id
}

/// A VPC-provisioned domain and one endpoint on it whose CREATE has not landed yet,
/// answering with the endpoint's id.
async fn creating_endpoint(emulator: &Emulator, domain: &str) -> String {
    let arn = vpc_domain(emulator, domain).await;
    let case = "an endpoint created for an update";
    let answer = create_endpoint(
        emulator,
        json!({ "DomainArn": arn, "VpcOptions": created_with() }),
    )
    .await;
    member(&minted(&answer, case), "VpcEndpointId", case)
}

/// One update to apply: how its `VpcOptions` differ from the other rows'.
struct Row {
    case: &'static str,
    domain: &'static str,
    /// The `VpcOptions` the update carries.
    vpc_options: Value,
}

/// The update table: one row per way an accepted update's `VpcOptions` differ — which
/// of the two members it names, how many values it names, and whether the value it
/// names is one the endpoint already carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a different security group",
            domain: "endpoint-upd-group",
            vpc_options: json!({
                "SubnetIds": ["subnet-0aaa0000000000001"],
                "SecurityGroupIds": ["sg-0bbb0000000000002"],
            }),
        },
        Row {
            case: "one subnet replaced by two, one per availability zone",
            domain: "endpoint-upd-subnets",
            vpc_options: json!({
                "SubnetIds": ["subnet-0ccc0000000000001", "subnet-0ccc0000000000002"],
                "SecurityGroupIds": ["sg-0aaa0000000000001"],
            }),
        },
        Row {
            case: "several security groups at once",
            domain: "endpoint-upd-many-groups",
            vpc_options: json!({
                "SecurityGroupIds": [
                    "sg-0ddd0000000000001",
                    "sg-0ddd0000000000002",
                    "sg-0ddd0000000000003",
                ],
            }),
        },
        Row {
            case: "only subnets, so the security groups are untouched",
            domain: "endpoint-upd-only-subnets",
            vpc_options: json!({ "SubnetIds": ["subnet-0eee0000000000001"] }),
        },
        Row {
            case: "only security groups, so the subnets are untouched",
            domain: "endpoint-upd-only-groups",
            vpc_options: json!({ "SecurityGroupIds": ["sg-0fff0000000000001"] }),
        },
        Row {
            case: "egress routed through the customer VPC",
            domain: "endpoint-upd-egress",
            vpc_options: json!({
                "SubnetIds": ["subnet-0aaa0000000000001"],
                "SecurityGroupIds": ["sg-0aaa0000000000001"],
                "EgressEnabled": true,
            }),
        },
    ]
}

/// The options an endpoint created with [`created_with`] carries once `update` has been
/// applied to it: every member the update NAMES, and the created value for every member
/// it does not.
fn expected_options(update: &Value) -> Value {
    let created = created_with();
    let mut expected = json!({});
    for member in ["SubnetIds", "SecurityGroupIds", "EgressEnabled"] {
        let value = if update.get(member).is_some() {
            update[member].clone()
        } else {
            created.get(member).cloned().unwrap_or(Value::Null)
        };
        if !value.is_null() {
            expected[member] = value;
        }
    }
    expected
}

/// Every member of `expected` as the endpoint reports it, with the case named on
/// failure. Compared member by member, because the reported `VPCDerivedInfo` also
/// carries the members EC2 derives from the subnets, which this emulator does not model.
fn assert_options(endpoint: &Value, expected: &Value, case: &str) {
    let reported = &endpoint["VpcOptions"];
    let members = expected
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: the expected options are a set of members"));
    for (member, value) in members {
        assert_eq!(
            reported[member], *value,
            "case {case}: expected {member} to read {value:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_update_applies_the_options_it_names_and_leaves_the_rest_standing() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let id = active_endpoint(&emulator, row.domain).await;
        let expected = expected_options(&row.vpc_options);

        let answer = update_endpoint(&emulator, &id, &row.vpc_options).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let updated = answer.body["VpcEndpoint"].clone();
        assert_eq!(
            updated["VpcEndpointId"],
            json!(id),
            "case {case}: the answer names the endpoint the request addressed, got {updated:?}"
        );
        assert_eq!(
            updated["Status"],
            json!(VPC_ENDPOINT_STATUS_UPDATING),
            "case {case}: the endpoint is being modified the instant the call returns, got \
             {updated:?}"
        );
        assert_options(&updated, &expected, case);

        // The read-back reports the SAME record the update answered with, still in its
        // transient status: the call persisted the change rather than echoing it.
        let read = read_endpoint(&emulator, &id).await;
        assert_eq!(
            read, updated,
            "case {case}: the read reports the record the update persisted, got {read:?}"
        );

        // Nothing below asks for the move: the span the update was recorded with falls
        // due on its own, and the next read materialises it.
        tokio::time::sleep(SETTLE_WAIT).await;
        let settled = read_endpoint(&emulator, &id).await;
        assert_eq!(
            settled["Status"],
            json!(VPC_ENDPOINT_STATUS_ACTIVE),
            "case {case}: the status the clock owed is in place once the span passed, got \
             {settled:?}"
        );
        assert_options(&settled, &expected, case);
    }
}

#[tokio::test]
async fn a_change_arriving_while_another_is_in_flight_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let id = active_endpoint(&emulator, row.domain).await;
        let expected = expected_options(&row.vpc_options);

        // The winner: its change is the one in flight for the rest of the span.
        let won = update_endpoint(&emulator, &id, &row.vpc_options).await;
        assert_eq!(
            won.status, 200,
            "case {case}: expected the first writer to win with 200, got {} with {:?}",
            won.status, won.body
        );

        // The loser arrives while that change has not landed: refused, with the modelled
        // conflict naming the endpoint the two are contending over.
        let loser_options = json!({ "SecurityGroupIds": ["sg-0999000000000000a"] });
        let lost = update_endpoint(&emulator, &id, &loser_options).await;
        assert_eq!(
            lost.status, 409,
            "case {case}: expected the second writer to be refused with 409, got {} with {:?}",
            lost.status, lost.body
        );
        assert_eq!(
            error_code(&lost),
            "ConflictException",
            "case {case}: expected ConflictException, got {:?}",
            lost.body
        );
        assert_eq!(
            lost.body["message"].as_str(),
            Some(conflict_message(VPC_ENDPOINT_CHANGE_UPDATE).as_str()),
            "case {case}: the refusal states the update already in progress, got {:?}",
            lost.body
        );

        // The refused request changed nothing: the endpoint still carries the winner's
        // options, never a merge of both.
        let read = read_endpoint(&emulator, &id).await;
        assert_options(&read, &expected, case);
        assert_ne!(
            read["VpcOptions"]["SecurityGroupIds"], loser_options["SecurityGroupIds"],
            "case {case}: the loser's security groups are not on the endpoint, got {read:?}"
        );

        // Once the winner's change has landed the endpoint is idle, and the loser's
        // identical request is served.
        tokio::time::sleep(SETTLE_WAIT).await;
        let retried = update_endpoint(&emulator, &id, &loser_options).await;
        assert_eq!(
            retried.status, 200,
            "case {case}: expected the refused request to succeed once the endpoint is idle, got \
             {} with {:?}",
            retried.status, retried.body
        );
        assert_eq!(
            retried.body["VpcEndpoint"]["VpcOptions"]["SecurityGroupIds"],
            loser_options["SecurityGroupIds"],
            "case {case}: the retried request applies its own options, got {:?}",
            retried.body
        );
    }
}

#[tokio::test]
async fn a_change_to_an_endpoint_whose_create_has_not_landed_is_refused() {
    let emulator = Emulator::new();
    let id = creating_endpoint(&emulator, "endpoint-upd-creating").await;

    let read = read_endpoint(&emulator, &id).await;
    assert_eq!(
        read["Status"],
        json!(VPC_ENDPOINT_STATUS_CREATING),
        "case an endpoint still being provisioned: expected CREATING, got {read:?}"
    );

    let answer = update_endpoint(
        &emulator,
        &id,
        &json!({ "SecurityGroupIds": ["sg-0888000000000000a"] }),
    )
    .await;
    assert_eq!(
        answer.status, 409,
        "case an endpoint still being provisioned: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ConflictException",
        "case an endpoint still being provisioned: expected ConflictException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(conflict_message(VPC_ENDPOINT_CHANGE_CREATE).as_str()),
        "case an endpoint still being provisioned: the refusal states the create already in \
         progress, got {:?}",
        answer.body
    );
    let case = "an endpoint still being provisioned";
    assert_options(&read_endpoint(&emulator, &id).await, &created_with(), case);

    // The create's own span falls due, and the same request is then served.
    tokio::time::sleep(SETTLE_WAIT).await;
    let retried = update_endpoint(
        &emulator,
        &id,
        &json!({ "SecurityGroupIds": ["sg-0888000000000000a"] }),
    )
    .await;
    assert_eq!(
        retried.status, 200,
        "case an endpoint whose create has landed: expected 200, got {} with {:?}",
        retried.status, retried.body
    );
}

#[tokio::test]
async fn two_racing_writers_settle_on_exactly_one_winner() {
    let emulator = Emulator::new();
    let id = active_endpoint(&emulator, "endpoint-upd-racing").await;

    let first = json!({ "SecurityGroupIds": ["sg-0aaa0000000000011"] });
    let second = json!({ "SecurityGroupIds": ["sg-0aaa0000000000022"] });
    let (a, b) = tokio::join!(
        update_endpoint(&emulator, &id, &first),
        update_endpoint(&emulator, &id, &second),
    );

    let statuses = [a.status, b.status];
    let winners = statuses.iter().filter(|status| **status == 200).count();
    let losers = statuses.iter().filter(|status| **status == 409).count();
    assert_eq!(
        winners, 1,
        "case two concurrent updates: expected exactly one winner, got statuses {statuses:?} \
         with {:?} and {:?}",
        a.body, b.body
    );
    assert_eq!(
        losers, 1,
        "case two concurrent updates: expected exactly one loser refused with 409, got statuses \
         {statuses:?} with {:?} and {:?}",
        a.body, b.body
    );
    let refused = if a.status == 409 { &a } else { &b };
    assert_eq!(
        error_code(refused),
        "ConflictException",
        "case two concurrent updates: the loser is refused with the modelled conflict, got {:?}",
        refused.body
    );

    // The endpoint settles with the WINNER's security groups, and never a merge of both.
    let winner = if a.status == 200 { &first } else { &second };
    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = read_endpoint(&emulator, &id).await;
    assert_eq!(
        settled["Status"],
        json!(VPC_ENDPOINT_STATUS_ACTIVE),
        "case two concurrent updates: the endpoint settles once the span passed, got {settled:?}"
    );
    assert_eq!(
        settled["VpcOptions"]["SecurityGroupIds"], winner["SecurityGroupIds"],
        "case two concurrent updates: the endpoint carries exactly the winner's security groups, \
         got {settled:?}"
    );
}

#[tokio::test]
async fn the_conflict_refusal_states_the_change_already_in_progress() {
    let emulator = Emulator::new();

    // One row per change an endpoint can have in flight when the next writer arrives,
    // and per way that writer differs: the create that minted the endpoint, and updates
    // of each of the two members `VpcOptions` carries. The refused request itself is
    // varied too, so the sentence cannot be coming from what it asked for.
    let rows: Vec<(&str, Option<Value>, Value, &str)> = vec![
        (
            "the create that minted the endpoint has not landed",
            None,
            json!({ "SecurityGroupIds": ["sg-0111000000000000a"] }),
            VPC_ENDPOINT_CHANGE_CREATE,
        ),
        (
            "an update of the subnets has not landed",
            Some(json!({ "SubnetIds": ["subnet-0222000000000001"] })),
            json!({ "SubnetIds": ["subnet-0222000000000002"] }),
            VPC_ENDPOINT_CHANGE_UPDATE,
        ),
        (
            "an update of the security groups has not landed",
            Some(json!({ "SecurityGroupIds": ["sg-0333000000000001"] })),
            json!({
                "SubnetIds": ["subnet-0333000000000001"],
                "SecurityGroupIds": ["sg-0333000000000002"],
                "EgressEnabled": false,
            }),
            VPC_ENDPOINT_CHANGE_UPDATE,
        ),
    ];

    for (index, (case, in_flight, refused, change)) in rows.into_iter().enumerate() {
        let domain = format!("endpoint-conflict-word-{index}");
        let id = match &in_flight {
            // Nothing waits for the create: it is the change in flight.
            None => creating_endpoint(&emulator, &domain).await,
            Some(options) => {
                let id = active_endpoint(&emulator, &domain).await;
                let won = update_endpoint(&emulator, &id, options).await;
                assert_eq!(
                    won.status, 200,
                    "case {case}: expected the first writer to win with 200, got {} with {:?}",
                    won.status, won.body
                );
                id
            }
        };

        let answer = update_endpoint(&emulator, &id, &refused).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {case}: expected ConflictException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(conflict_message(change).as_str()),
            "case {case}: expected the refusal to state the {change} already in progress, got \
             {:?}",
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default().to_owned();
        assert!(
            !message.contains(&id),
            "case {case}: the refusal carries no value of the request, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_id_that_names_no_endpoint_is_refused_and_nothing_is_changed() {
    let emulator = Emulator::new();
    let id = active_endpoint(&emulator, "endpoint-upd-missing").await;

    // One row per way an id can name no endpoint of this account: a well-formed id
    // nothing minted, the shortest id the pattern admits, and one that was minted and
    // then released.
    let released = active_endpoint(&emulator, "endpoint-upd-released").await;
    let gone = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/vpcEndpoints/{released}"),
            None,
        )
        .await;
    assert_eq!(
        gone.status, 200,
        "case DeleteVpcEndpoint({released:?}): expected 200, got {} with {:?}",
        gone.status, gone.body
    );

    let rows: Vec<(&str, String)> = vec![
        ("an id nothing minted", "aos-0123456789abcdef".to_owned()),
        ("the shortest id the pattern admits", "aos-0".to_owned()),
        ("an endpoint that was released", released),
    ];

    for (case, missing) in rows {
        let answer = update_endpoint(
            &emulator,
            &missing,
            &json!({ "SecurityGroupIds": ["sg-0777000000000000a"] }),
        )
        .await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case:?}: expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(ENDPOINT_INACCESSIBLE),
            "case {case:?}: expected the refusal a call addressing one endpoint states, got {:?}",
            answer.body
        );
    }

    // The endpoint that DOES exist was not touched by any of those calls.
    assert_options(
        &read_endpoint(&emulator, &id).await,
        &created_with(),
        "an endpoint no refused call addressed",
    );
}
