// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appsync-awsdeepdishcontrolplaneservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// The Event API's DATA plane: the two sockets each `Api` is served on — the HTTP endpoint
/// a publisher POSTs to and the real-time endpoint a subscriber opens a WebSocket against
/// — which are what the hostnames in `Api$dns` publish. They are SOCKETS rather than
/// routes of this service, because the seams this file carries take one buffered request
/// and return one buffered response: see that module for why, and for what each endpoint
/// answers.
pub mod event_api;

/// AppSync's own modelled error for a request that is not well formed:
/// `BadRequestException`, `httpError` 400, with members `message`/`reason`/`detail`
/// (`smithy_query(query="shape", shape="BadRequestException")`). Every operation of
/// this service models it, and no operation models `ValidationException`.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The error code the framework names a boundary refusal with on this service's
/// restJson1 envelope (`json` in `vendor/smithy-server-runtime/src/pre_dispatch.rs`),
/// repeated here because [`Refused`] replaces the code and the message TOGETHER: the
/// refusals below that only adjust the WORDING must keep answering under the code the
/// caller would otherwise have seen.
const FRAMEWORK_REFUSAL_CODE: &str = "ValidationException";

/// Every member this service's model constrains with `@range`, with the bounds the
/// shape it targets declares, keyed by the member name a violation path ends in.
///
/// Source: `smithy_query(query="shape", shape="MaxResults,MaxBatchSize,
/// QueryDepthLimit,ResolverCountLimit")` — `MaxResults` 0..25 (every list operation's
/// `maxResults`), `MaxBatchSize` 0..2000 (`CreateResolver`/`UpdateResolver`/
/// `CreateFunction`/`UpdateFunction`), `QueryDepthLimit` 0..75 and
/// `ResolverCountLimit` 0..10000 (`CreateGraphqlApi`/`UpdateGraphqlApi`). Those four
/// shapes are the whole of it: every other numeric member of this model (the `ttl`
/// and `expires` members among them) targets an unconstrained `Long`, so no other
/// member can produce a range violation.
///
/// FOLLOW-UP: these bounds are copied out of the model rather than read from it,
/// because a refusal reaches this file as a [`Violation`] whose
/// `ViolationKind::MinValue`/`MaxValue` carries only the bound that was CROSSED
/// (`vendor/smithy-schema-server/src/constraints.rs`), and the message states both.
/// Carrying the whole declared range on those variants would let the message be
/// computed from the model at refusal time and make this table unnecessary; that is a
/// change to the vendored framework, which this fix cannot make. Until then a member
/// absent from the table is named alone rather than with invented bounds, so drift
/// drops the range clause instead of stating a wrong one.
const RANGED_MEMBERS: &[(&str, i64, i64)] = &[
    ("maxBatchSize", 0, 2_000),
    ("maxResults", 0, 25),
    ("queryDepthLimit", 0, 75),
    ("resolverCountLimit", 0, 10_000),
];

/// The members AppSync's own refusal labels differently from the request member that
/// carries them.
///
/// This is service vocabulary, not model data: nothing in the model states it, and it
/// comes from the service's observed wording — `resolverCountLimit` is reported as
/// `resolverLimit` ("Invalid resolverLimit given. Must fall within range 0 to
/// 10,000") and `operationLevelMetricsConfig` as `operationLevelMetricConfig`
/// ("Invalid operationLevelMetricConfig given."), a label short of the member's
/// plural. A member absent from this table is named exactly as the model names it,
/// which is what every other refusal above shows.
const REFUSAL_LABELS: &[(&str, &str)] = &[
    ("operationLevelMetricsConfig", "operationLevelMetricConfig"),
    ("resolverCountLimit", "resolverLimit"),
];

/// The member a violation path names: its last segment, so a nested member
/// (`enhancedMetricsConfig.dataSourceLevelMetricsBehavior`) is named the way AppSync
/// names it in a refusal — by the member, not by the path it sits on.
fn violated_member(path: &str) -> &str {
    path.split('.').next_back().unwrap_or(path)
}

/// The label AppSync's refusal names `member` by ([`REFUSAL_LABELS`]).
fn refusal_label(member: &str) -> &str {
    REFUSAL_LABELS
        .iter()
        .find(|(name, _)| *name == member)
        .map_or(member, |(_, label)| *label)
}

/// `value` with its digits grouped in threes by commas, the way AppSync renders a
/// bound in a refusal ("Must fall within range 0 to 10,000").
fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if value < 0 {
        out.push('-');
    }
    for (position, digit) in digits.char_indices() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// AppSync's own refusal for a member whose value the model rules out outright — an
/// absent required member, or a value outside the member's modelled `@range`: the
/// service's `BadRequestException`, naming the offending member, and the whole
/// modelled range rather than the single bound that was crossed.
///
/// The framework's generic report would name `ValidationException`, a shape no
/// operation of this service models, so a client matching on `BadRequestException`
/// (and reading its `reason`/`detail`) would never fire. The message states ONE
/// member — the first the walk refused — where the framework's aggregates every
/// clause, because that is how the service states it.
///
/// `None` when no violation is of either kind; a length, a pattern or an enum value
/// set then keeps the framework's wording (see [`without_rejected_enum_values`]).
fn member_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        let member = violated_member(&violation.path);
        let label = refusal_label(member);
        let range = RANGED_MEMBERS.iter().find(|(name, _, _)| *name == member);
        let message = match violation.kind {
            ViolationKind::MissingRequiredMember => format!("Invalid {label} given."),
            ViolationKind::MinValue(_) | ViolationKind::MaxValue(_) => match range {
                Some((_, min, max)) => format!(
                    "Invalid {label} given. Must fall within range {} to {}",
                    grouped(*min),
                    grouped(*max)
                ),
                // A range this model does not declare cannot be stated; name the
                // member alone rather than inventing bounds for it.
                None => format!("Invalid {label} given."),
            },
            _ => return None,
        };
        Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message,
        })
    })
}

/// The member every operation of this service names a reporting format in: the
/// `OutputType` `GetIntrospectionSchema` renders a schema under and the
/// `TypeDefinitionFormat` every type operation reports a definition in
/// (`smithy_query(query="shape", shape="OutputType,TypeDefinitionFormat")` — both
/// enumerate `SDL` and `JSON`).
const FORMAT_MEMBER: &str = "format";

/// AppSync's own refusal for a `format` whose value is outside the enumerated set the
/// member's type declares: its `BadRequestException`, naming the member and the rejected
/// value — the one value the caller has to change, and the one the service does echo.
///
/// The framework's generic report would name `ValidationException`, a shape no operation
/// of this service models, so a client matching on `BadRequestException` would never
/// fire. Every OTHER member whose enumerated value set a request falls outside keeps the
/// framework's wording with the rejected value lifted out
/// ([`without_rejected_enum_values`]).
fn format_refusal(violations: &[Violation]) -> Option<Refused> {
    violations.iter().find_map(|violation| {
        if violated_member(&violation.path) != FORMAT_MEMBER
            || !matches!(violation.kind, ViolationKind::EnumValue(_))
        {
            return None;
        }
        let value = violation.value.as_deref()?;
        Some(Refused {
            code: BAD_REQUEST_CODE.to_string(),
            message: format!("Invalid {FORMAT_MEMBER} {value} given"),
        })
    })
}

/// The framework's aggregated constraint report with the rejected value lifted out of
/// every ENUM clause, or `None` when no clause echoes one.
///
/// A value outside an enumerated type's value set is reported by naming the member
/// path and the permitted values; the rejected value itself is withheld, because it
/// is precisely the value the set does not contain. Every other clause — a length, a
/// range, a pattern, an absent member — keeps the framework's wording, including its
/// echo, and the code stays the one the framework answers with
/// ([`FRAMEWORK_REFUSAL_CODE`]).
///
/// The text replaced is built from the SAME two fields the framework interpolated
/// (`Violation::message`), so it matches the echo exactly whatever the value holds.
fn without_rejected_enum_values(violations: &[Violation]) -> Option<Refused> {
    let echoes_enum_value = violations
        .iter()
        .any(|v| matches!(v.kind, ViolationKind::EnumValue(_)) && v.value.is_some());
    if !echoes_enum_value {
        return None;
    }
    let mut message = aws_smithy_schema_server::envelope(violations)?;
    for violation in violations {
        if !matches!(violation.kind, ViolationKind::EnumValue(_)) {
            continue;
        }
        let Some(value) = violation.value.as_deref() else {
            continue;
        };
        message = message.replace(
            &format!("Value '{value}' at '{}'", violation.path),
            &format!("Value at '{}'", violation.path),
        );
    }
    Some(Refused {
        code: FRAMEWORK_REFUSAL_CODE.to_string(),
        message,
    })
}

/// The member every tagging operation addresses its resource by.
///
/// Source: `smithy_query(query="operation", operation="TagResource,UntagResource,
/// ListTagsForResource")` — each takes `resourceArn`, bound to the one `/v1/tags/…`
/// path label and targeting `ResourceArn`, whose `@length` and `@pattern` admit one ARN
/// shape.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// Whether this violation is the TAGGING surface's own `resourceArn`: the TOP-LEVEL
/// member of that name, the one bound to the `/v1/tags/{resourceArn}` label and targeting
/// `ResourceArn`, whose pattern admits only an ARN of this service's own vendor.
///
/// The path is compared whole, because another member of this model is also named
/// `resourceArn` and is an ARN of ANOTHER vendor: `RdsDataApiConfig$resourceArn` — "the
/// resource ARN of the RDS cluster", pattern `^arn:[a-z-]*:rds:…:cluster:…$`
/// (`smithy_query(query="shape", shape="RdsDataApiConfig")`) — reaches this file on the
/// nested path `rdsDataApiConfig.resourceArn`, and the vendor the tagging surface expects
/// is not the vendor that member names.
fn tagging_resource_arn(violation: &Violation) -> bool {
    violation.path == RESOURCE_ARN_MEMBER
}

/// AppSync's own refusal for a `resourceArn` whose VALUE cannot be a resource ARN at all
/// — outside the shape's length bounds or its pattern.
///
/// The tagging operations model `BadRequestException` and no constraint-violation error,
/// so the framework's generic report (`ValidationException`, with every broken clause
/// recited) names a shape no operation of this service models and a client's error-type
/// dispatch cannot match it. The service states the malformed ARN as one sentence
/// instead, which is why the rejected value is not echoed — and when the value IS an ARN,
/// of a vendor this service does not serve, that sentence names the vendor it expected
/// (`crate::state::invalid_resource_arn_message`), the one diagnosis that says which
/// field of the ARN failed.
///
/// `None` when no violation is of that member and kind, which leaves every other
/// refusal to the wordings the rest of this file states.
fn resource_arn_refusal(violations: &[Violation]) -> Option<Refused> {
    violations
        .iter()
        .find(|violation| {
            violated_member(&violation.path) == RESOURCE_ARN_MEMBER
                && matches!(
                    violation.kind,
                    ViolationKind::MinLength(_)
                        | ViolationKind::MaxLength(_)
                        | ViolationKind::Pattern(_)
                )
        })
        .map(|violation| Refused {
            code: BAD_REQUEST_CODE.to_string(),
            // Only the tagging surface's own member is read as an ARN of THIS vendor;
            // for any other member of that name the sentence stays the bare one.
            message: crate::state::invalid_resource_arn_message(
                violation
                    .value
                    .as_deref()
                    .filter(|_| tagging_resource_arn(violation)),
            ),
        })
}

/// The `com-amazonaws-appsync-awsdeepdishcontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. AppSync states three of those classes itself:
        //
        // * a `resourceArn` whose value cannot be a resource ARN at all is its own
        //   `BadRequestException` with the service's sentence for a malformed ARN —
        //   naming the vendor it expected when the value is an ARN of another one
        //   (`resource_arn_refusal`) — the only client error its operations model;
        // * a member the model rules out outright — absent though required, or
        //   outside its modelled `@range` — is its own `BadRequestException` naming
        //   that member, and its range when it has one (`member_refusal`); the
        //   framework's `ValidationException` is a shape no operation of this service
        //   models;
        // * a value outside an enumerated type's value set names the member path and
        //   the permitted values WITHOUT echoing the rejected value
        //   (`without_rejected_enum_values`), under the framework's own code — except
        //   for the `format` a report is asked in, which the service refuses by naming
        //   the member and the rejected value (`format_refusal`).
        //
        // Every other refusal — a length, a pattern — keeps the framework's wording
        // and code.
        match refusal {
            Refusal::Invalid(violations) => resource_arn_refusal(violations)
                .or_else(|| member_refusal(violations))
                .or_else(|| format_refusal(violations))
                .or_else(|| without_rejected_enum_values(violations)),
            // Authentication refusals, and any refusal kind added later, keep the
            // framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
