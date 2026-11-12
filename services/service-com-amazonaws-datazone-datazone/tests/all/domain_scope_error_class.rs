// SPDX-License-Identifier: CC0-1.0

//! Which STAGE answers a domain-scoped call whose `domainIdentifier` resolves to no
//! stored domain: the input-constraint stage, or the authorization stage.
//!
//! RULE: the domain scope of a domain-scoped request is an input-constrained member
//! (`DomainId`, `@pattern ^dzd[-_][a-zA-Z0-9_-]{1,36}$` — smithy_query shape="DomainId"),
//! so a value that VIOLATES that pattern never reaches a handler and is the
//! input-constraint failure the model binds (`ValidationException`, HTTP 400, "The input
//! fails to satisfy the constraints specified by the Amazon Web Services service"), while
//! a value that SATISFIES it has passed every modelled constraint — whether it resolves to
//! a domain is not an input constraint but a permission the caller holds ON a domain, so an
//! identifier this store serves no domain for is answered by the authorization stage
//! (`AccessDeniedException`, HTTP 403, which every operation below binds, e.g.
//! `smithy_query(query="operation", operation="ListProjects")`). The two stages are never
//! swapped: resolvability is decided after validation, so a 400 for a pattern-valid
//! identifier would tell a caller to "fix" a well-formed value it merely lacks permission
//! on, and a 403 for a malformed one would hide a request the service cannot even read.
//!
//! `domain_scoped_read_authorization` and `domain_scoped_write_authorization` assert the
//! denial over every domain-scoped operation this emulator serves; this suite asserts the
//! BOUNDARY between the two stages, which is what decides the error class, over both
//! halves at once:
//!
//! * pattern-valid identifiers naming no stored domain, differing in separator, in tail
//!   length at each bound the pattern publishes and in tail content — every one a denial,
//!   never an input-constraint failure;
//! * identifiers that violate the pattern in each way it can be violated (the prefix, the
//!   separator, an empty tail, a tail one character past the published maximum, a
//!   character the tail's class excludes) — every one an input-constraint failure naming
//!   the member the model binds, never a denial;
//! * the same operations against a domain this store DOES serve, which answer from state —
//!   so neither stage is refusing on something other than the scope.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::{Answer, Emulator, SETTLED};
use serde_json::{json, Value};

/// The one `DataZoneEntityType` member (smithy_query shape="DataZoneEntityType").
const ENTITY_TYPE: &str = "DOMAIN_UNIT";

/// A `ManagedPolicyType` member (smithy_query shape="ManagedPolicyType").
const POLICY_TYPE: &str = "CREATE_GLOSSARY";

/// The member name the model binds the domain scope to on an operation addressing a
/// resource INSIDE the domain (smithy_query operation="ListProjects").
const SCOPE_MEMBER: &str = "domainIdentifier";

/// The member name the model binds on an operation whose own identifier IS the domain
/// (smithy_query operation="GetDomain").
const DOMAIN_SCOPE_MEMBER: &str = "identifier";

/// One domain-scoped call: the operation the model binds, its method, the path it
/// addresses under `/v2/domains/{<scope member>}` (empty for the operation whose own
/// identifier IS the domain), the body it takes, and the name of the member the domain
/// scope arrives in.
struct ScopedCall {
    operation: &'static str,
    method: &'static str,
    suffix: String,
    body: Option<Value>,
    scope_member: &'static str,
}

impl ScopedCall {
    fn new(
        operation: &'static str,
        method: &'static str,
        suffix: String,
        body: Option<Value>,
        scope_member: &'static str,
    ) -> Self {
        Self {
            operation,
            method,
            suffix,
            body,
            scope_member,
        }
    }

    /// This call, scoped to `domain`.
    fn uri(&self, domain: &str) -> String {
        if self.suffix.is_empty() {
            format!("/v2/domains/{domain}")
        } else {
            format!("/v2/domains/{domain}/{}", self.suffix)
        }
    }

    /// The `message` member this call's denial must carry: the operation-scoped sentence
    /// for a call INSIDE a domain, whose only variable part is the operation name of the
    /// request, and NO member at all for the one whose own identifier is the domain scope
    /// (that denial's body carries no members, and a member the service states nothing in
    /// is absent from a `restJson1` body rather than present and empty).
    fn denial_message(&self) -> Option<String> {
        if self.suffix.is_empty() {
            None
        } else {
            Some(format!(
                "User is not permitted to perform operation: {}",
                self.operation
            ))
        }
    }
}

/// The `message` member an answer carries, absent when the body carries none.
fn message_member(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Every call the rule is asserted over, addressing resource identifiers that satisfy
/// their own members' modelled patterns so that the domain scope is the only thing in
/// question. The rows differ along what the rule generalises over: the HTTP method, the
/// kind of call (listing, create, single-resource read), whether the call carries a body,
/// and whether the domain is the scope of a resource inside it or the resource itself.
fn scoped_calls() -> Vec<ScopedCall> {
    // A `DomainUnitId` is 1..256 of `^[a-z0-9_\-]+$` (smithy_query shape="DomainUnitId"),
    // a `ProjectId` is `^[a-zA-Z0-9_-]{1,36}$` (smithy_query shape="ProjectId") — both
    // well-formed here, and neither resolvable, which is what the rule is about.
    let unit = "rootunit";
    let project = "someproject";
    vec![
        ScopedCall::new(
            "ListProjects",
            "GET",
            "projects".to_string(),
            None,
            SCOPE_MEMBER,
        ),
        ScopedCall::new(
            "ListDomainUnitsForParent",
            "GET",
            format!("domain-units?parentDomainUnitIdentifier={unit}"),
            None,
            SCOPE_MEMBER,
        ),
        ScopedCall::new(
            "ListEntityOwners",
            "GET",
            format!("entities/{ENTITY_TYPE}/{unit}/owners"),
            None,
            SCOPE_MEMBER,
        ),
        ScopedCall::new(
            "ListPolicyGrants",
            "GET",
            format!("policies/managed/{ENTITY_TYPE}/{unit}/grants?policyType={POLICY_TYPE}"),
            None,
            SCOPE_MEMBER,
        ),
        ScopedCall::new(
            "CreateDomainUnit",
            "POST",
            "domain-units".to_string(),
            Some(json!({ "name": "boundary unit", "parentDomainUnitIdentifier": unit })),
            SCOPE_MEMBER,
        ),
        ScopedCall::new(
            "CreateAssetType",
            "POST",
            "asset-types".to_string(),
            Some(json!({
                "name": "BoundaryType",
                "owningProjectIdentifier": project,
                "formsInput": {},
            })),
            SCOPE_MEMBER,
        ),
        // A single-resource read, to show the boundary is not keyed to listings and
        // creates.
        ScopedCall::new(
            "GetDomainUnit",
            "GET",
            format!("domain-units/{unit}"),
            None,
            SCOPE_MEMBER,
        ),
        // The operation whose own identifier IS the domain scope: a different member name,
        // and a denial that states nothing.
        ScopedCall::new("GetDomain", "GET", String::new(), None, DOMAIN_SCOPE_MEMBER),
    ]
}

/// Identifiers no domain in the store carries, each SATISFYING `DomainId`'s pattern
/// `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId"). The rows differ along
/// what the pattern leaves open: which separator it admits, the tail length at each bound
/// it publishes, and a tail mixing the other characters of its class.
fn pattern_valid_unserved_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "an underscore separator",
            "dzd_noboundarydomain".to_string(),
        ),
        ("a hyphen separator", "dzd-noboundarydomain".to_string()),
        ("a tail of 1, the pattern's minimum", "dzd_b".to_string()),
        (
            "a tail of 36, the pattern's maximum",
            format!("dzd_{}", "b".repeat(36)),
        ),
        (
            "a tail mixing digits, hyphens and underscores",
            "dzd_7-absent_2".to_string(),
        ),
    ]
}

/// Identifiers that VIOLATE `DomainId`'s pattern, one row per way the pattern can be
/// violated: the literal prefix, the separator class `[-_]`, the tail's `{1,36}` bound at
/// each end, and the tail's character class.
fn pattern_violating_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "a prefix the pattern does not admit",
            "dzx_absent0".to_string(),
        ),
        ("no separator after the prefix", "dzd0absent".to_string()),
        (
            "an empty tail, one below the minimum of 1",
            "dzd_".to_string(),
        ),
        (
            "a tail of 37, one past the maximum of 36",
            format!("dzd_{}", "c".repeat(37)),
        ),
        (
            "a character the tail's class excludes",
            "dzd_absent.0".to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_pattern_valid_scope_the_store_serves_no_domain_for_is_answered_by_authorization() {
    let dz = Emulator::new();

    for (id_label, unserved) in pattern_valid_unserved_ids() {
        for call in scoped_calls() {
            let case = format!("{} scoped to {id_label}", call.operation);
            let answer = dz
                .call(call.method, &call.uri(&unserved), call.body.clone())
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: a scope that satisfies {:?}'s pattern has passed every \
                 modelled constraint, so the answer is the denial, not an input-constraint \
                 failure; got {} {:?}",
                call.scope_member,
                answer.status,
                answer.body
            );
            let message = message_member(&answer);
            assert_eq!(
                message,
                call.denial_message(),
                "case {case:?}: the denial must carry the operation-scoped sentence, got \
                 {message:?}"
            );
            assert!(
                message.as_ref().is_none_or(|text| !text.contains(&unserved)),
                "case {case:?}: the denial must not echo the requested scope {unserved:?}, \
                 got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_scope_that_violates_the_pattern_is_answered_by_the_input_constraint_stage() {
    let dz = Emulator::new();

    for (id_label, malformed) in pattern_violating_ids() {
        for call in scoped_calls() {
            let case = format!("{} scoped to {id_label}", call.operation);
            let answer = dz
                .call(call.method, &call.uri(&malformed), call.body.clone())
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: a scope that violates the modelled pattern is refused as \
                 input, got {} {:?}",
                answer.status,
                answer.body
            );
            let message = message_member(&answer).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the refusal must say what failed, got {:?}",
                    answer.body
                )
            });
            assert!(
                message.contains(call.scope_member),
                "case {case:?}: the refusal must name the member that failed ({:?}), got \
                 {message:?}",
                call.scope_member
            );
            assert!(
                !message.contains("not permitted"),
                "case {case:?}: input the service cannot read is not a denial, got \
                 {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_same_calls_answer_from_state_under_a_domain_the_store_serves() {
    let dz = Emulator::new();

    // The control for both rules above: the two stages refuse the SCOPE and nothing else,
    // so the same calls, scoped to a domain this store serves, answer from its state.
    let created = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "boundary domain" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "CreateDomain: expected 201, got {} {:?}",
        created.status, created.body
    );
    let domain = created.s("id");
    let root_unit = created.s("rootDomainUnitId");
    tokio::time::sleep(SETTLED).await;

    let project = dz.project(&domain, "boundary owner").await;

    // A create, read back through the listing of the same resource: the scope that was
    // denied above now carries real state.
    let unit = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({ "name": "boundary unit", "parentDomainUnitIdentifier": root_unit })),
        )
        .await;
    assert_eq!(
        unit.status, 201,
        "CreateDomainUnit under a served domain: expected 201, got {} {:?}",
        unit.status, unit.body
    );
    let unit_id = unit.s("id");

    let children = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={root_unit}"),
            None,
        )
        .await;
    assert_eq!(
        children.status, 200,
        "ListDomainUnitsForParent under a served domain: expected 200, got {} {:?}",
        children.status, children.body
    );
    assert!(
        listed_ids(&children).contains(&unit_id),
        "the listing must report the unit just created ({unit_id:?}), got {:?}",
        children.body
    );

    let projects = dz
        .call("GET", &format!("/v2/domains/{domain}/projects"), None)
        .await;
    assert_eq!(
        projects.status, 200,
        "ListProjects under a served domain: expected 200, got {} {:?}",
        projects.status, projects.body
    );
    assert!(
        listed_ids(&projects).contains(&project),
        "the listing must report the project just created ({project:?}), got {:?}",
        projects.body
    );

    let asset_type = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/asset-types"),
            Some(json!({
                "name": "BoundaryType",
                "owningProjectIdentifier": project,
                "formsInput": {},
            })),
        )
        .await;
    assert_eq!(
        asset_type.status, 201,
        "CreateAssetType under a served domain: expected 201, got {} {:?}",
        asset_type.status, asset_type.body
    );
}

/// The `id` of every item a listing reported.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
