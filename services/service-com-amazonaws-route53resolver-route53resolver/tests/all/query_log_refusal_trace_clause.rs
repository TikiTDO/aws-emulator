// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the trace clause a Resolver query-logging refusal closes
//! its coded sentence with.
//!
//! What they pin is the rule every one of these refusals follows: the `Message`
//! of a refused query-logging call is the service's coded sentence for that
//! fault followed by ` Trace Id: "<Root>"`, where `<Root>` is the trace the
//! REQUEST named in its `X-Amzn-Trace-Id` header and one the service minted for
//! the request when the caller named none. So the clause is always there, it is
//! read from the request and never from anything the caller asked ABOUT, and a
//! chain the caller started is reported under that chain's own id.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`].

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::harness::{call_tracing, emulator, error_type, ok, string_member, traced_message};
use super::resolver_query_log_config_associations::{association, config_id};

/// A `ResolverQueryLogConfigId` no create ever minted, well formed for the
/// modelled `ResourceId` (length 1..64).
const UNKNOWN_CONFIG: &str = "rqlc-0123456789abcdef0";

/// A `ResolverQueryLogConfigAssociationId` no associate ever minted.
const UNKNOWN_ASSOCIATION: &str = "rqlca-0123456789abcdef0";

/// A VPC of the account every row names, so a refusal is about the query-logging
/// resource and never about the VPC.
const HELD_VPC: &str = "vpc-0123456789abcdef0";

/// A VPC of the account no configuration is associated with, which is what makes
/// a disassociate address an ASSOCIATION the store holds nothing under.
const UNASSOCIATED_VPC: &str = "vpc-00000000000000009";

/// The sentence a configuration the service holds nothing under is refused with:
/// fixed text behind that fault's service error code, naming no id.
const CONFIG_GONE_SENTENCE: &str =
    "[RSLVR-01601] The specified query logging configuration doesn't exist.";

/// The sentence an association the service holds nothing under is refused with,
/// whichever way the request addressed it.
const ASSOCIATION_GONE_SENTENCE: &str =
    "[RSLVR-01602] The specified query logging configuration association doesn't exist.";

/// One tracing header a request may carry, and the trace the refusal it gets
/// back has to be reported under.
struct Tracing {
    case: &'static str,
    /// The `X-Amzn-Trace-Id` the request carries, or `None` for a caller that
    /// sends none.
    header: Option<&'static str>,
    /// The `Root` the clause must carry, or `None` where the caller named no
    /// trace and the service mints one for the request.
    expected_root: Option<&'static str>,
}

/// The rows differ along what the rule generalises over: the tracing header the
/// REQUEST carries. A header naming a trace in X-Ray's own form has its `Root`
/// reported back whatever else the header carries beside it (the sampling
/// decision, the parent span, a field this service does not read, the fields in
/// another order); a caller that names no trace, or names something that is not
/// a trace id at all, has one minted for the request instead — the one reading
/// of that form the services of this emulator share
/// (`service_common::xray_trace`).
const TRACINGS: &[Tracing] = &[
    Tracing {
        case: "no tracing header at all",
        header: None,
        expected_root: None,
    },
    Tracing {
        case: "a root and the sampling decision",
        header: Some("Root=1-6a76e074-570e69780dc8a349626ecf87;Sampled=0"),
        expected_root: Some("1-6a76e074-570e69780dc8a349626ecf87"),
    },
    Tracing {
        case: "a root alone",
        header: Some("Root=1-6ae16e88-ff4aa3322da8497b848feb98"),
        expected_root: Some("1-6ae16e88-ff4aa3322da8497b848feb98"),
    },
    Tracing {
        case: "a root, a parent span and a sampled request",
        header: Some(
            "Root=1-6ae23e89-0624219a04ba4315978e3230;Parent=53995c3f42cd8ad8;Sampled=1",
        ),
        expected_root: Some("1-6ae23e89-0624219a04ba4315978e3230"),
    },
    Tracing {
        case: "a root behind a field this service does not read",
        header: Some("Self=1-11111111-222222222222222222222222;Root=1-6b000001-abcdefabcdefabcdefabcdef"),
        expected_root: Some("1-6b000001-abcdefabcdefabcdefabcdef"),
    },
    Tracing {
        case: "a header naming no root",
        header: Some("Sampled=0"),
        expected_root: None,
    },
    Tracing {
        case: "a root that is not a trace id",
        header: Some("Root=not-a-trace-id;Sampled=0"),
        expected_root: None,
    },
];

/// One refused query-logging call: the operation, the request, and the coded
/// sentence its refusal states before the trace clause.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: fn(&str) -> Value,
    sentence: &'static str,
    /// The modelled resource name the refusal reports beside the message.
    resource_type: &'static str,
}

/// Every query-logging path that refuses a resource it cannot address: each
/// operation that resolves a `ResolverQueryLogConfigId`, the read that resolves
/// an association by its own id, and the disassociate that addresses one by the
/// (configuration, VPC) pair it was made from — so both coded sentences of the
/// slice are covered, and the one operation that can answer either is asserted
/// for both.
///
/// `body` is handed the id of a configuration the store DOES hold, which the
/// pair rows need to get past the configuration lookup.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a read of a configuration the store holds nothing under",
        operation: "GetResolverQueryLogConfig",
        body: |_| json!({ "ResolverQueryLogConfigId": UNKNOWN_CONFIG }),
        sentence: CONFIG_GONE_SENTENCE,
        resource_type: "ResolverQueryLogConfig",
    },
    Refusal {
        case: "a delete of a configuration the store holds nothing under",
        operation: "DeleteResolverQueryLogConfig",
        body: |_| json!({ "ResolverQueryLogConfigId": UNKNOWN_CONFIG }),
        sentence: CONFIG_GONE_SENTENCE,
        resource_type: "ResolverQueryLogConfig",
    },
    Refusal {
        case: "an associate naming a configuration the store holds nothing under",
        operation: "AssociateResolverQueryLogConfig",
        body: |_| json!({ "ResolverQueryLogConfigId": UNKNOWN_CONFIG, "ResourceId": HELD_VPC }),
        sentence: CONFIG_GONE_SENTENCE,
        resource_type: "ResolverQueryLogConfig",
    },
    Refusal {
        case: "a disassociate naming a configuration the store holds nothing under",
        operation: "DisassociateResolverQueryLogConfig",
        body: |_| json!({ "ResolverQueryLogConfigId": UNKNOWN_CONFIG, "ResourceId": HELD_VPC }),
        sentence: CONFIG_GONE_SENTENCE,
        resource_type: "ResolverQueryLogConfig",
    },
    Refusal {
        case: "a read of an association the store holds nothing under",
        operation: "GetResolverQueryLogConfigAssociation",
        body: |_| json!({ "ResolverQueryLogConfigAssociationId": UNKNOWN_ASSOCIATION }),
        sentence: ASSOCIATION_GONE_SENTENCE,
        resource_type: "ResolverQueryLogConfigAssociation",
    },
    Refusal {
        case: "a disassociate of a pair the configuration holds no association for",
        operation: "DisassociateResolverQueryLogConfig",
        body: |stored| {
            json!({ "ResolverQueryLogConfigId": stored, "ResourceId": UNASSOCIATED_VPC })
        },
        sentence: ASSOCIATION_GONE_SENTENCE,
        resource_type: "ResolverQueryLogConfigAssociation",
    },
];

/// Every query-logging refusal states its coded sentence and closes it with the
/// trace clause of the request: the `Root` the caller named where it named one,
/// and a trace minted for the request where it did not.
#[tokio::test]
async fn every_query_log_refusal_closes_with_the_trace_clause_of_the_request() {
    let router = emulator();
    router.hold_vpcs([HELD_VPC, UNASSOCIATED_VPC]);
    // A configuration the store DOES hold, so a refusal is about the resource
    // the request named and not about an empty store.
    let stored = config_id(&router, "traced-refusals").await;

    // Every trace the service minted for a request that named none, so two
    // requests are never reported under one minted chain.
    let mut minted: Vec<String> = Vec::new();

    for refusal in REFUSALS {
        for tracing in TRACINGS {
            let case = format!("{} / {}", refusal.case, tracing.case);
            let (status, body) = call_tracing(
                &router,
                refusal.operation,
                (refusal.body)(&stored),
                tracing.header,
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: {} must answer HTTP 400, got {status}: {body}",
                refusal.operation
            );
            assert!(
                error_type(&body).contains("ResourceNotFoundException"),
                "case {case:?}: the refusal must name ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                string_member(&body, "ResourceType"),
                refusal.resource_type,
                "case {case:?}: the refusal must name the resource that is missing, got {body}"
            );

            let message = string_member(&body, "Message");
            let (stated, trace) = traced_message(&message).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the refusal must close with its trace clause, carrying a \
                     trace id in quotes, got {message:?}"
                )
            });
            assert_eq!(
                stated, refusal.sentence,
                "case {case:?}: the refusal must state its coded sentence before the clause, got \
                 {message:?}"
            );
            match tracing.expected_root {
                // The caller named a chain, so the refusal is reported under
                // that chain and not under one of the service's own.
                Some(root) => assert_eq!(
                    trace, root,
                    "case {case:?}: the clause must carry the Root the request named, got \
                     {message:?}"
                ),
                // The caller named none, so the service minted one for THIS
                // request: nothing the request carried is echoed into it.
                None => {
                    assert!(
                        !message.contains("not-a-trace-id"),
                        "case {case:?}: a Root that is not a trace id must be replaced, not \
                         echoed, got {message:?}"
                    );
                    minted.push(trace.to_string());
                }
            }
        }
    }

    let distinct = minted.iter().collect::<BTreeSet<&String>>().len();
    assert_eq!(
        distinct,
        minted.len(),
        "a trace the service mints is this request's own, so no two minted clauses may repeat, \
         got {minted:?}"
    );
}

/// The clause reports the trace of the REQUEST, so the same refusal of the same
/// resource is reported under a different chain for every caller that asks —
/// and under the same chain for two calls of one chain, which is what lets a
/// caller tie a refusal back to the request that drew it.
#[tokio::test]
async fn the_clause_follows_the_request_and_not_the_resource_it_asked_about() {
    let router = emulator();
    let first = "Root=1-6b000002-111111111111111111111111;Sampled=0";
    let second = "Root=1-6b000003-222222222222222222222222;Sampled=1";

    let mut seen: Vec<(String, String)> = Vec::new();
    for (case, header) in [
        ("the first chain", first),
        ("the second chain", second),
        ("the first chain again", first),
    ] {
        let (_, body) = call_tracing(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": UNKNOWN_CONFIG }),
            Some(header),
        )
        .await;
        let message = string_member(&body, "Message");
        let (_, trace) = traced_message(&message).unwrap_or_else(|| {
            panic!("case {case:?}: the refusal must close with its trace clause, got {message:?}")
        });
        seen.push((case.to_string(), trace.to_string()));
    }

    assert_eq!(
        seen[0].1, seen[2].1,
        "two calls of one chain must be reported under that chain, got {seen:?}"
    );
    assert_ne!(
        seen[0].1, seen[1].1,
        "two calls of different chains must be reported under their own, got {seen:?}"
    );
}

/// A SERVED query-logging call carries no refusal and so no clause: the trace
/// clause belongs to the refusal and is not appended to anything the service
/// answers, so the tracing header changes nothing about a call that succeeds.
#[tokio::test]
async fn a_served_query_log_call_carries_no_trace_clause() {
    let router = emulator();
    router.hold_vpcs([HELD_VPC]);
    let stored = config_id(&router, "served-with-tracing").await;

    let made = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": stored.clone(), "ResourceId": HELD_VPC }),
    )
    .await;
    let association_id = string_member(association(&made), "Id");

    for (case, header) in [
        ("a caller naming a chain", Some("Root=1-6b000004-333333333333333333333333;Sampled=0")),
        ("a caller naming none", None),
    ] {
        for (operation, body) in [
            (
                "GetResolverQueryLogConfig",
                json!({ "ResolverQueryLogConfigId": stored.clone() }),
            ),
            (
                "GetResolverQueryLogConfigAssociation",
                json!({ "ResolverQueryLogConfigAssociationId": association_id.clone() }),
            ),
        ] {
            let (status, served) = call_tracing(&router, operation, body, header).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {operation} must be served, got {status}: {served}"
            );
            assert!(
                served.get("Message").is_none(),
                "case {case:?}: {operation} must answer no Message, got {served}"
            );
            assert!(
                !serde_json::to_string(&served)
                    .expect("the answer serializes")
                    .contains("Trace Id"),
                "case {case:?}: {operation} must carry no trace clause, got {served}"
            );
        }
    }

    // The configuration and its association are untouched by every refusal
    // above, which is what makes those refusals about the id and not about the
    // store.
    let read = ok(
        &router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": stored.clone() }),
    )
    .await;
    let config = read
        .get("ResolverQueryLogConfig")
        .unwrap_or_else(|| panic!("the read answers a configuration, got {read}"));
    assert_eq!(
        string_member(config, "Id"),
        stored,
        "the stored configuration must read back, got {read}"
    );
}
