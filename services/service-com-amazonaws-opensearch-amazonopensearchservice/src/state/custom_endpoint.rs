// SPDX-License-Identifier: CC0-1.0

//! Whether the custom endpoint a domain's endpoint options name is a fully qualified
//! domain name — the one input constraint of those options no walk ahead of a handler
//! applies.
//!
//! `DomainEndpointOptions$CustomEndpoint` targets `DomainNameFqdn`, whose `@pattern`
//! opens every label with the negative lookahead `(?!-)`
//! (`smithy_query(query="shape",shape="DomainNameFqdn")`). The engine the request
//! boundary compiles a `@pattern` with has no look-around, and that boundary treats a
//! pattern it cannot compile as matching (`pattern_matches` in
//! `vendor/smithy-schema-server/src/constraints.rs`), so the syntax the model declares for
//! this member would otherwise never be applied to a request at all. It is applied here
//! instead, by a look-around-capable engine over the MODEL's own expression, so the clause
//! — which quotes that expression — cannot state a requirement this module does not apply.
//! The member's `@length` is left to the boundary, which enforces it already.
//!
//! Being an INPUT constraint it is evaluated BEFORE the request's resources are resolved:
//! a value outside a member's declared syntax is refused by naming that member, never
//! reported as a domain that does not exist — which is a different claim, about a
//! different thing, and sends a caller to fix the wrong half of its request.
//!
//! The WORDS are the service's one home for them (`crate::service::validation`), so a
//! clause this handler-side check states reads like one the boundary states.

use std::sync::LazyLock;

use ::aws_smithy_schema_server::constraints::path;

use crate::ops::types::{DomainEndpointOptions, ValidationException};

/// The structure the custom endpoint travels in, as the model names it.
const OPTIONS_MEMBER: &str = "DomainEndpointOptions";

/// The member inside it that carries the endpoint, as the model names it.
const ENDPOINT_MEMBER: &str = "CustomEndpoint";

/// The syntax a custom endpoint has to satisfy, as the model declares it for
/// `DomainNameFqdn` (`smithy_query(query="shape",shape="DomainNameFqdn")`): a sequence of
/// dot-separated labels over letters, digits and hyphens, none of them opening with a
/// hyphen or closing with one, and at least one dot — a fully qualified name rather than a
/// single label.
///
/// The one text used BOTH ways: it is the expression the check runs and the expression the
/// clause quotes, so a caller that lifts the expression out of the refusal and
/// pre-validates with it gets the one this service applies.
const CUSTOM_ENDPOINT_PATTERN: &str =
    r"^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$";

/// The step budget one custom-endpoint match may spend.
///
/// The engine below BACKTRACKS, so a match has no linear-time guarantee in general and the
/// work one request may cost is bounded here rather than left to the shape of the
/// expression. Two facts put this budget out of reach in practice, and both are properties
/// of the input rather than of the engine: the expression is a MODEL CONSTANT, never built
/// from a request, and the value matched is `DomainNameFqdn`, whose `@length` maximum of
/// 255 characters the boundary validator enforces before this handler-side check runs. The
/// budget is the backstop for a model expression that is not so tame: a match that
/// exhausts it is answered, not retried, and never spins.
const BACKTRACK_LIMIT: usize = 1_000_000;

/// [`CUSTOM_ENDPOINT_PATTERN`] compiled once, under that budget.
///
/// Compilation is fallible and its input is a compile-time constant, never request text,
/// but an expression that failed to compile is still dropped rather than unwrapped:
/// nothing here panics, on any input.
static CUSTOM_ENDPOINT: LazyLock<Option<::fancy_regex::Regex>> = LazyLock::new(|| {
    ::fancy_regex::RegexBuilder::new(CUSTOM_ENDPOINT_PATTERN)
        .backtrack_limit(BACKTRACK_LIMIT)
        .build()
        .ok()
});

/// `Some(refusal)` when `options` carries a custom endpoint outside the syntax
/// [`CUSTOM_ENDPOINT_PATTERN`] states, and `None` for one that satisfies it — and for a
/// request that names no custom endpoint at all, which has nothing to refuse.
///
/// The member path is the whole dotted path a clause spells for a member inside a
/// structure, so a caller is told which member of which container to fix.
pub fn custom_endpoint_refusal(
    options: Option<&DomainEndpointOptions>,
) -> Option<ValidationException> {
    let endpoint = options?.custom_endpoint.as_deref()?;
    if satisfies_fqdn(endpoint) {
        return None;
    }
    Some(ValidationException {
        message: Some(crate::service::validation::published_pattern_report(
            &path::join(&[
                path::member(OPTIONS_MEMBER),
                path::member(ENDPOINT_MEMBER),
            ]),
            endpoint,
            CUSTOM_ENDPOINT_PATTERN,
        )),
    })
}

/// Whether `endpoint` satisfies the FQDN syntax of its member.
///
/// An expression this crate holds no compiled form for, and a match that exhausts its
/// backtrack budget, are both treated as satisfied — which is what the boundary validator
/// does with a pattern it cannot compile: a value is never refused because of an
/// expression nothing could evaluate.
fn satisfies_fqdn(endpoint: &str) -> bool {
    CUSTOM_ENDPOINT
        .as_ref()
        .is_none_or(|compiled| compiled.is_match(endpoint).unwrap_or(true))
}
