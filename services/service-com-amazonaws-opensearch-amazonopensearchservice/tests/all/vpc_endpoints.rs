// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the service-managed VPC endpoint surface:
//! `CreateVpcEndpoint`, `DescribeVpcEndpoints`, `ListVpcEndpoints` and
//! `DeleteVpcEndpoint`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a create mints ONE endpoint for the domain its `DomainArn` names and every read
//!   path reports that same stored record, over a table of five creates differing in
//!   how many subnets they name, whether they name security groups, and whether egress
//!   is asked for;
//! * the endpoint is minted in a transient status and the clock owes it the status
//!   provisioning leaves it in: `CREATING` the instant the call returns and `ACTIVE`
//!   once the span has passed, without any call having asked for the move;
//! * a `DomainArn` that no endpoint can be created for is refused with the service's
//!   ONE non-committal `ValidationException`, over the four ways it can fail, and
//!   nothing is created;
//! * the two error channels of this surface differ by what the call does: a READ of
//!   ids reports an id it holds no endpoint for IN ITS PAYLOAD, with the modelled
//!   `ENDPOINT_NOT_FOUND` code, while still reporting every id that resolves; a call
//!   that ADDRESSES one endpoint refuses a missing id with the modelled
//!   `ResourceNotFoundException`;
//! * a release takes the endpoint out of every read path, and a second release of the
//!   same id names no endpoint;
//! * `ClientToken` makes a create idempotent: the same token twice answers with the
//!   one endpoint the first call minted;
//! * the number of endpoints a create may add is bounded by the set the ACCOUNT already
//!   holds at the instant of the call: one create at the bound is served, every create
//!   past it is refused with the modelled `LimitExceededException` and mints nothing —
//!   whichever domain it names and whether or not it carries a token — a retry of a
//!   create already served is still answered from the record it made, and releasing an
//!   endpoint frees exactly one slot, so the bound counts what is STORED and not how
//!   many creates were ever made;
//! * one answer reports the whole set exactly once, in the order the listing reports
//!   it, and carries no `NextToken` member at all.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::VPC_ENDPOINT_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ENDPOINT_DOES_NOT_EXIST, ENDPOINT_INACCESSIBLE, VPC_ENDPOINT_QUOTA,
    VPC_ENDPOINT_STATUS_ACTIVE, VPC_ENDPOINT_STATUS_CREATING, VPC_ENDPOINT_STATUS_DELETING,
};

use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The sentence the service refuses a `DomainArn` it cannot create an endpoint for
/// with: one refusal for every reason, naming none of them.
const DOMAIN_UNAVAILABLE: &str = "Either the domain doesn't exist, it doesn't support creation \
                                  of VPC endpoints, or you don't have permission to create a \
                                  VPC endpoint for the domain.";

/// `CreateVpcEndpoint`: `POST /2021-01-01/opensearch/vpcEndpoints`.
///
/// `pub(crate)` because the sibling suites for `UpdateVpcEndpoint` and for the
/// per-domain listing create endpoints through THIS wrapper rather than growing a
/// second copy of it.
pub(crate) async fn create_endpoint(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/vpcEndpoints"),
            Some(body),
        )
        .await
}

/// `DescribeVpcEndpoints`: `POST /2021-01-01/opensearch/vpcEndpoints/describe`.
///
/// `pub(crate)` for the same reason as [`create_endpoint`]: it is this crate's one read
/// of an endpoint by id.
pub(crate) async fn describe_endpoints(emulator: &Emulator, ids: &[&str]) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/vpcEndpoints/describe"),
            Some(json!({ "VpcEndpointIds": ids })),
        )
        .await
}

/// `ListVpcEndpoints`: `GET /2021-01-01/opensearch/vpcEndpoints`, optionally resumed
/// from a page token.
///
/// `pub(crate)` for the same reason as [`create_endpoint`]: the sibling suite for the
/// statuses an endpoint rests in polls BOTH read paths through this crate's one copy of
/// each.
pub(crate) async fn list_endpoints(emulator: &Emulator, token: Option<&str>) -> Answer {
    let uri = match token {
        None => format!("{PREFIX}/opensearch/vpcEndpoints"),
        Some(token) => format!("{PREFIX}/opensearch/vpcEndpoints?nextToken={token}"),
    };
    emulator.call("GET", &uri, None).await
}

/// `DeleteVpcEndpoint`: `DELETE /2021-01-01/opensearch/vpcEndpoints/{VpcEndpointId}`.
///
/// `pub(crate)` for the same reason as [`create_endpoint`]: it is this crate's one
/// release of an endpoint.
pub(crate) async fn delete_endpoint(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/vpcEndpoints/{id}"),
            None,
        )
        .await
}

/// A VPC-provisioned domain to create endpoints for, answering with its ARN — the
/// value a create addresses the domain by.
///
/// `pub(crate)` for the same reason as [`create_endpoint`].
pub(crate) async fn vpc_domain(emulator: &Emulator, name: &str) -> String {
    let answer = emulator
        .create(json!({
            "DomainName": name,
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
            "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
            "VPCOptions": {
                "SubnetIds": ["subnet-0123456789abcdef0"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}, in a VPC): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateDomain({name:?}): expected the domain's ARN, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// A domain with no `VPCOptions` at all, answering with its ARN.
async fn public_domain(emulator: &Emulator, name: &str) -> String {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}, public): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateDomain({name:?}): expected the domain's ARN, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// The `VpcEndpoint` of an accepted create, with the case named on failure.
///
/// `pub(crate)` for the same reason as [`create_endpoint`].
pub(crate) fn minted(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let endpoint = answer.body["VpcEndpoint"].clone();
    assert!(
        endpoint.is_object(),
        "case {case}: expected a VpcEndpoint object, got {:?}",
        answer.body
    );
    endpoint
}

/// A required string member of a response object, with the case named on failure.
///
/// `pub(crate)` for the same reason as [`create_endpoint`].
pub(crate) fn member(object: &Value, name: &str, case: &str) -> String {
    object[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {name}, got {object:?}"))
        .to_owned()
}

/// One create to make: how its `VpcOptions` differ from the other rows'.
struct Row {
    case: &'static str,
    domain: &'static str,
    vpc_options: Value,
}

/// The create table: one row per way an accepted create's `VpcOptions` differ — one
/// subnet, two subnets, no security group at all, several security groups, and egress
/// asked for explicitly.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one subnet and one security group",
            domain: "endpoint-one",
            vpc_options: json!({
                "SubnetIds": ["subnet-0aaa0000000000001"],
                "SecurityGroupIds": ["sg-0aaa0000000000001"],
            }),
        },
        Row {
            case: "two subnets, one per availability zone",
            domain: "endpoint-two-subnets",
            vpc_options: json!({
                "SubnetIds": ["subnet-0bbb0000000000001", "subnet-0bbb0000000000002"],
                "SecurityGroupIds": ["sg-0bbb0000000000001"],
            }),
        },
        Row {
            case: "no security group, so the service picks the default",
            domain: "endpoint-no-group",
            vpc_options: json!({ "SubnetIds": ["subnet-0ccc0000000000001"] }),
        },
        Row {
            case: "several security groups",
            domain: "endpoint-many-groups",
            vpc_options: json!({
                "SubnetIds": ["subnet-0ddd0000000000001"],
                "SecurityGroupIds": [
                    "sg-0ddd0000000000001",
                    "sg-0ddd0000000000002",
                    "sg-0ddd0000000000003",
                ],
            }),
        },
        Row {
            case: "egress routed through the customer VPC",
            domain: "endpoint-egress",
            vpc_options: json!({
                "SubnetIds": ["subnet-0eee0000000000001"],
                "SecurityGroupIds": ["sg-0eee0000000000001"],
                "EgressEnabled": true,
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_endpoint_is_persisted_and_read_back_through_every_path() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let arn = vpc_domain(&emulator, row.domain).await;
        let answer = create_endpoint(
            &emulator,
            json!({ "DomainArn": arn, "VpcOptions": row.vpc_options }),
        )
        .await;
        let endpoint = minted(&answer, case);
        let id = member(&endpoint, "VpcEndpointId", case);

        assert!(
            id.starts_with("aos-"),
            "case {case}: the endpoint's id carries the service's own prefix, got {id:?}"
        );
        assert_eq!(
            member(&endpoint, "DomainArn", case),
            arn,
            "case {case}: the endpoint names the domain it was created for, got {endpoint:?}"
        );
        assert_eq!(
            member(&endpoint, "VpcEndpointOwner", case),
            engine::emulator_account_id(),
            "case {case}: the endpoint names the account that created it, got {endpoint:?}"
        );
        assert_eq!(
            endpoint["VpcOptions"]["SubnetIds"], row.vpc_options["SubnetIds"],
            "case {case}: the endpoint reports the subnets it was given, got {endpoint:?}"
        );
        assert_eq!(
            endpoint["VpcOptions"]["SecurityGroupIds"], row.vpc_options["SecurityGroupIds"],
            "case {case}: the endpoint reports the security groups it was given, got {endpoint:?}"
        );
        assert_eq!(
            endpoint["VpcOptions"]["EgressEnabled"], row.vpc_options["EgressEnabled"],
            "case {case}: the endpoint reports the egress option it was given, got {endpoint:?}"
        );
        assert!(
            member(&endpoint, "Endpoint", case).ends_with(".es.amazonaws.com"),
            "case {case}: the endpoint reports the host it is reached on, got {endpoint:?}"
        );
        assert_eq!(
            member(&endpoint, "Status", case),
            VPC_ENDPOINT_STATUS_CREATING,
            "case {case}: a new endpoint is being provisioned, got {endpoint:?}"
        );

        // The read of the one id reports the SAME record the create answered with.
        let read = describe_endpoints(&emulator, &[&id]).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected 200 from DescribeVpcEndpoints, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["VpcEndpoints"],
            json!([endpoint]),
            "case {case}: the read reports the record the create minted, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["VpcEndpointErrors"],
            json!([]),
            "case {case}: an id the store holds an endpoint for is no error, got {:?}",
            read.body
        );

        // And so does the listing, through the summary projection: the same id, owner,
        // domain and status.
        let listed = list_endpoints(&emulator, None).await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from ListVpcEndpoints, got {} with {:?}",
            listed.status, listed.body
        );
        let summaries = listed.body["VpcEndpointSummaryList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a VpcEndpointSummaryList, got {:?}",
                    listed.body
                )
            })
            .clone();
        let summary = summaries
            .iter()
            .find(|summary| summary["VpcEndpointId"] == json!(id))
            .unwrap_or_else(|| panic!("case {case}: the listing reports the minted endpoint"));
        assert_eq!(
            *summary,
            json!({
                "VpcEndpointId": id,
                "VpcEndpointOwner": engine::emulator_account_id(),
                "DomainArn": arn,
                "Status": VPC_ENDPOINT_STATUS_CREATING,
            }),
            "case {case}: the listing reports the stored record, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn an_endpoint_settles_from_creating_to_active_on_the_clock() {
    let emulator = Emulator::new();
    let arn = vpc_domain(&emulator, "endpoint-settling").await;

    // Every row's endpoint is minted on the same domain, so what differs is only the
    // options each was created with: the settling is owed to each of them alike.
    let mut ids = Vec::new();
    for row in rows() {
        let answer = create_endpoint(
            &emulator,
            json!({ "DomainArn": arn, "VpcOptions": row.vpc_options }),
        )
        .await;
        ids.push((row.case, member(&minted(&answer, row.case), "VpcEndpointId", row.case)));
    }

    // Nothing below asks for the move: the span the endpoint was minted with falls due
    // on its own, and the next read materialises it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, id) in &ids {
        let read = describe_endpoints(&emulator, &[id]).await;
        assert_eq!(
            read.body["VpcEndpoints"][0]["Status"],
            json!(VPC_ENDPOINT_STATUS_ACTIVE),
            "case {case}: the status the clock owed is in place once the span passed, got {:?}",
            read.body
        );
        assert!(
            VPC_ENDPOINT_STATUS_VALUES.contains(&VPC_ENDPOINT_STATUS_ACTIVE),
            "case {case}: the status reported is one the model declares"
        );
    }
}

#[tokio::test]
async fn a_domain_no_endpoint_can_be_created_for_is_refused_and_nothing_is_created() {
    let emulator = Emulator::new();
    let vpc = vpc_domain(&emulator, "endpoint-refusals").await;
    let public = public_domain(&emulator, "endpoint-public").await;
    let (prefix, _) = vpc
        .rsplit_once('/')
        .expect("a domain ARN names its domain after a slash");

    // One row per way a `DomainArn` can fail the rule: a domain nothing created, a
    // domain created and then deleted, a domain that exists but has no VPC to proxy
    // into, and an ARN whose Region and account are not this account's.
    let deleted = "endpoint-deleted";
    let deleted_arn = vpc_domain(&emulator, deleted).await;
    let gone = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{deleted}"), None)
        .await;
    assert_eq!(
        gone.status, 200,
        "case DeleteDomain({deleted:?}): expected 200, got {} with {:?}",
        gone.status, gone.body
    );

    let rows: Vec<(&str, String)> = vec![
        ("a domain nothing created", format!("{prefix}/no-such-domain")),
        ("a domain that was deleted", deleted_arn),
        ("a domain with no VPC to proxy into", public),
        (
            "an ARN in another Region and account",
            "arn:aws:es:eu-west-1:111111111111:domain/elsewhere".to_owned(),
        ),
    ];

    for (case, arn) in rows {
        let answer = create_endpoint(
            &emulator,
            json!({
                "DomainArn": arn,
                "VpcOptions": { "SubnetIds": ["subnet-0fff0000000000001"] },
            }),
        )
        .await;
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
        assert_eq!(
            answer.body["message"].as_str(),
            Some(DOMAIN_UNAVAILABLE),
            "case {case:?}: expected the service's one non-committal sentence, got {:?}",
            answer.body
        );
        let listed = list_endpoints(&emulator, None).await;
        assert_eq!(
            listed.body["VpcEndpointSummaryList"],
            json!([]),
            "case {case:?}: a refused create mints nothing, got {:?}",
            listed.body
        );
    }

    // The same call for the domain that CAN take an endpoint is served, so the refusals
    // above are the domain each named rather than the call.
    let accepted = create_endpoint(
        &emulator,
        json!({
            "DomainArn": vpc,
            "VpcOptions": { "SubnetIds": ["subnet-0fff0000000000001"] },
        }),
    )
    .await;
    let endpoint = minted(&accepted, "a domain an endpoint can be created for");
    assert_eq!(
        endpoint["DomainArn"],
        json!(vpc),
        "case a domain an endpoint can be created for: expected the endpoint to name it, got \
         {endpoint:?}"
    );
}

#[tokio::test]
async fn an_id_that_names_no_endpoint_is_reported_in_the_channel_the_call_has() {
    let emulator = Emulator::new();
    let arn = vpc_domain(&emulator, "endpoint-channels").await;
    let answer = create_endpoint(
        &emulator,
        json!({
            "DomainArn": arn,
            "VpcOptions": { "SubnetIds": ["subnet-0999000000000001"] },
        }),
    )
    .await;
    let held = member(&minted(&answer, "the endpoint under test"), "VpcEndpointId", "the endpoint under test");

    // One row per way a READ can name ids, over the ids the store holds an endpoint for
    // and ids it does not: the read answers 200 either way, reporting each resolved id
    // and one error per unresolved one, in request order.
    let unknown = ["aos-00000000000000", "aos-1", "aos-zzzzzzzzzzzzzzzzzzzz"];
    let rows: Vec<(&str, Vec<&str>, usize, Vec<&str>)> = vec![
        ("one unknown id", vec![unknown[0]], 0, vec![unknown[0]]),
        (
            "several unknown ids",
            unknown.to_vec(),
            0,
            unknown.to_vec(),
        ),
        ("the one id the store holds", vec![&held], 1, Vec::new()),
        (
            "a known id beside an unknown one",
            vec![&held, unknown[1]],
            1,
            vec![unknown[1]],
        ),
        (
            "the same unknown id named twice",
            vec![unknown[2], unknown[2]],
            0,
            vec![unknown[2], unknown[2]],
        ),
    ];

    for (case, ids, endpoints, missing) in rows {
        let read = describe_endpoints(&emulator, &ids).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a read of ids is answered, not refused: got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["VpcEndpoints"].as_array().map(Vec::len),
            Some(endpoints),
            "case {case:?}: expected {endpoints} endpoint(s) reported, got {:?}",
            read.body
        );
        let errors = read.body["VpcEndpointErrors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a VpcEndpointErrors list, got {:?}", read.body))
            .clone();
        assert_eq!(
            errors.len(),
            missing.len(),
            "case {case:?}: expected {} error(s), got {errors:?}",
            missing.len()
        );
        for (error, id) in errors.iter().zip(&missing) {
            assert_eq!(
                (error["VpcEndpointId"].as_str(), error["ErrorCode"].as_str()),
                (Some(*id), Some("ENDPOINT_NOT_FOUND")),
                "case {case:?}: expected the id and the modelled code for an endpoint that does \
                 not exist, got {error:?}"
            );
            assert_eq!(
                error["ErrorMessage"].as_str(),
                Some(ENDPOINT_DOES_NOT_EXIST),
                "case {case:?}: expected the service's own sentence for an endpoint that does \
                 not exist, got {error:?}"
            );
        }

        // A call that ADDRESSES one endpoint refuses the same ids instead of reporting
        // them in a payload.
        for id in &missing {
            let released = delete_endpoint(&emulator, id).await;
            assert_eq!(
                released.status, RECORDED_NOT_FOUND_STATUS,
                "case {case:?}: expected {RECORDED_NOT_FOUND_STATUS} from \
                 DeleteVpcEndpoint({id:?}), got {} with {:?}",
                released.status, released.body
            );
            assert_eq!(
                error_code(&released),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for {id:?}, got {:?}",
                released.body
            );
            assert_eq!(
                released.body["message"].as_str(),
                Some(ENDPOINT_INACCESSIBLE),
                "case {case:?}: expected the refusal a call addressing one endpoint states, got \
                 {:?}",
                released.body
            );
        }

        // And no read of an id this service never handed out makes one up.
        let listed = list_endpoints(&emulator, None).await;
        assert_eq!(
            listed.body["VpcEndpointSummaryList"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "case {case:?}: the listing reports only the endpoint that was created, got {:?}",
            listed.body
        );
    }

    // The answer carries one entry per id named, so a read that names more ids than
    // this account can hold endpoints is refused rather than answered: one at the bound
    // is served, one past it is not.
    let at_bound: Vec<String> = (0..VPC_ENDPOINT_QUOTA)
        .map(|index| format!("aos-{index:012}"))
        .collect();
    let borrowed: Vec<&str> = at_bound.iter().map(String::as_str).collect();
    let served = describe_endpoints(&emulator, &borrowed).await;
    assert_eq!(
        served.status, 200,
        "case DescribeVpcEndpoints(at the bound): expected 200, got {} with {:?}",
        served.status, served.body
    );
    let mut past_bound = borrowed.clone();
    past_bound.push(&held);
    let refused = describe_endpoints(&emulator, &past_bound).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case DescribeVpcEndpoints(past the bound): expected the request to be refused, got {} \
         with {:?}",
        refused.status,
        refused.body
    );
}

#[tokio::test]
async fn a_released_endpoint_is_in_no_read_path_and_cannot_be_released_twice() {
    let emulator = Emulator::new();
    let arn = vpc_domain(&emulator, "endpoint-release").await;

    // Three endpoints on one domain, released in a different order from the one they
    // were minted in, so what a release takes away is the endpoint it names and not a
    // position in the set.
    let mut ids = Vec::new();
    for index in 0..3 {
        let answer = create_endpoint(
            &emulator,
            json!({
                "DomainArn": arn,
                "VpcOptions": { "SubnetIds": [format!("subnet-0888000000000{index:03}")] },
            }),
        )
        .await;
        ids.push(member(
            &minted(&answer, "an endpoint to release"),
            "VpcEndpointId",
            "an endpoint to release",
        ));
    }

    for index in [1usize, 0, 2] {
        let id = &ids[index];
        let case = format!("DeleteVpcEndpoint({id:?})");
        let released = delete_endpoint(&emulator, id).await;
        assert_eq!(
            released.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            released.status, released.body
        );
        let summary = released.body["VpcEndpointSummary"].clone();
        assert_eq!(
            (
                summary["VpcEndpointId"].as_str(),
                summary["Status"].as_str(),
                summary["DomainArn"].as_str(),
            ),
            (
                Some(id.as_str()),
                Some(VPC_ENDPOINT_STATUS_DELETING),
                Some(arn.as_str())
            ),
            "case {case}: expected the released endpoint in the status the release left it in, \
             got {summary:?}"
        );

        // Gone from both read paths, and a second release names no endpoint.
        let read = describe_endpoints(&emulator, &[id]).await;
        assert_eq!(
            read.body["VpcEndpoints"],
            json!([]),
            "case {case}: a released endpoint is reported by no read, got {:?}",
            read.body
        );
        assert_eq!(
            read.body["VpcEndpointErrors"][0]["ErrorCode"],
            json!("ENDPOINT_NOT_FOUND"),
            "case {case}: a released id names no endpoint, got {:?}",
            read.body
        );
        let again = delete_endpoint(&emulator, id).await;
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {case}: a second release names no endpoint, got {:?}",
            again.body
        );
    }

    let listed = list_endpoints(&emulator, None).await;
    assert_eq!(
        listed.body["VpcEndpointSummaryList"],
        json!([]),
        "case ListVpcEndpoints(after every release): expected no endpoint at all, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn the_same_client_token_mints_one_endpoint() {
    let emulator = Emulator::new();
    let arn = vpc_domain(&emulator, "endpoint-idempotent").await;

    // Rows differ in the token's shape, which the rule generalises over: a plain token,
    // one carrying a hyphenated id, and one at the member's maximum length.
    let tokens = [
        "retry-1".to_owned(),
        "0b2f8c6e-9a1d-4d7f-9b0a-1f2e3d4c5b6a".to_owned(),
        "t".repeat(64),
    ];

    for token in tokens {
        let body = json!({
            "DomainArn": arn,
            "VpcOptions": { "SubnetIds": ["subnet-0777000000000001"] },
            "ClientToken": token,
        });
        let first = create_endpoint(&emulator, body.clone()).await;
        let one = minted(&first, "a create carrying a client token");
        let second = create_endpoint(&emulator, body).await;
        let again = minted(&second, "the same create, retried");
        assert_eq!(
            one, again,
            "case ClientToken({token:?}): a retried create answers with the endpoint the first \
             one minted, got {one:?} then {again:?}"
        );
    }

    let listed = list_endpoints(&emulator, None).await;
    assert_eq!(
        listed.body["VpcEndpointSummaryList"]
            .as_array()
            .map(Vec::len),
        Some(3),
        "case ListVpcEndpoints: expected one endpoint per distinct token, got {:?}",
        listed.body
    );

    // One token cannot identify two different creates: a token already held for
    // another domain is the modelled conflict rather than a second endpoint or the
    // other domain's one.
    let other = vpc_domain(&emulator, "endpoint-other-domain").await;
    let clashing = create_endpoint(
        &emulator,
        json!({
            "DomainArn": other,
            "VpcOptions": { "SubnetIds": ["subnet-0777000000000002"] },
            "ClientToken": "retry-1",
        }),
    )
    .await;
    assert_eq!(
        (clashing.status, error_code(&clashing).as_str()),
        (409, "ConflictException"),
        "case ClientToken(reused for another domain): expected the modelled conflict, got {} \
         with {:?}",
        clashing.status,
        clashing.body
    );
    let listed = list_endpoints(&emulator, None).await;
    assert_eq!(
        listed.body["VpcEndpointSummaryList"]
            .as_array()
            .map(Vec::len),
        Some(3),
        "case ListVpcEndpoints(after the clash): a refused create mints nothing, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn the_listing_reports_the_whole_set_exactly_once_and_ends_it_with_no_token() {
    let emulator = Emulator::new();
    let arn = vpc_domain(&emulator, "endpoint-paging").await;

    // Rows differ in how large the set being reported is — the rule generalises over
    // the size of the set, and the largest row is far past the ten summaries this
    // listing used to cap an answer at, so a capped answer could not pass it.
    let sizes = [1_usize, 10, 23];
    let mut minted_ids: Vec<String> = Vec::new();
    for total in sizes {
        while minted_ids.len() < total {
            let index = minted_ids.len();
            let answer = create_endpoint(
                &emulator,
                json!({
                    "DomainArn": arn,
                    "VpcOptions": { "SubnetIds": [format!("subnet-0666000000000{index:03}")] },
                }),
            )
            .await;
            minted_ids.push(member(
                &minted(&answer, "an endpoint to list"),
                "VpcEndpointId",
                "an endpoint to list",
            ));
        }

        let answer = list_endpoints(&emulator, None).await;
        assert_eq!(
            answer.status, 200,
            "case ListVpcEndpoints({total} endpoints): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        // A complete result set is reported with NO `NextToken` member at all — not the
        // empty string, which a client testing for the member's presence rather than
        // its truthiness paginates on forever.
        assert_eq!(
            answer.body.get("NextToken"),
            None,
            "case ListVpcEndpoints({total} endpoints): the whole set is reported, so the \
             answer carries no NextToken, got {:?}",
            answer.body
        );
        let listed: Vec<String> = summaries(&answer, "ListVpcEndpoints")
            .iter()
            .map(|summary| member(summary, "VpcEndpointId", "ListVpcEndpoints"))
            .collect();
        let mut expected = minted_ids.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case ListVpcEndpoints({total} endpoints): one answer reports every endpoint \
             exactly once, in the order the listing reports them"
        );
    }

    // A token that names no endpoint of the set is a malformed request rather than an
    // empty page: this listing hands none out, so a token sent to it is never one it
    // issued.
    let refused = list_endpoints(&emulator, Some("aos-00000000000000")).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case ListVpcEndpoints(a token naming nothing): expected the request to be refused, got \
         {} with {:?}",
        refused.status,
        refused.body
    );
}

/// The `VpcEndpointSummaryList` of a listing answer, with the case named on failure.
fn summaries(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["VpcEndpointSummaryList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a VpcEndpointSummaryList, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// Every endpoint id this account holds, in the order the listing reports them: the
/// STORED set a create is counted against. One answer reports the whole set — it is
/// bounded by [`VPC_ENDPOINT_QUOTA`] — so there is no page to walk.
async fn held_ids(emulator: &Emulator, case: &str) -> Vec<String> {
    let answer = list_endpoints(emulator, None).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListVpcEndpoints, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {case}: the whole set is reported, so the answer carries no NextToken, got {:?}",
        answer.body
    );
    summaries(&answer, case)
        .iter()
        .map(|summary| member(summary, "VpcEndpointId", case))
        .collect()
}

/// One create to make once the account is at its bound: which domain it names and what
/// else it carries.
struct PastBound {
    case: &'static str,
    body: Value,
}

/// The subnet the create filling position `index` of the account's bound names, so the
/// retry of one of those creates asks for exactly what it asked for.
fn filling_subnet(index: usize) -> String {
    format!("subnet-0b0b00000000{index:05}")
}

#[tokio::test]
async fn a_create_past_the_account_s_bound_is_refused_and_mints_nothing() {
    let emulator = Emulator::new();

    // The bound is on the ACCOUNT's set, so it is filled across two domains: neither of
    // them alone holds the maximum, and the refusals below are the account's set rather
    // than any one domain's.
    let first = vpc_domain(&emulator, "endpoint-bound-one").await;
    let second = vpc_domain(&emulator, "endpoint-bound-two").await;
    // A token the first create is served under, so the retry below is a repeat of a
    // request the account already holds an endpoint for.
    let served_token = "bound-retry";

    for index in 0..VPC_ENDPOINT_QUOTA {
        let case = format!("the create filling position {index} of the bound");
        let arn = if index % 2 == 0 { &first } else { &second };
        let mut body = json!({
            "DomainArn": arn,
            "VpcOptions": { "SubnetIds": [filling_subnet(index)] },
        });
        if index == 0 {
            body["ClientToken"] = json!(served_token);
        }
        let answer = create_endpoint(&emulator, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: a create below the bound is served, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // The create that brought the account TO the bound was served, so what is refused
    // below is only the one past it.
    let at_bound = held_ids(&emulator, "the account at its bound").await;
    assert_eq!(
        at_bound.len(),
        VPC_ENDPOINT_QUOTA,
        "case the account at its bound: expected the creates at and below the bound to all be \
         held, got {at_bound:?}"
    );

    // A third domain holding no endpoint at all, so one row below asks for the create on
    // a domain that has used none of the account's set.
    let third = vpc_domain(&emulator, "endpoint-bound-three").await;
    let rows = vec![
        PastBound {
            case: "a domain that already holds endpoints",
            body: json!({
                "DomainArn": first,
                "VpcOptions": { "SubnetIds": ["subnet-0c0c00000000000a1"] },
            }),
        },
        PastBound {
            case: "a domain that holds none",
            body: json!({
                "DomainArn": third,
                "VpcOptions": { "SubnetIds": ["subnet-0c0c00000000000a2"] },
            }),
        },
        PastBound {
            case: "a create carrying a token nothing was served under",
            body: json!({
                "DomainArn": second,
                "VpcOptions": {
                    "SubnetIds": ["subnet-0c0c00000000000a3", "subnet-0c0c00000000000a4"],
                    "SecurityGroupIds": ["sg-0c0c00000000000a1"],
                    "EgressEnabled": true,
                },
                "ClientToken": "past-the-bound",
            }),
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = create_endpoint(&emulator, row.body).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409 past the bound, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "LimitExceededException",
            "case {case}: expected LimitExceededException, got {:?}",
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default().to_owned();
        assert!(
            message.contains(&VPC_ENDPOINT_QUOTA.to_string()),
            "case {case}: expected the refusal to state the maximum of {VPC_ENDPOINT_QUOTA}, got \
             {message:?}"
        );
        assert_eq!(
            held_ids(&emulator, case).await,
            at_bound,
            "case {case}: a refused create mints nothing, so the account holds the same set"
        );
    }

    // A repeat of a create the account WAS served is still answered from the record that
    // create made: it adds nothing to the set, so the bound has nothing to refuse.
    let retried = create_endpoint(
        &emulator,
        json!({
            "DomainArn": first,
            "VpcOptions": { "SubnetIds": [filling_subnet(0)] },
            "ClientToken": served_token,
        }),
    )
    .await;
    let endpoint = minted(&retried, "a create already served, retried at the bound");
    assert!(
        at_bound.contains(&member(
            &endpoint,
            "VpcEndpointId",
            "a create already served, retried at the bound"
        )),
        "case a create already served, retried at the bound: expected the endpoint the first \
         call minted, got {endpoint:?}"
    );
    assert_eq!(
        held_ids(&emulator, "a create already served, retried at the bound").await,
        at_bound,
        "case a create already served, retried at the bound: the retry mints nothing"
    );

    // Releasing an endpoint frees exactly one slot: the bound counts the set the store
    // HOLDS at the instant of the call, not how many creates it ever served.
    let released = at_bound
        .last()
        .expect("the account holds endpoints at its bound")
        .clone();
    let gone = delete_endpoint(&emulator, &released).await;
    assert_eq!(
        gone.status, 200,
        "case releasing one endpoint at the bound: expected 200, got {} with {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        held_ids(&emulator, "one slot freed").await.len(),
        VPC_ENDPOINT_QUOTA - 1,
        "case one slot freed: expected the released endpoint to be out of the set"
    );

    let refilled = create_endpoint(
        &emulator,
        json!({
            "DomainArn": third,
            "VpcOptions": { "SubnetIds": ["subnet-0d0d00000000000a1"] },
        }),
    )
    .await;
    let took_the_slot = minted(&refilled, "the create taking the freed slot");
    assert_eq!(
        took_the_slot["DomainArn"],
        json!(third),
        "case the create taking the freed slot: expected it minted for the domain it named, got \
         {took_the_slot:?}"
    );
    assert_eq!(
        held_ids(&emulator, "the slot taken again").await.len(),
        VPC_ENDPOINT_QUOTA,
        "case the slot taken again: expected the account back at its bound"
    );

    // And the account is at its bound again, so the next create past it is refused as
    // before.
    let refused_again = create_endpoint(
        &emulator,
        json!({
            "DomainArn": third,
            "VpcOptions": { "SubnetIds": ["subnet-0d0d00000000000a2"] },
        }),
    )
    .await;
    assert_eq!(
        (refused_again.status, error_code(&refused_again).as_str()),
        (409, "LimitExceededException"),
        "case a create past the bound once the slot was taken again: expected the same refusal, \
         got {} with {:?}",
        refused_again.status,
        refused_again.body
    );
}
