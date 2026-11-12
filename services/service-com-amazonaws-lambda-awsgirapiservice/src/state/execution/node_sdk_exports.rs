// SPDX-License-Identifier: CC0-1.0

//! What one service's own Smithy model states for an SDK CLIENT built against it: the names
//! that service's client package exports, and the input members a client of it fills in itself.
//!
//! A package of the AWS SDK for JavaScript v3 exports a class per thing its service has: the
//! client, a `<Operation>Command` for every operation the service binds, and one error class
//! per error shape it declares. A handler written against the SDK binds those names — `import
//! { SQSClient, GetQueueUrlCommand, QueueDoesNotExist } from '@aws-sdk/client-sqs'` — and an
//! ES module's named imports are resolved when the module is LINKED, so the package has to
//! state the names before any of them is read. That list is what this module computes, along
//! with the three other things a client reads out of the same file: which error shapes its
//! exception classes are built from (`crate::state::execution::aws_sdk_guest`, the Python SDK,
//! asks the same question), which members it generates a value for when the caller leaves
//! them out (`crate::state::execution::idempotency_token`) and how a paginator of it pages one
//! operation through (the same Python SDK's `client.get_paginator`).
//!
//! It states none of those itself: they are read out of the target service's own Smithy
//! model — the same file that service's emulator crate is generated from, embedded by
//! [`crate::state::execution::aws_api`] — so a command, an error class or a filled member this
//! runtime supplies for a service is one that service really has, and none can drift from the
//! surface it serves. Every question is answered from ONE reading of that model, held per
//! service.

use ::std::collections::{BTreeMap, BTreeSet};
use ::std::sync::{Arc, OnceLock};

use ::serde_json::Value;

use crate::state::execution::aws_api;

/// What one service's model states for a client of it, beside the client class and the base
/// exception its own name gives ([`super`]).
#[derive(Debug, Default)]
pub struct ClientExports {
    /// The operations the service binds, each of which the package exports a
    /// `<Operation>Command` for.
    pub operations: Vec<String>,
    /// The error shapes the service declares, each of which the package exports a class of
    /// the shape's own name for.
    pub errors: Vec<String>,
    /// The members of each operation's input that the model marks as an idempotency token,
    /// keyed by the operation's own name — the members a client generates a value for. An
    /// operation with none is not held at all.
    pub idempotency_tokens: BTreeMap<String, Vec<String>>,
    /// How each operation the model marks as paginated is paged through, keyed by the
    /// operation's own name — what a client's paginator for it drives ([`Pagination`]). An
    /// operation the model does not mark is not held at all, which is what makes a client
    /// report it as one that cannot be paginated.
    pub pagination: BTreeMap<String, Pagination>,
}

/// How ONE operation is paged through, as its own model states it (`@paginated`).
///
/// The members are the ones an SDK's paginator drives: the input member the continuation
/// value is sent back in, the output member that value arrives in, the input member a page's
/// size is asked for in, and the output members a page's own results arrive in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pagination {
    /// The input member the value continuing a listing is sent in (`inputToken`).
    pub input_token: String,
    /// The output member that value arrives in (`outputToken`). A page carrying no value
    /// here is the last one.
    pub output_token: String,
    /// The input member the size of one page is asked for in (`pageSize`), where the
    /// operation has one.
    pub page_size: Option<String>,
    /// The output members one page's own results arrive in: the member the model names as
    /// the paged items (`items`) where it names one, and otherwise every member of the
    /// output the model types as a LIST — the collections a page carries.
    pub result_keys: Vec<String>,
}

/// What the model of the service `token` names states for a client of it, for a service whose
/// model this emulator holds; nothing at all for a service it holds none for, whose package
/// therefore exports its client alone.
///
/// What is held between calls is one cell per entry of the reachable-services table
/// (`aws_api::APIS`), so the memory this holds and the number of models it parses are bounded
/// by that fixed table and not by anything a run asks for: a token no entry of it answers to
/// parses nothing and holds nothing, however many distinct ones a handler names.
#[must_use]
pub fn client_exports(token: &str) -> Arc<ClientExports> {
    /// The answer for a service this emulator holds no model for — one value, shared.
    static NONE_HELD: OnceLock<Arc<ClientExports>> = OnceLock::new();
    let none_held = || Arc::clone(NONE_HELD.get_or_init(|| Arc::new(ClientExports::default())));
    /// One cell per entry of that table, each filled on the first question asked about that
    /// entry's service and never again.
    static CACHE: OnceLock<Vec<OnceLock<Arc<ClientExports>>>> = OnceLock::new();
    let Some((at, model)) = aws_api::held_model(token) else {
        return none_held();
    };
    let cache = CACHE.get_or_init(|| aws_api::APIS.iter().map(|_| OnceLock::new()).collect());
    let Some(cell) = cache.get(at) else {
        return none_held();
    };
    Arc::clone(cell.get_or_init(|| Arc::new(exports_of(model))))
}

/// What `model` states: every operation shape it declares, every shape it marks as an error,
/// the idempotency-token members of each operation's input, and how each operation it marks as
/// paginated is paged through.
///
/// Reading the shapes rather than the service's own operation list takes in the operations a
/// resource's lifecycle binds as well as the ones the service lists directly, which is the
/// same set the SDK generates a command for.
fn exports_of(model: &str) -> ClientExports {
    let shapes = ::serde_json::from_str::<Value>(model)
        .ok()
        .and_then(|document| document.get("shapes").cloned());
    let Some(Value::Object(shapes)) = shapes else {
        return ClientExports::default();
    };
    let mut operations = BTreeSet::new();
    let mut errors = BTreeSet::new();
    let mut idempotency_tokens = BTreeMap::new();
    let mut pagination = BTreeMap::new();
    let defaults = service_pagination(&shapes);
    for (id, shape) in &shapes {
        let Some(name) = id.split('#').next_back().filter(|name| !name.is_empty()) else {
            continue;
        };
        match shape.get("type").and_then(Value::as_str) {
            Some("operation") => {
                operations.insert(name.to_string());
                let filled = idempotency_members(&shapes, shape);
                if !filled.is_empty() {
                    idempotency_tokens.insert(name.to_string(), filled);
                }
                if let Some(paged) = pagination_of(&shapes, shape, defaults.as_ref()) {
                    pagination.insert(name.to_string(), paged);
                }
            }
            Some("structure") if is_error(shape) => {
                errors.insert(name.to_string());
            }
            _ => {}
        }
    }
    ClientExports {
        operations: operations.into_iter().collect(),
        errors: errors.into_iter().collect(),
        idempotency_tokens,
        pagination,
    }
}

/// The pagination settings the SERVICE shape states, which every paginated operation of it
/// inherits the members it leaves out from.
fn service_pagination(shapes: &::serde_json::Map<String, Value>) -> Option<Value> {
    shapes
        .values()
        .find(|shape| shape.get("type").and_then(Value::as_str) == Some("service"))
        .and_then(|service| paginated_trait(service).cloned())
}

/// How the operation `operation` is paged through, or `None` for one the model does not mark
/// as paginated — which is what makes a client report that it cannot be paginated.
///
/// The settings are the operation's own, with `defaults` standing in for each member it
/// leaves out: that is how the trait resolves, and an operation carrying none of it at all is
/// not paginated however the service states its defaults.
fn pagination_of(
    shapes: &::serde_json::Map<String, Value>,
    operation: &Value,
    defaults: Option<&Value>,
) -> Option<Pagination> {
    let own = paginated_trait(operation)?;
    let stated = |member: &str| {
        own.get(member)
            .and_then(Value::as_str)
            .or_else(|| defaults.and_then(|held| held.get(member)).and_then(Value::as_str))
            .filter(|named| !named.is_empty())
    };
    let input_token = stated("inputToken")?.to_string();
    let output_token = stated("outputToken")?.to_string();
    let result_keys = match stated("items") {
        Some(items) => vec![items.to_string()],
        None => output_list_members(shapes, operation),
    };
    Some(Pagination {
        input_token,
        output_token,
        page_size: stated("pageSize").map(str::to_string),
        result_keys,
    })
}

/// The `@paginated` trait `shape` carries, if any.
fn paginated_trait(shape: &Value) -> Option<&Value> {
    shape.get("traits")?.get("smithy.api#paginated")
}

/// The members of the output of `operation` that the model types as a LIST — the collections
/// one page of that operation carries.
///
/// Sorted, so the answer is the same whatever order the model document is read in (a
/// `serde_json` map keeps its keys sorted or in the document's order, depending on a feature
/// anything in the build may turn on).
fn output_list_members(
    shapes: &::serde_json::Map<String, Value>,
    operation: &Value,
) -> Vec<String> {
    let Some(members) = operation
        .get("output")
        .and_then(|output| output.get("target"))
        .and_then(Value::as_str)
        .and_then(|target| shapes.get(target))
        .and_then(|output| output.get("members"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    members
        .iter()
        .filter(|(_name, member)| {
            member
                .get("target")
                .and_then(Value::as_str)
                .and_then(|target| shapes.get(target))
                .and_then(|target| target.get("type"))
                .and_then(Value::as_str)
                == Some("list")
        })
        .map(|(name, _member)| name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The members of the input of the operation `operation` that the model marks as an idempotency
/// token.
fn idempotency_members(
    shapes: &::serde_json::Map<String, Value>,
    operation: &Value,
) -> Vec<String> {
    operation
        .get("input")
        .and_then(|input| input.get("target"))
        .and_then(Value::as_str)
        .and_then(|target| shapes.get(target))
        .and_then(|input| input.get("members"))
        .and_then(Value::as_object)
        .map(|members| {
            members
                .iter()
                .filter(|(_name, member)| is_idempotency_token(member))
                .map(|(name, _member)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `shape` is one the model marks as an error (`@error`), which is what makes the SDK
/// export a class for it.
fn is_error(shape: &Value) -> bool {
    has_trait(shape, "smithy.api#error")
}

/// Whether `member` is one the model marks as an idempotency token (`@idempotencyToken`), which
/// is what makes an SDK client generate a value for it.
fn is_idempotency_token(member: &Value) -> bool {
    has_trait(member, "smithy.api#idempotencyToken")
}

/// Whether `shape` carries the trait `trait_id`.
fn has_trait(shape: &Value, trait_id: &str) -> bool {
    shape
        .get("traits")
        .and_then(Value::as_object)
        .is_some_and(|traits| traits.contains_key(trait_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a client package exports are the service's own model's: one row per service
    /// a call may be made to, each asserted on an operation and an error shape of that
    /// service's model, and a row for a service whose model this emulator does not hold.
    #[test]
    fn a_packages_exported_names_are_its_services_own() {
        // Each row: the package's token, an operation the service's model declares, and an
        // error shape it declares.
        let cases: &[(&str, &str, &str)] = &[
            ("sqs", "GetQueueUrl", "QueueDoesNotExist"),
            ("sqs", "SendMessage", "InvalidMessageContents"),
            ("dynamodb", "PutItem", "ResourceNotFoundException"),
            ("eventbridge", "PutEvents", "ResourceNotFoundException"),
            ("eventbridge", "PutRule", "ConcurrentModificationException"),
            // The SDK publishes CloudWatch Logs as `client-cloudwatch-logs`, whose token is
            // the same service's as `cloudwatchlogs`.
            ("cloudwatch-logs", "PutLogEvents", "ResourceNotFoundException"),
            ("secretsmanager", "GetSecretValue", "ResourceNotFoundException"),
            ("apigateway", "GetRestApis", "NotFoundException"),
            ("sesv2", "SendEmail", "MessageRejected"),
            ("s3", "PutObject", "NoSuchKey"),
            ("sfn", "StartExecution", "ExecutionDoesNotExist"),
            // The SDK publishes the RDS Data API as `client-rds-data`, whose token is the
            // same service's as `rdsdata`.
            ("rds-data", "ExecuteStatement", "HttpEndpointNotEnabledException"),
            ("rdsdata", "BeginTransaction", "AccessDeniedException"),
        ];
        for (token, operation, error) in cases {
            let exports = client_exports(token);
            assert!(
                exports.operations.iter().any(|held| held == operation),
                "case {token:?}: expected the operation {operation:?} among the \
                 {} the model declares, got none of it",
                exports.operations.len()
            );
            assert!(
                exports.errors.iter().any(|held| held == error),
                "case {token:?}: expected the error shape {error:?} among the {} the model \
                 declares, got none of it",
                exports.errors.len()
            );
        }
        for token in ["kinesis", "", "not-a-service"] {
            let exports = client_exports(token);
            assert!(
                exports.operations.is_empty() && exports.errors.is_empty(),
                "case {token:?}: this emulator holds no model for that service, so it states \
                 no name of one, got {} operations and {} errors",
                exports.operations.len(),
                exports.errors.len()
            );
        }
    }

    /// How an operation is paged through is read off its own `@paginated` trait, and an
    /// operation the model does not mark is not paged at all.
    ///
    /// The rows differ along what the trait states: one naming the paged items, one naming
    /// none (where the page's results are the output's own list members), one whose
    /// continuation value is a map rather than text, one whose input and output name the same
    /// member, and one the model does not mark.
    #[test]
    fn how_an_operation_is_paged_through_is_its_models_own() {
        // Each row: the service token, the operation, and what its model states.
        let cases: &[(&str, &str, Option<Pagination>)] = &[
            (
                "s3",
                "ListObjectsV2",
                Some(Pagination {
                    input_token: "ContinuationToken".to_string(),
                    output_token: "NextContinuationToken".to_string(),
                    page_size: Some("MaxKeys".to_string()),
                    // The trait names no items member, so a page's results are the output's
                    // own list members.
                    result_keys: vec!["CommonPrefixes".to_string(), "Contents".to_string()],
                }),
            ),
            (
                "dynamodb",
                "Scan",
                Some(Pagination {
                    input_token: "ExclusiveStartKey".to_string(),
                    output_token: "LastEvaluatedKey".to_string(),
                    page_size: Some("Limit".to_string()),
                    result_keys: vec!["Items".to_string()],
                }),
            ),
            (
                "sqs",
                "ListQueues",
                Some(Pagination {
                    input_token: "NextToken".to_string(),
                    output_token: "NextToken".to_string(),
                    page_size: Some("MaxResults".to_string()),
                    result_keys: vec!["QueueUrls".to_string()],
                }),
            ),
            ("s3", "GetObject", None),
            ("sqs", "SendMessage", None),
            // A service this emulator holds no model for states nothing about paging either.
            ("not-a-service", "ListThings", None),
        ];
        for (token, operation, expected) in cases {
            let held = client_exports(token).pagination.get(*operation).cloned();
            assert_eq!(
                held.as_ref(),
                expected.as_ref(),
                "case {token:?}/{operation:?}: expected {expected:?}, got {held:?}"
            );
        }
    }

    /// The same token asked twice is answered from the one reading of the model, and a token
    /// no service answers to holds nothing between calls — whatever spelling it is written
    /// in, since what is held is one cell per service and not one per name asked for.
    #[test]
    fn the_model_is_read_once_per_service() {
        let first = client_exports("sqs");
        let again = client_exports("sqs");
        assert!(
            Arc::ptr_eq(&first, &again),
            "expected the one reading of the model to answer both, got two"
        );
        // The two spellings of one token are the one service, so they share that cell.
        assert!(
            Arc::ptr_eq(&client_exports("cloudwatch-logs"), &client_exports("cloudwatchlogs")),
            "expected both spellings of the token to be the one service's cell"
        );
        let unknown = client_exports("not-a-service");
        assert!(
            Arc::ptr_eq(&unknown, &client_exports("another-absent-service")),
            "expected a service this emulator holds no model for to hold nothing of its own, \
             got a second value"
        );
    }
}
