// SPDX-License-Identifier: CC0-1.0

//! What Amazon MSK admits a VPC connection — multi-VPC private connectivity — to,
//! driven end to end over the same `MultiServiceRouter` the emulator serves from
//! (the shared `kafka_harness`).
//!
//! `CreateVpcConnection` asks the TARGET CLUSTER before it reads the rest of the
//! request, and this suite asserts each of those decisions as a RULE over a table
//! of inputs rather than one request:
//!
//! * AUTHORIZATION on the resource the request names, taken before the target is
//!   resolved or parsed: a `TargetClusterArn` the caller holds no grant over —
//!   another account's cluster, or a value naming no cluster of this account at
//!   all — is refused HTTP 403 `AccessDeniedException` with the sentence AWS words
//!   that denial in, never a parameter complaint, and whatever else the body
//!   carried.
//! * The cluster's own RESOURCE-BASED POLICY: an explicit deny of the action
//!   refuses the call with the same code and status, ahead of every precondition
//!   below, and the call reverts to its previous answer once the policy is
//!   deleted. The same holds for the other actions an MSK cluster policy governs
//!   (`GetBootstrapBrokers`, `DescribeCluster`, `DescribeClusterV2`).
//! * The cluster's STATE: a cluster that does not serve a VPC-connection
//!   operation is refused 400 `BadRequestException` with MSK's fixed sentence, and
//!   registers nothing — the same refusal whether the rest of the request was
//!   usable or not, which is what makes it the FIRST gate after authorization.
//! * The cluster's CLIENT AUTHENTICATION: a connection is served only for a
//!   scheme the cluster turns on, and refused with MSK's sentence for that scheme
//!   otherwise.
//!
//! Each suite's controls pin the other side of the rule: the same request against
//! a cluster that does admit it is served, so no suite here could pass against a
//! handler that refuses everything.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::authorization::{
    caller_principal, CLUSTER_POLICY_ACTIONS,
};
use service_com_amazonaws_kafka_kafka::state::VPC_CONNECTION_AUTHENTICATION_TYPES;

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, ConnectionRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The HTTP status an authorization refusal carries, spelled out here rather than
/// read from the service so a drift in the service's own constant fails this
/// suite.
const ACCESS_DENIED_STATUS: u16 = 403;

/// The wire error code (the JSON envelope's `__type`) those refusals carry.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The sentence Amazon MSK refuses a VPC-connection operation on a cluster whose
/// state does not serve one with, as the recorded refusal words it: fixed text end
/// to end, naming the states it is served from and nothing of the request.
const INVALID_CLUSTER_STATE_MESSAGE: &str =
    "Invalid cluster state. To create, delete or reject a VPC connection, your cluster must be \
     in one of these states: ACTIVE, UPDATING, MAINTENANCE, HEALING, or REBOOTING_BROKER.";

/// The wire name of the member the refusals above report as the parameter at
/// fault.
const TARGET_CLUSTER_ARN: &str = "targetClusterArn";

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// How MSK writes each published `Authentication` value in the sentence that
/// refuses it: the SASL family and its mechanism separated by a slash. Spelled out
/// here, so the suite does not read its expectation off the implementation.
fn scheme_label(authentication: &str) -> &'static str {
    match authentication {
        "SASL_IAM" => "SASL/IAM",
        "SASL_SCRAM" => "SASL/SCRAM",
        "TLS" => "TLS",
        other => panic!("the suite has no label for the authentication type {other:?}"),
    }
}

/// The sentence MSK refuses a connection asking for a scheme the cluster does not
/// turn on with. The SASL/SCRAM spelling of it is the one the SCRAM-secret surface
/// answers, which this crate publishes as `SASL_SCRAM_NOT_ENABLED_MESSAGE`.
fn not_enabled_message(authentication: &str) -> String {
    format!(
        "{} client authentication must be enabled for the cluster.",
        scheme_label(authentication)
    )
}

/// The sentence AWS refuses an action nothing grants with, naming the principal,
/// the action and the resource the decision was taken over.
fn not_authorized(action: &str, resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {action} on resource: {resource}",
        caller_principal()
    )
}

/// The same sentence closed by the clause AWS adds when a resource-based policy
/// explicitly denied the request.
fn explicit_deny_message(action: &str, resource: &str) -> String {
    format!(
        "{} with an explicit deny in a resource-based policy",
        not_authorized(action, resource)
    )
}

/// The resource URI of one cluster's policy: the model binds the three policy
/// operations to `/v1/clusters/{ClusterArn}/policy`.
fn policy_uri(arn: &str) -> String {
    format!("/v1/clusters/{}/policy", pct(arn))
}

/// `PutClusterPolicy` with `document` attached to the cluster, asserted to
/// succeed: this suite is about what the stored policy then DOES, so a failure to
/// store it is a setup failure.
async fn put_policy(emulator: &Emulator, case: &str, arn: &str, document: &Value) {
    let reply = emulator
        .call(
            "PUT",
            &policy_uri(arn),
            Some(json!({ "policy": document.to_string() })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected PutClusterPolicy to store the document, got {} {}",
        reply.status, reply.body
    );
}

/// `DeleteClusterPolicy`, asserted to succeed.
async fn delete_policy(emulator: &Emulator, case: &str, arn: &str) {
    let reply = emulator.call("DELETE", &policy_uri(arn), None).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DeleteClusterPolicy to remove the document, got {} {}",
        reply.status, reply.body
    );
}

/// A cluster created with exactly the client authentication the row wants, waited
/// into `ACTIVE`.
///
/// `client_authentication` is the cluster's own `ClientAuthentication` and
/// `vpc_connectivity` the one under
/// `BrokerNodeGroupInfo.ConnectivityInfo.VpcConnectivity`; `None` omits the
/// member, which is how a row tests a cluster that turns a scheme on in one place
/// only.
async fn cluster_with_authentication(
    emulator: &Emulator,
    name: &str,
    client_authentication: Option<Value>,
    vpc_connectivity: Option<Value>,
) -> String {
    let mut body = json!({
        "clusterName": name,
        "kafkaVersion": "3.6.0",
        "numberOfBrokerNodes": 3,
        "brokerNodeGroupInfo": {
            "instanceType": "kafka.m5.large",
            "clientSubnets": ["subnet-a", "subnet-b", "subnet-c"],
        },
    });
    if let Some(authentication) = client_authentication {
        body["clientAuthentication"] = authentication;
    }
    if let Some(vpc_connectivity) = vpc_connectivity {
        body["brokerNodeGroupInfo"]["connectivityInfo"] =
            json!({ "vpcConnectivity": { "clientAuthentication": vpc_connectivity } });
    }
    let reply = emulator.call("POST", "/v1/clusters", Some(body)).await;
    assert_eq!(
        reply.status, 200,
        "case {name}: expected CreateCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    let arn = reply.body["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateCluster returned no clusterArn"))
        .to_string();
    sleep_past_activation();
    arn
}

/// The `ClientAuthentication` document that turns `authentication` on at the
/// CLUSTER level.
fn cluster_level(authentication: &str) -> Value {
    match authentication {
        "SASL_IAM" => json!({ "sasl": { "iam": { "enabled": true } } }),
        "SASL_SCRAM" => json!({ "sasl": { "scram": { "enabled": true } } }),
        "TLS" => json!({ "tls": { "enabled": true } }),
        other => panic!("the suite cannot turn on the authentication type {other:?}"),
    }
}

/// The `VpcConnectivityClientAuthentication` document that turns `authentication`
/// on for the cluster's MULTI-VPC connectivity.
fn vpc_level(authentication: &str) -> Value {
    match authentication {
        "SASL_IAM" => json!({ "sasl": { "iam": { "enabled": true } } }),
        "SASL_SCRAM" => json!({ "sasl": { "scram": { "enabled": true } } }),
        "TLS" => json!({ "tls": { "enabled": true } }),
        other => panic!("the suite cannot turn on the authentication type {other:?}"),
    }
}

/// Every connection ARN the endpoint's region holds, so a suite can assert that a
/// refused create registered nothing.
async fn listed_connections(emulator: &Emulator) -> BTreeSet<String> {
    let reply = emulator.list_vpc_connections("").await;
    assert_eq!(
        reply.status, 200,
        "expected ListVpcConnections to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/vpcConnections")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["vpcConnectionArn"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// How many client VPC connections a cluster lists, which a refused create must
/// leave at zero.
async fn client_connection_count(emulator: &Emulator, case: &str, cluster_arn: &str) -> usize {
    let reply = emulator.list_client_vpc_connections(cluster_arn, "").await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListClientVpcConnections to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/clientVpcConnections")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// Assert `reply` is the 400 refusal `CreateVpcConnection` answers a target
/// cluster that cannot serve it: the fixed `message`, and the target member named
/// as the parameter at fault.
fn assert_target_refusal(reply: &Reply, case: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected the refusal {message:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(TARGET_CLUSTER_ARN),
        "case {case}: expected the refusal to name {TARGET_CLUSTER_ARN}, got {}",
        reply.body
    );
}

/// Assert `reply` is an authorization refusal carrying `message` as its whole
/// message.
fn assert_access_denied(reply: &Reply, case: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED_CODE),
        "case {case}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED_CODE}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected {message:?}, got {:?}",
        reply.error_message()
    );
}

/// RULE: a cluster whose state does not serve a VPC-connection operation refuses
/// `CreateVpcConnection` with HTTP 400 `BadRequestException` and MSK's fixed
/// cluster-state sentence, registering nothing — and it answers that refusal
/// before any member of the body is resolved, so the SAME refusal comes back
/// whatever else the request carried.
#[tokio::test]
async fn a_cluster_that_is_still_creating_refuses_a_connection() {
    let emulator = Emulator::new();

    // The rows differ along everything the rule generalises over: the
    // authentication type asked for, the VPC id (one of each published EC2 id
    // length, one no account holds, and one that is not an EC2 id at all), the
    // subnet and security-group counts, and the tag count. Every row is sent to a
    // cluster of its own, created and not waited for, so each is refused on the
    // state the clock has that cluster in rather than on the clock of the row
    // before it.
    let rows = [
        ("real-ids", ConnectionRequest::to("")),
        (
            "seventeen-digit-ids",
            ConnectionRequest {
                authentication: "SASL_SCRAM",
                vpc_id: "vpc-0123456789abcdef0",
                client_subnets: vec!["subnet-0123456789abcdef0", "subnet-0a1b2c3e"],
                security_groups: vec!["sg-0123456789abcdef0", "sg-0a1b2c3e"],
                tags: vec![("env", "test"), ("tier", "gold")],
                ..ConnectionRequest::to("")
            },
        ),
        (
            "a-vpc-no-account-holds",
            ConnectionRequest {
                authentication: "TLS",
                vpc_id: "vpc-00000000",
                ..ConnectionRequest::to("")
            },
        ),
        (
            "a-vpc-id-that-is-not-one",
            ConnectionRequest {
                vpc_id: "vpc-00000000deadbeef",
                ..ConnectionRequest::to("")
            },
        ),
        (
            "an-empty-subnet-list",
            ConnectionRequest {
                client_subnets: Vec::new(),
                ..ConnectionRequest::to("")
            },
        ),
        (
            "an-authentication-type-the-service-does-not-publish",
            ConnectionRequest {
                authentication: "SASL",
                ..ConnectionRequest::to("")
            },
        ),
    ];

    let mut answered: BTreeSet<String> = BTreeSet::new();
    for (index, (case, row)) in rows.iter().enumerate() {
        // A cluster of this row's own, created and left in the state the create
        // answered: a client that wires up private connectivity immediately is
        // exactly this case.
        let creating = emulator
            .create_ok(&ClusterRequest::vpc_connection_target(&format!(
                "state-gate-{index}"
            )))
            .await;
        let request = ConnectionRequest {
            target_cluster_arn: creating.clone(),
            ..row.clone()
        };
        let reply = emulator.create_vpc_connection(&request).await;
        assert_target_refusal(&reply, case, INVALID_CLUSTER_STATE_MESSAGE);
        answered.insert(reply.error_message().to_string());

        // Nothing was registered: not for this cluster, and not in the region.
        assert_eq!(
            client_connection_count(&emulator, case, &creating).await,
            0,
            "case {case}: a refused create must register nothing for the cluster"
        );
        assert!(
            listed_connections(&emulator).await.is_empty(),
            "case {case}: a refused create must register nothing in the region"
        );
    }

    // One refusal, byte for byte, across every row: a real VPC id and a
    // nonexistent one are told the same thing, because the state decided first.
    let distinct: Vec<&String> = answered.iter().collect();
    assert_eq!(
        distinct,
        vec![&INVALID_CLUSTER_STATE_MESSAGE.to_string()],
        "expected one fixed cluster-state refusal across every row, got {distinct:?}"
    );

    // The control, which is what makes the rows above a gate rather than a
    // blanket no: the same cluster serves the same request once the clock owes it
    // `ACTIVE`.
    let settling = emulator
        .create_ok(&ClusterRequest::vpc_connection_target("state-gate-control"))
        .await;
    let request = ConnectionRequest::to(&settling);
    let while_creating = emulator.create_vpc_connection(&request).await;
    assert_target_refusal(
        &while_creating,
        "control while creating",
        INVALID_CLUSTER_STATE_MESSAGE,
    );
    sleep_past_activation();
    let once_active = emulator.create_vpc_connection(&request).await;
    assert_eq!(
        once_active.status, 200,
        "case control once active: expected the same request to be served, got {} {}",
        once_active.status, once_active.body
    );
    assert_eq!(
        client_connection_count(&emulator, "control once active", &settling).await,
        1,
        "case control once active: the served create must register the connection"
    );
}

/// RULE: `CreateVpcConnection` is served only for a client-authentication scheme
/// the target cluster turns on — at the cluster, or for its multi-VPC
/// connectivity — and refuses any other with HTTP 400 `BadRequestException` and
/// MSK's sentence for the scheme the request asked for, registering nothing.
#[tokio::test]
async fn a_connection_needs_the_scheme_it_asks_for_on_the_cluster() {
    let emulator = Emulator::new();

    // Exhaustive over the published scheme set rather than sampled, and for each
    // scheme over every way a cluster can relate to it: turned on at the cluster,
    // turned on for multi-VPC connectivity, turned on nowhere, and turned on for
    // the OTHER schemes only — which is what shows the gate is per scheme rather
    // than "some authentication".
    for (index, authentication) in VPC_CONNECTION_AUTHENTICATION_TYPES.iter().enumerate() {
        let others: Vec<&str> = VPC_CONNECTION_AUTHENTICATION_TYPES
            .iter()
            .copied()
            .filter(|other| other != authentication)
            .collect();
        // (case, the cluster's own authentication, its VPC-connectivity
        // authentication, whether the create must be served)
        let rows: Vec<(&str, Option<Value>, Option<Value>, bool)> = vec![
            ("nothing enabled", None, None, false),
            (
                "the scheme at the cluster",
                Some(cluster_level(authentication)),
                None,
                true,
            ),
            (
                "the scheme for multi-VPC connectivity",
                None,
                Some(vpc_level(authentication)),
                true,
            ),
            (
                "the scheme in both places",
                Some(cluster_level(authentication)),
                Some(vpc_level(authentication)),
                true,
            ),
            (
                "only the other schemes",
                Some(cluster_level(others[0])),
                Some(vpc_level(others[1])),
                false,
            ),
        ];

        for (row, (case, client_authentication, vpc_connectivity, served)) in
            rows.into_iter().enumerate()
        {
            let case = format!("{authentication} with {case}");
            let cluster = cluster_with_authentication(
                &emulator,
                &format!("auth-gate-{index}-{row}"),
                client_authentication,
                vpc_connectivity,
            )
            .await;
            let before = listed_connections(&emulator).await;
            let request = ConnectionRequest {
                authentication,
                ..ConnectionRequest::to(&cluster)
            };
            let reply = emulator.create_vpc_connection(&request).await;
            if served {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: expected the create to be served, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.at("/authentication").as_str(),
                    Some(*authentication),
                    "case {case}: expected the connection to carry the scheme asked for, got {}",
                    reply.body
                );
                assert_eq!(
                    client_connection_count(&emulator, &case, &cluster).await,
                    1,
                    "case {case}: a served create must register the connection"
                );
                continue;
            }
            assert_target_refusal(&reply, &case, &not_enabled_message(authentication));
            assert_eq!(
                client_connection_count(&emulator, &case, &cluster).await,
                0,
                "case {case}: a refused create must register nothing for the cluster"
            );
            assert_eq!(
                listed_connections(&emulator).await,
                before,
                "case {case}: a refused create must leave the region's connections alone"
            );
        }
    }
}

/// RULE: a cluster's resource-based policy is in force on the actions an MSK
/// cluster policy governs: an explicit deny of the action for the calling
/// principal refuses the call with HTTP 403 `AccessDeniedException` and AWS's
/// explicit-deny sentence, ahead of every precondition the operation would
/// otherwise answer — and the call goes back to that previous answer once the
/// policy is deleted.
#[tokio::test]
async fn an_explicit_deny_in_the_cluster_policy_refuses_the_create() {
    let emulator = Emulator::new();
    // A cluster that cannot serve a SASL/IAM connection anyway, so a row that is
    // refused by the policy is demonstrably refused BEFORE the precondition it
    // would otherwise fail; and one that can serve it, so a row the policy does
    // not deny is demonstrably served.
    let unservable = cluster_with_authentication(&emulator, "policy-no-iam", None, None).await;
    let servable = cluster_with_authentication(
        &emulator,
        "policy-iam",
        Some(cluster_level("SASL_IAM")),
        None,
    )
    .await;

    let statement = |effect: &str, principal: Value, action: Value, resource: Value| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": effect,
                "Principal": principal,
                "Action": action,
                "Resource": resource,
            }],
        })
    };
    let caller = caller_principal();
    let root = format!("arn:aws:iam::{ACCOUNT}:root");
    let action = "kafka:CreateVpcConnection";

    // The rows differ along everything the decision generalises over: the effect,
    // how the document names the calling principal (itself, its account's root,
    // the whole-element wildcard, another account), which action it names (this
    // one, by wildcard, another one), and which resource (any, this cluster, the
    // other cluster), plus a document carrying both an allow and a deny — where
    // IAM's order says the deny wins.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a deny naming the caller",
            statement("Deny", json!({ "AWS": caller.clone() }), json!([action]), json!("*")),
            true,
        ),
        (
            "a deny naming the caller's account root",
            statement("Deny", json!({ "AWS": root.clone() }), json!([action]), json!("*")),
            true,
        ),
        (
            "a deny naming every principal",
            statement("Deny", json!("*"), json!(action), json!("*")),
            true,
        ),
        (
            "a deny by action wildcard",
            statement(
                "Deny",
                json!({ "AWS": caller.clone() }),
                json!("kafka:Create*"),
                json!("*"),
            ),
            true,
        ),
        (
            "a deny beside an allow of the same action",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "allow",
                        "Effect": "Allow",
                        "Principal": { "AWS": caller.clone() },
                        "Action": [action],
                        "Resource": "*",
                    },
                    {
                        "Sid": "deny",
                        "Effect": "Deny",
                        "Principal": { "AWS": caller.clone() },
                        "Action": [action],
                        "Resource": "*",
                    },
                ],
            }),
            true,
        ),
        (
            "an allow of the action",
            statement("Allow", json!({ "AWS": caller.clone() }), json!([action]), json!("*")),
            false,
        ),
        (
            "a deny of another action",
            statement(
                "Deny",
                json!({ "AWS": caller.clone() }),
                json!(["kafka:DeleteVpcConnection"]),
                json!("*"),
            ),
            false,
        ),
        (
            "a deny naming another account's principal",
            statement(
                "Deny",
                json!({ "AWS": "arn:aws:iam::111122223333:root" }),
                json!([action]),
                json!("*"),
            ),
            false,
        ),
    ];

    for (case, document, denied) in rows {
        for (which, cluster) in [("unservable", &unservable), ("servable", &servable)] {
            let case = format!("{case} on the {which} cluster");
            // The resource a deny is scoped to is the cluster the policy is
            // attached to, so each row is stored on each cluster in turn.
            let scoped = {
                let mut scoped = document.clone();
                scoped["Statement"][0]["Resource"] = json!(cluster);
                scoped
            };
            for (scope, document) in [("any resource", &document), ("this cluster", &scoped)] {
                let case = format!("{case}, scoped to {scope}");
                put_policy(&emulator, &case, cluster, document).await;
                let request = ConnectionRequest::to(cluster);
                let reply = emulator.create_vpc_connection(&request).await;
                if denied {
                    assert_access_denied(&reply, &case, &explicit_deny_message(action, cluster));
                } else if which == "unservable" {
                    // Not denied: the call reaches the precondition it would
                    // have reached with no policy at all.
                    assert_target_refusal(&reply, &case, &not_enabled_message("SASL_IAM"));
                } else {
                    assert_eq!(
                        reply.status, 200,
                        "case {case}: expected the create to be served, got {} {}",
                        reply.status, reply.body
                    );
                }
                delete_policy(&emulator, &case, cluster).await;

                // Once the policy is gone the answer is the one the cluster's own
                // configuration gives, whichever answer the policy produced.
                let after = emulator.create_vpc_connection(&request).await;
                if which == "unservable" {
                    assert_target_refusal(
                        &after,
                        &format!("{case}, after the delete"),
                        &not_enabled_message("SASL_IAM"),
                    );
                } else {
                    assert_eq!(
                        after.status, 200,
                        "case {case}, after the delete: expected the create to be served, got {} {}",
                        after.status, after.body
                    );
                }
            }
        }
    }

    // Nothing the policy refused was registered: the servable cluster holds one
    // connection per call that was actually served.
    let listed = client_connection_count(&emulator, "registered", &servable).await;
    assert!(
        listed > 0,
        "the served creates must have registered their connections, got {listed}"
    );
    assert_eq!(
        client_connection_count(&emulator, "registered", &unservable).await,
        0,
        "no call against the unservable cluster was ever served, so it must hold no connection"
    );
}

/// RULE: the same decision on the other actions an MSK cluster policy governs —
/// `GetBootstrapBrokers`, `DescribeCluster` and `DescribeClusterV2` — each refused
/// with its own IAM action in the sentence, and each served again once the policy
/// is deleted.
#[tokio::test]
async fn an_explicit_deny_in_the_cluster_policy_refuses_the_readers_it_governs() {
    let emulator = Emulator::new();
    let cluster = emulator.vpc_connection_target("policy-readers").await;

    // (the API name, the call the row drives)
    let readers: Vec<(&str, &str, String)> = vec![
        (
            "DescribeCluster",
            "GET",
            format!("/v1/clusters/{}", pct(&cluster)),
        ),
        (
            "DescribeClusterV2",
            "GET",
            format!("/api/v2/clusters/{}", pct(&cluster)),
        ),
        (
            "GetBootstrapBrokers",
            "GET",
            format!("/v1/clusters/{}/bootstrap-brokers", pct(&cluster)),
        ),
    ];

    // The rows are the whole published set, so an action added to or removed from
    // what a cluster policy governs is a row this suite demands rather than one it
    // can quietly stop driving: the four are these three readers plus the create
    // the suites above drive.
    let mut covered: BTreeSet<&str> = readers.iter().map(|(api, _, _)| *api).collect();
    covered.insert("CreateVpcConnection");
    let governed: BTreeSet<&str> = CLUSTER_POLICY_ACTIONS.iter().copied().collect();
    assert_eq!(
        covered, governed,
        "every action a cluster policy governs must be driven here or by the create suites, got \
         {covered:?} against {governed:?}"
    );

    for (api, method, uri) in &readers {
        let action = format!("kafka:{api}");
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Deny",
                "Principal": { "AWS": caller_principal() },
                "Action": [action.clone()],
                "Resource": cluster,
            }],
        });
        put_policy(&emulator, api, &cluster, &document).await;

        let refused = emulator.call(method, uri, None).await;
        assert_access_denied(&refused, api, &explicit_deny_message(&action, &cluster));

        // The deny names ONE action, so the other readers are untouched by it.
        for (other, other_method, other_uri) in &readers {
            if other == api {
                continue;
            }
            let reply = emulator.call(other_method, other_uri, None).await;
            assert_eq!(
                reply.status, 200,
                "case {api} denied: expected {other} to be served, got {} {}",
                reply.status, reply.body
            );
        }

        delete_policy(&emulator, api, &cluster).await;
        let served = emulator.call(method, uri, None).await;
        assert_eq!(
            served.status, 200,
            "case {api}: expected the reader to be served once the policy is gone, got {} {}",
            served.status, served.body
        );
    }
}

/// RULE: `CreateVpcConnection`'s authorization decision is taken on the resource
/// the request NAMES, before that target is resolved or parsed, so a
/// `TargetClusterArn` the caller holds no grant over is refused HTTP 403
/// `AccessDeniedException` naming the principal, the IAM action and the resource
/// the decision was taken over — the wildcard when the value names no resource at
/// all — and never a parameter complaint about the value's shape.
#[tokio::test]
async fn a_target_that_names_no_cluster_of_this_account_is_an_authorization_refusal() {
    let emulator = Emulator::new();
    let action = "kafka:CreateVpcConnection";

    // The rows differ along every way a value can fail to name a cluster of this
    // account: not an ARN at all (free text, empty, a truncated ARN, a very long
    // opaque value, a non-ASCII one), an ARN of another vendor, an ARN of another
    // resource kind of this service, and a cluster ARN missing its id segment.
    // The resource the refusal names is the value where the value is an ARN —
    // which IAM matched against the caller's statements and none covered — and the
    // wildcard where it is not, where there was nothing to match at all.
    let wildcard = "*";
    let long = format!("not-a-cluster-arn-{}", "x".repeat(200));
    let configuration = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/a-configuration/{GHOST_ID}"
    );
    let instance = format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0a1b2c3d");
    let nameless = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/only-a-name");
    let rows: Vec<(&str, String, &str)> = vec![
        ("not-an-arn-at-all", "not-a-cluster-arn".to_string(), wildcard),
        ("free-text-with-spaces", "not an arn at all".to_string(), wildcard),
        ("empty", String::new(), wildcard),
        ("truncated-arn", "arn:aws:kafka".to_string(), wildcard),
        ("a-long-opaque-value", long.clone(), wildcard),
        ("a-non-ascii-value", "ニセのクラスター-arn".to_string(), wildcard),
        ("another-vendor", instance.clone(), instance.as_str()),
        (
            "another-resource-kind-of-this-service",
            configuration.clone(),
            configuration.as_str(),
        ),
        (
            "a-cluster-arn-without-its-id-segment",
            nameless.clone(),
            nameless.as_str(),
        ),
    ];

    for (case, value, resource) in &rows {
        // A value that READS as an ARN is a resource the decision was taken over
        // and nothing granted, which IAM's default-deny clause says; one that
        // reads as no resource was decided over the wildcard, where no document
        // was reached to be named.
        let expected = if *resource == wildcard {
            not_authorized(action, resource)
        } else {
            format!(
                "{} because no identity-based policy allows the {action} action",
                not_authorized(action, resource)
            )
        };
        let request = ConnectionRequest {
            target_cluster_arn: value.clone(),
            ..ConnectionRequest::to("")
        };
        let reply = emulator.create_vpc_connection(&request).await;
        assert_access_denied(&reply, case, &expected);

        // The decision precedes the body, so the same refusal comes back for a
        // request whose other members are unusable too.
        let unusable = ConnectionRequest {
            target_cluster_arn: value.clone(),
            authentication: "SASL",
            vpc_id: "not-a-vpc",
            client_subnets: Vec::new(),
            security_groups: Vec::new(),
            ..ConnectionRequest::to("")
        };
        let reply = emulator.create_vpc_connection(&unusable).await;
        assert_access_denied(&reply, &format!("{case} with an unusable body"), &expected);
    }

    // A cluster ARN of ANOTHER account does name a resource, and the account that
    // holds it is the one whose resource-based policy could have allowed the
    // call: that denial is the resource-policy sentence, not this one.
    let foreign = format!("arn:aws:kafka:{REGION}:111122223333:cluster/foreign/{GHOST_ID}");
    let reply = emulator
        .create_vpc_connection(&ConnectionRequest {
            target_cluster_arn: foreign.clone(),
            ..ConnectionRequest::to("")
        })
        .await;
    assert_access_denied(
        &reply,
        "another account's cluster",
        &format!(
            "{} because no resource-based policy allows the {action} action",
            not_authorized(action, &foreign)
        ),
    );

    // A well-formed cluster ARN of THIS account names a cluster the account could
    // hold, so it is resolved rather than denied: absent, it is the parameter
    // refusal; present and ready, the call is served. Nothing was registered by
    // any of the denials above.
    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");
    let reply = emulator
        .create_vpc_connection(&ConnectionRequest {
            target_cluster_arn: absent,
            ..ConnectionRequest::to("")
        })
        .await;
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case an absent cluster of this account: expected the parameter refusal, got {} {}",
        reply.status,
        reply.body
    );
    assert!(
        listed_connections(&emulator).await.is_empty(),
        "no refused create may register a connection"
    );

    let served = emulator.vpc_connection_target("authorized-target").await;
    let reply = emulator
        .create_vpc_connection(&ConnectionRequest::to(&served))
        .await;
    assert_eq!(
        reply.status, 200,
        "case a cluster of this account: expected the create to be served, got {} {}",
        reply.status, reply.body
    );
}
