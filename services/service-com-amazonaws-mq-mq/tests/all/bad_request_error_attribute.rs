// SPDX-License-Identifier: CC0-1.0

//! How Amazon MQ refuses an input, and the one precondition `CreateBroker` enforces
//! that the model does not express.
//!
//! RULE 1: every request this service refuses because of a MEMBER of that request is
//! answered with the modelled `BadRequestException` — HTTP 400, `x-amzn-errortype:
//! BadRequestException` — whose `errorAttribute` carries that member's wire (jsonName)
//! spelling (`smithy_query(query="shape", shape="BadRequestException")`:
//! `ErrorAttribute`, jsonName `errorAttribute`, "The attribute which caused the error").
//! That holds for a value outside a set the MODEL declares just as it does for a rule
//! only the service knows, so no refusal of a member may fall through to the
//! framework's generic constraint envelope, which carries code and message only.
//!
//! RULE 2: a broker is created together with its initial user, so a `CreateBroker` that
//! names no user — an absent `users` member or an empty list — is refused under rule 1
//! naming `users`, and no broker is stored.
//!
//! RULE 3: the sentence a refusal carries is Amazon MQ's own for that failure, built
//! from the request wherever it names a value: the rejected enum value with the set the
//! model declares, the `BrokerId` that resolved to nothing, the `NextToken` the request
//! sent. Every operation stating the same failure states it identically.
//!
//! The tables span the classes the rules generalise over: every enum-typed input member
//! of the operations that are reachable (not just the one the issue cited), each with a
//! value outside its set built from the set itself rather than from one recorded
//! request; the published bounds of `maxResults` and one past each; and, for rule 2,
//! both ways of naming no user crossed with the engine type and authentication strategy
//! the request asks for, plus the accepted side of the bound.
//!
//! The value catalogs are the model's own enumerations, re-exported by the crate
//! (`smithy_query(query="shape",
//! shape="EngineType,DeploymentMode,BrokerStorageType,AuthenticationStrategy,DataReplicationMode")`),
//! never a list invented here. The wire harness is shared with
//! [`crate::broker_lifecycle`], so every call goes through `MultiServiceRouter::dispatch`
//! and the error code each row asserts is the one a restJson1 client branches on
//! (`x-amzn-errortype`, falling back to the body's `__type`).

use serde_json::{json, Value};

use service_com_amazonaws_mq_mq::ops::types::{
    AUTHENTICATION_STRATEGY_VALUES, BROKER_STORAGE_TYPE_VALUES, DATA_REPLICATION_MODE_VALUES,
    DEPLOYMENT_MODE_VALUES, ENGINE_TYPE_VALUES,
};
use service_com_amazonaws_mq_mq::state::{
    MAX_MAX_RESULTS, MAX_USERS_PER_BROKER, MIN_MAX_RESULTS, MIN_USERS_PER_BROKER,
};

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;
use crate::configuration_wire_form::engine_type_on_the_wire;

/// One input member whose modelled value set this service refuses itself: the
/// operation that carries it, the member's `jsonName`, and the set the model declares.
struct EnumMember {
    operation: &'static str,
    attribute: &'static str,
    allowed: &'static [&'static str],
}

/// Every enum-typed input member of the operations a caller can reach today —
/// `CreateBroker` and `CreateConfiguration`.
const ENUM_MEMBERS: &[EnumMember] = &[
    EnumMember {
        operation: "CreateBroker",
        attribute: "engineType",
        allowed: ENGINE_TYPE_VALUES,
    },
    EnumMember {
        operation: "CreateBroker",
        attribute: "deploymentMode",
        allowed: DEPLOYMENT_MODE_VALUES,
    },
    EnumMember {
        operation: "CreateBroker",
        attribute: "storageType",
        allowed: BROKER_STORAGE_TYPE_VALUES,
    },
    EnumMember {
        operation: "CreateBroker",
        attribute: "authenticationStrategy",
        allowed: AUTHENTICATION_STRATEGY_VALUES,
    },
    EnumMember {
        operation: "CreateBroker",
        attribute: "dataReplicationMode",
        allowed: DATA_REPLICATION_MODE_VALUES,
    },
    EnumMember {
        operation: "CreateConfiguration",
        attribute: "engineType",
        allowed: ENGINE_TYPE_VALUES,
    },
    EnumMember {
        operation: "CreateConfiguration",
        attribute: "authenticationStrategy",
        allowed: AUTHENTICATION_STRATEGY_VALUES,
    },
];

/// The request `operation` is refused for, with `attribute` set to `value`.
fn body_with(operation: &str, name: &str, attribute: &str, value: &str) -> (&'static str, Value) {
    let mut body = match operation {
        "CreateBroker" => create_body(name),
        "CreateConfiguration" => configuration_body(name),
        other => panic!("case {other:?}: this table has no request builder for that operation"),
    };
    body[attribute] = json!(value);
    let path = match operation {
        "CreateBroker" => "/v1/brokers",
        _ => "/v1/configurations",
    };
    (path, body)
}

/// Values outside `allowed` that are built FROM the declared set rather than copied
/// from any one request: each declared value in the wrong case, a value no enumeration
/// in this model holds, and the empty string.
fn values_outside(allowed: &[&str]) -> Vec<String> {
    let mut outside: Vec<String> = allowed.iter().map(|value| value.to_lowercase()).collect();
    outside.push("NOTAREALVALUE".to_string());
    outside.push(String::new());
    outside
}

/// RULE 1 over every enum-typed input member: the refusal is a `BadRequestException`
/// naming that member, never the framework's generic constraint envelope.
#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_naming_the_member() {
    let mq = Mq::new("us-east-1");
    let mut rows = 0usize;

    for member in ENUM_MEMBERS {
        for (index, value) in values_outside(member.allowed).into_iter().enumerate() {
            let case = format!(
                "{}.{} = {value:?}",
                member.operation, member.attribute
            );
            let (path, body) = body_with(
                member.operation,
                &format!("enum-{}-{}-{index}", member.operation, member.attribute),
                member.attribute,
                &value,
            );
            let (status, code, answer) = mq.call_with_code("POST", path, Some(body)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {answer:?}"
            );
            assert_eq!(
                code, "BadRequestException",
                "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
            );
            assert_eq!(
                answer.pointer("/errorAttribute").and_then(Value::as_str),
                Some(member.attribute),
                "case {case:?}: expected errorAttribute {:?}, got {answer:?}",
                member.attribute
            );
            // The sentence Amazon MQ states such a refusal with: the member's
            // subject, the rejected value in brackets, then the set it accepts.
            // The subject is the member's own documented description, so the row
            // checks the parts the rule builds from the request and the model.
            let message = string_at(&answer, "/message");
            assert!(
                message.contains(&format!("[{value}] is invalid. Valid values are: [{}]", member.allowed.join(", "))),
                "case {case:?}: the message must name the rejected value and the accepted \
                 set, got {message:?}"
            );
            rows += 1;
        }
    }

    assert_eq!(
        rows,
        ENUM_MEMBERS
            .iter()
            .map(|member| member.allowed.len() + 2)
            .sum::<usize>(),
        "every member must be exercised against every value outside its set, covered {rows}"
    );

    // A refused create stores nothing.
    let (_, listed) = mq.list_brokers("").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// A value INSIDE every modelled set is not refused by these checks: the resource is
/// created and reads back — the input check did not become a refusal of every request.
#[tokio::test]
async fn a_value_inside_every_modelled_enum_is_accepted() {
    let mq = Mq::new("us-east-1");

    for (index, engine_type) in ENGINE_TYPE_VALUES.iter().enumerate() {
        for strategy in AUTHENTICATION_STRATEGY_VALUES {
            for mode in DATA_REPLICATION_MODE_VALUES {
                let case = format!("{engine_type}/{strategy}/{mode}");
                let name = format!("inside-{index}-{strategy}-{mode}").to_lowercase();
                let mut body = create_body(&name);
                body["engineType"] = json!(engine_type);
                body["authenticationStrategy"] = json!(strategy);
                body["dataReplicationMode"] = json!(mode);
                if *mode == "CRDR" {
                    // "Must be set when dataReplicationMode is set to CRDR".
                    body["dataReplicationPrimaryBrokerArn"] =
                        json!("arn:aws:mq:us-east-1:000000000000:broker:primary:b-1");
                }
                let (status, created) = mq.create_broker(body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} {created:?}"
                );
                let broker_id = string_at(&created, "/brokerId");
                let (status, described) = mq.describe_broker(&broker_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the broker to read back, got {status} {described:?}"
                );
                assert_eq!(
                    string_at(&described, "/engineType"),
                    engine_type_on_the_wire(engine_type),
                    "case {case:?}: expected the stored engineType back, in the spelling \
                     Amazon MQ answers it in, got {described:?}"
                );
            }
        }
    }

    for (index, engine_type) in ENGINE_TYPE_VALUES.iter().enumerate() {
        for strategy in AUTHENTICATION_STRATEGY_VALUES {
            let case = format!("CreateConfiguration {engine_type}/{strategy}");
            // A configuration name is unique per account, so each row names its own.
            let name = format!("valid-configuration-{index}-{strategy}");
            let mut body = configuration_body(&name);
            body["engineType"] = json!(engine_type);
            body["authenticationStrategy"] = json!(strategy);
            let (status, code, answer) = mq
                .call_with_code("POST", "/v1/configurations", Some(body))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a valid request must be served, got {status} {code:?} \
                 {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/name"),
                name,
                "case {case:?}: expected the name sent back, got {answer:?}"
            );
        }
    }
}

/// RULE 1 on `maxResults`, whose bound is a modelled `@range` rather than a value set:
/// each published bound is accepted and one past each is refused naming the member.
#[tokio::test]
async fn max_results_outside_its_published_range_is_refused_naming_the_member() {
    let mq = Mq::new("us-east-1");
    let (status, created) = mq.create_broker(create_body("paged")).await;
    assert_eq!(status, 200, "setup: expected 200, got {status} {created:?}");

    // One past each published bound, and further outside each.
    for value in [
        MIN_MAX_RESULTS - 1,
        MAX_MAX_RESULTS + 1,
        -1,
        MAX_MAX_RESULTS * 10,
    ] {
        let case = format!("maxResults={value}");
        let (status, code, answer) = mq
            .call_with_code("GET", &format!("/v1/brokers?maxResults={value}"), None)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("maxResults"),
            "case {case:?}: expected errorAttribute \"maxResults\", got {answer:?}"
        );
    }

    // Both published bounds, and a value between them, are served.
    for value in [MIN_MAX_RESULTS, MIN_MAX_RESULTS + 1, MAX_MAX_RESULTS] {
        let case = format!("maxResults={value}");
        let (status, listed) = mq.list_brokers(&format!("maxResults={value}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value inside the range must be served, got {status} {listed:?}"
        );
        assert_eq!(
            listed
                .pointer("/brokerSummaries")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1),
            "case {case:?}: expected the one stored broker, got {listed:?}"
        );
    }
}

/// Every operation that resolves a `BrokerId` reports an id with no broker behind
/// it the same way: `NotFoundException` (404) whose sentence is built from the id the
/// request named. One sentence, every handler that looks a broker up.
#[tokio::test]
async fn a_broker_id_with_no_broker_behind_it_reads_the_same_on_every_operation() {
    let mq = Mq::new("us-east-1");

    // Rows: the forms a `BrokerId` can take that resolve to nothing (a label, a
    // minted-looking id, a bare prefix, one that was never minted at all).
    for broker_id in [
        "b-does-not-exist",
        "b-00000000-0000-0000-0000-000000000000",
        "b-",
        "not-even-a-broker-id",
    ] {
        // Every implemented operation that resolves `BrokerId`.
        let calls: Vec<(&str, (u16, String, Value))> = vec![
            (
                "DescribeBroker",
                mq.call_with_code("GET", &format!("/v1/brokers/{broker_id}"), None)
                    .await,
            ),
            (
                "DeleteBroker",
                mq.call_with_code("DELETE", &format!("/v1/brokers/{broker_id}"), None)
                    .await,
            ),
            (
                "DescribeUser",
                mq.call_with_code(
                    "GET",
                    &format!("/v1/brokers/{broker_id}/users/someone"),
                    None,
                )
                .await,
            ),
            (
                "CreateUser",
                mq.call_with_code(
                    "POST",
                    &format!("/v1/brokers/{broker_id}/users/someone"),
                    Some(json!({"password": "GoodPassword12"})),
                )
                .await,
            ),
        ];

        for (operation, (status, code, answer)) in calls {
            let case = format!("{operation} on {broker_id:?}");
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404, got {status} {answer:?}"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException, got {code:?} {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/message"),
                format!(
                    "Cannot find broker ID [{broker_id}]. Verify that your broker exists or \
                     check broker permissions."
                ),
                "case {case:?}: the sentence must name the requested id, got {answer:?}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is refused naming `nextToken`, with the
/// sentence built from the token the request sent.
#[tokio::test]
async fn a_foreign_next_token_is_refused_naming_the_member() {
    let mq = Mq::new("us-east-1");

    // Rows: the forms a token that is not this service's can take — a plain
    // label, this listing's tag with no digits, the tag with the wrong digit
    // count, and a bare number.
    for token in ["not-a-valid-token", "mqb1", "mqb1abc", "7"] {
        let case = format!("nextToken={token}");
        let (status, code, answer) = mq
            .call_with_code("GET", &format!("/v1/brokers?nextToken={token}"), None)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            code, "BadRequestException",
            "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
        );
        assert_eq!(
            answer.pointer("/errorAttribute").and_then(Value::as_str),
            Some("nextToken"),
            "case {case:?}: expected errorAttribute \"nextToken\", got {answer:?}"
        );
        assert_eq!(
            string_at(&answer, "/message"),
            format!("Invalid next token [{token}]. Make sure you copied the token correctly."),
            "case {case:?}: the sentence must name the token sent, got {answer:?}"
        );
    }
}

/// RULE 2: a `CreateBroker` that names no user is refused naming `users`, whatever
/// else the request asks for, and nothing is stored.
#[tokio::test]
async fn create_broker_refuses_a_request_that_names_no_user() {
    let mq = Mq::new("eu-west-1");
    let mut rows = 0usize;

    // The two ways of naming no user, crossed with the engine type and the
    // authentication strategy — the rule does not depend on either.
    for (how, users) in [
        ("users absent", None),
        ("users empty", Some(json!([]))),
    ] {
        for engine_type in ENGINE_TYPE_VALUES {
            for strategy in AUTHENTICATION_STRATEGY_VALUES {
                let case = format!("{how} on {engine_type} with {strategy}");
                let mut body = create_body(&format!(
                    "no-users-{}-{engine_type}-{strategy}",
                    rows
                ));
                body["engineType"] = json!(engine_type);
                body["authenticationStrategy"] = json!(strategy);
                match &users {
                    None => {
                        body.as_object_mut().expect("object").remove("users");
                    }
                    Some(empty) => body["users"] = empty.clone(),
                }

                let (status, code, answer) =
                    mq.call_with_code("POST", "/v1/brokers", Some(body)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status} {answer:?}"
                );
                assert_eq!(
                    code, "BadRequestException",
                    "case {case:?}: expected BadRequestException, got {code:?} {answer:?}"
                );
                assert_eq!(
                    answer.pointer("/errorAttribute").and_then(Value::as_str),
                    Some("users"),
                    "case {case:?}: expected errorAttribute \"users\", got {answer:?}"
                );
                let message = string_at(&answer, "/message");
                assert_eq!(
                    message, "Specify at least one user.",
                    "case {case:?}: expected Amazon MQ's sentence for the missing input, \
                     got {message:?}"
                );
                rows += 1;
            }
        }
    }

    assert_eq!(
        rows,
        2 * ENGINE_TYPE_VALUES.len() * AUTHENTICATION_STRATEGY_VALUES.len(),
        "the table must cover both ways of naming no user on every engine and strategy, \
         covered {rows}"
    );

    let (_, listed) = mq.list_brokers("maxResults=100").await;
    assert_eq!(
        listed
            .pointer("/brokerSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused create must store no broker, got {listed:?}"
    );
}

/// The accepted side of rule 2: from the published minimum of one user up to the
/// per-broker quota, every count is created and reads back; one past the quota is
/// refused naming the same member.
#[tokio::test]
async fn create_broker_accepts_from_one_user_up_to_the_quota() {
    let mq = Mq::new("eu-west-1");

    let users = |count: usize| -> Value {
        json!((0..count)
            .map(|index| json!({
                "username": format!("user-{index}"),
                "password": format!("GoodPassword{index:04}"),
            }))
            .collect::<Vec<Value>>())
    };

    for count in [
        MIN_USERS_PER_BROKER,
        MIN_USERS_PER_BROKER + 1,
        7,
        MAX_USERS_PER_BROKER,
    ] {
        let case = format!("{count} users");
        let mut body = create_body(&format!("users-{count}"));
        body["users"] = users(count);
        let (status, created) = mq.create_broker(body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {created:?}"
        );

        let broker_id = string_at(&created, "/brokerId");
        let (status, described) = mq.describe_broker(&broker_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the broker to read back, got {status} {described:?}"
        );
        assert_eq!(
            described
                .pointer("/users")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(count),
            "case {case:?}: expected {count} users back, got {described:?}"
        );
    }

    // One past the per-broker quota is refused under the same rule and member.
    let mut body = create_body("users-one-too-many");
    body["users"] = users(MAX_USERS_PER_BROKER + 1);
    let (status, code, answer) = mq.call_with_code("POST", "/v1/brokers", Some(body)).await;
    assert_eq!(
        status, 400,
        "one past the user quota: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        code, "BadRequestException",
        "one past the user quota: expected BadRequestException, got {code:?} {answer:?}"
    );
    assert_eq!(
        answer.pointer("/errorAttribute").and_then(Value::as_str),
        Some("users"),
        "one past the user quota: expected errorAttribute \"users\", got {answer:?}"
    );
}
