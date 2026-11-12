// SPDX-License-Identifier: CC0-1.0

//! @op ListRecommendedActions
//!
//! Reports the recommended actions held in [`crate::state`], narrowed by the
//! request's `filter` and cut into pages by `maxResults`/`nextToken`. This is the
//! ONLY file to touch for this operation.
//!
//! ## What this operation does
//!
//! * **Reporting only.** The model binds this single operation and its doc says it
//!   "returns a list of recommended actions"; it never writes, so two identical
//!   calls answer identically and a later caller sees exactly what an earlier one
//!   saw. The records come from the store, which already holds the inventory this
//!   account has when the emulator starts — installed while the emulator boots
//!   ([`crate::state::install_service_state`]), so no request of this operation,
//!   not even the first, decides what any record reports — and which a caller can
//!   re-fill through [`crate::state::RecommendedActionsStore::seed`] — the service
//!   has no create operation because AWS itself computes the recommendations.
//! * **Every account the caller can see.** `RecommendedAction$accountId` is "the
//!   account that the recommended action is for", and `FilterName` has no account
//!   key — the real API reports the actions of the calling (management) account AND
//!   of the member accounts it pays for, which is why each record names its own
//!   account. So the store holds exactly the set this caller may see and the handler
//!   reports all of it; it does not re-filter by the caller's own account, which
//!   would hide every member-account record the shape exists to carry. This emulator
//!   presents one account id ([`engine::emulator_account_id`]), so a record is
//!   normally seeded with that id.
//! * **Authorization is deliberately unmodelled.** The emulator verifies every
//!   request signature at its boundary and holds no per-principal policy, so this
//!   handler never evaluates permissions and the modelled `AccessDeniedException`
//!   (403) stays constructible but unreached; read the silence as "not modelled",
//!   not as an enforced allow.
//! * **Filtering.** `filter.actions` is a list of `ActionFilter`s, each naming a
//!   record field (`FEATURE` / `SEVERITY` / `TYPE`), a `matchOption` (`EQUALS`
//!   keeps the records whose field is one of `values`, `NOT_EQUALS` excludes them)
//!   and the `values` to compare against. Several filters are ANDed. `values` is a
//!   list of strings COMPARED against a record's field, not an enumerated member of
//!   the request shape: a value no record carries simply matches no record, so an
//!   `EQUALS` filter over it keeps nothing and a `NOT_EQUALS` filter over it keeps
//!   everything. Only the members the model types as enums (`key`, `matchOption`)
//!   and the modelled constraints on `values` itself (an empty list fails its
//!   minimum length of 1) are refused with `ValidationException`.
//! * **How a refusal is stated.** Every refusal of this operation's input is one
//!   `ValidationException` (400) carrying the modelled `reason` and one `fieldList`
//!   entry per offending member, built from [`Violation`]s by
//!   [`field_validation_failed`] — the same rendering whether the offending member
//!   was found at the constraint boundary (see `crate::service`) or here. A request
//!   the service could not READ carries the other reason: it is stated by
//!   [`cannot_parse_input`] as `cannotParse`, naming each member whose value could
//!   not be read. Which of the two reasons a refusal states never depends on WHERE
//!   the offending value was found, only on whether the service read it. The
//!   wording of a member-level refusal is split between its two statements: the
//!   summary envelope names the member and the requirement it failed, the member's
//!   own `fieldList` entry adds the value the request sent, and a requirement over
//!   an enumerated value set lists that enum's members in the order this service
//!   states that set in (`crate::state::FilterName::VALUE_SET_ORDER`), which is not
//!   the model's declaration order.
//! * **Pagination.** Records are reported in store (insertion) order, so a page
//!   boundary is stable. `nextToken` is opaque: a 128-bit fingerprint of the
//!   SELECTION the token was issued over (the filter) followed by the offset to
//!   resume at. The fingerprint names the selection and nothing else: the filters
//!   are reduced to a set (so listing them in another order, or listing one of them
//!   twice, resumes the same listing) and the fingerprinted encoding is
//!   length-prefixed (so a `values` element carrying any character cannot make one
//!   selection look like another). `maxResults` is a per-call page-size hint and is
//!   deliberately NOT
//!   part of the token's identity, so a caller may change it, or drop it, between
//!   pages and still resume where the previous page stopped; a request that names
//!   no `maxResults` is served at [`DEFAULT_PAGE_SIZE`], the largest page this
//!   service puts in one response. A token is emitted only while unreported
//!   records remain, and a token that does not parse, or that was issued over a
//!   different filter, is refused with `ValidationException` rather than silently
//!   restarting the listing. The two refusals are different answers and carry
//!   different modelled `reason`s: a token this service did not mint could not be
//!   read at all, so it is `cannotParse`; a token that WAS read and names another
//!   selection is a value `nextToken` does not accept for this request, so it is
//!   `fieldValidationFailed`, like every other member-level refusal here. The two
//!   also WORD their refusals differently: a token that could not be decoded is
//!   refused as a pagination-token exception naming this operation and the wrapping
//!   schema the supplied token's own payload names
//!   ([`pagination_token_exception`]), while a token that decoded and names another
//!   selection is refused as the member-level value it is.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ListRecommendedActions::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListRecommendedActions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/smithy-model/com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions.json")
//! ```
//!
//! Returns a list of recommended actions that match the filter criteria.
//!
//! ## Input
//!
//! * `filter` (RequestFilter) — The criteria that you want all returned recommended actions
//!   to match.
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response.
//! * `nextToken` (NextToken, len 1..8192, pattern ^[\S\s]*$) — The pagination token that
//!   indicates the next set of results that you want to retrieve.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..8192, pattern ^[\S\s]*$) — The pagination token that
//!   indicates the next set of results that you want to retrieve.
//! * `recommendedActions` (RecommendedActions) **required** — The list of recommended actions
//!   that satisfy the filter criteria.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of your request.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{member_strings, FilterName, MatchOption, StoredAction};
use aws_smithy_schema_server::constraints;
use base64::Engine as _;
use sha2::Digest;
use smithy_server_runtime::{Violation, ViolationKind};
use std::collections::BTreeSet;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListRecommendedActionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListRecommendedActionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bcmrecommendedactions#ListRecommendedActions")]
#[derive(Debug)]
pub enum ListRecommendedActionsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_recommended_actions::ListRecommendedActionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListRecommendedActionsError = crate::scaffolded::Scaffolded<ListRecommendedActionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListRecommendedActionsErrorModeled> for ListRecommendedActionsError {
    fn from(modeled: ListRecommendedActionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `ValidationExceptionReason$cannotParse` — the request carries a value this
/// service could not read at all (here: a malformed `nextToken`).
const REASON_CANNOT_PARSE: &str = "cannotParse";
/// `ValidationExceptionReason$fieldValidationFailed` — a named member carries a
/// value the service read and does not accept (here: a `nextToken` read as a
/// position in a different listing).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The modelled `MaxResults` range (`smithy_query(query="shape",
/// shape="MaxResults")`: range 1..1000).
const MAX_RESULTS_RANGE: (i32, i32) = (1, 1000);

/// The page size a request that names no `maxResults` is served at.
///
/// `maxResults` is "the maximum number of results to return in the response" and
/// the model caps it at [`MAX_RESULTS_RANGE`]`.1`, so that bound is the largest
/// page this operation can put in one response — whether or not the caller named a
/// size. Serving an unnamed size at the cap keeps every answer a page the model can
/// describe and keeps the listing terminating through `nextToken`, instead of
/// reporting an unbounded set no page size permits.
const DEFAULT_PAGE_SIZE: usize = MAX_RESULTS_RANGE.1 as usize;

/// Hex characters of the query fingerprint a `nextToken` carries (128 bits).
const TOKEN_FINGERPRINT_LEN: usize = 32;
/// Hex characters of the resume offset a `nextToken` carries.
const TOKEN_OFFSET_LEN: usize = 8;
/// Total length of a `nextToken` this service mints.
const TOKEN_LEN: usize = TOKEN_FINGERPRINT_LEN + TOKEN_OFFSET_LEN;

/// A 400 `ValidationException` stating `violations` the way this service's input
/// validator states them.
///
/// ONE rendering for every member this operation refuses, wherever the refusal is
/// found: the message is the `N validation error(s) detected: …` envelope built
/// from the violations, `reason` is the modelled
/// `ValidationExceptionReason$fieldValidationFailed`, and `fieldList` carries one
/// entry per violation — `name` the dotted member path, `message` that violation's
/// own clause. Both of those members are modelled (`reason` is REQUIRED), so a
/// client can branch on the reason and report which member was rejected and why.
///
/// Each violation is stated TWICE and the two statements carry different halves of
/// it: the envelope's clause names the member path and the requirement it failed
/// and does NOT echo the value the request sent, while the member's own `fieldList`
/// entry — whose `name` already carries that path — is where the rejected value is
/// reported. So a caller reading the summary is told what the service requires of
/// each member, and a caller reading a member's entry is told what it sent.
pub fn field_validation_failed(violations: &[Violation]) -> ListRecommendedActionsErrorModeled {
    // BOUNDED, like the boundary validator's own walk: every violation is rendered
    // TWICE into the body (a clause in the message and a `fieldList` entry) and each
    // clause interpolates the rejected value and the bound it failed, so an unbounded
    // count turns a small request into an arbitrarily large refusal. The cap is the
    // framework's (`MAX_VIOLATIONS`), and like the framework the envelope keeps
    // counting what was RECORDED, so the two refusal paths read alike.
    // Each violation as this service STATES it: the value set listed in this
    // service's order ([`as_stated`]).
    let stated: Vec<Violation> = violations
        .iter()
        .take(MAX_VIOLATIONS)
        .map(as_stated)
        .collect();
    // The SUMMARY clause names the member and the requirement it failed, and
    // withholds the value the request sent ([`without_echoed_value`]); the member's
    // own entry below states that value, which is the half a caller reads there.
    let summarised: Vec<Violation> = stated.iter().map(without_echoed_value).collect();
    ListRecommendedActionsErrorModeled::ValidationException(ValidationException {
        message: constraints::envelope(&summarised)
            .unwrap_or_else(|| "The input fails to satisfy the service's constraints.".to_string()),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(
            stated
                .iter()
                .map(|violation| ValidationExceptionField {
                    name: violation.path.clone(),
                    message: violation.message(),
                })
                .collect(),
        ),
    })
}

/// A violation as this service STATES it: the same violation, with an enumerated
/// value set re-listed in the order this service lists that set in
/// ([`FilterName::VALUE_SET_ORDER`]).
///
/// Needed because a violation found at the constraint boundary reads its value set
/// off the SCHEMA, so it arrives listed in the model's declaration order, while
/// the order a refusal states is the service's own. Violations are matched by the
/// value SET they name, so the re-listing reaches every enum member of every
/// request shape at any depth and from either refusal path — the boundary's walk
/// and this handler's own checks alike — and a value set this service publishes no
/// order for is stated exactly as it was found.
fn as_stated(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return violation.clone();
    };
    match stated_value_set(allowed) {
        Some(stated) => Violation {
            kind: ViolationKind::EnumValue(stated),
            ..violation.clone()
        },
        None => violation.clone(),
    }
}

/// The order this service lists `allowed` in, when `allowed` is a value set one of
/// its enums closes; `None` for any other set.
fn stated_value_set(allowed: &[String]) -> Option<Vec<String>> {
    let allowed: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
    [
        member_strings(FilterName::VALUE_SET_ORDER),
        member_strings(MatchOption::VALUE_SET_ORDER),
    ]
    .into_iter()
    .find(|stated| {
        stated.iter().map(String::as_str).collect::<BTreeSet<&str>>() == allowed
    })
}

/// The same violation with the rejected value WITHHELD, so the clause it renders
/// names the member and the requirement alone — the form the SUMMARY states.
///
/// The framework already renders a clause both ways and chooses by whether the
/// violation carries a value to show, so withholding the value here keeps the
/// summary's wording the framework's rather than respelling it in this crate.
fn without_echoed_value(violation: &Violation) -> Violation {
    Violation {
        value: None,
        sensitive: true,
        ..violation.clone()
    }
}

/// A 400 `ValidationException` for a request this service could not READ —
/// `ValidationExceptionReason$cannotParse`.
///
/// The counterpart of [`field_validation_failed`] for the OTHER class of refusal
/// the modelled reason distinguishes: a value that was read and refused versus one
/// that could not be read at all. `detail` is the reading failure's own
/// explanation, which the codec already words for a caller, and `members` names
/// each input member whose value could not be read, so a client is told WHERE the
/// request could not be read and not merely that it could not.
///
/// A reading failure that names no member — a body that is not this input's object
/// at all — carries no `fieldList`, which the model permits: `fieldList` is
/// optional where `reason` is REQUIRED.
pub fn cannot_parse_input(
    detail: &str,
    members: &[(String, String)],
) -> ListRecommendedActionsErrorModeled {
    // BOUNDED the way [`field_validation_failed`] is bounded: each entry renders a
    // member name and a clause about a request-chosen value, so both the count and
    // each string are capped before they reach the body.
    let field_list: Vec<ValidationExceptionField> = members
        .iter()
        .take(MAX_VIOLATIONS)
        .map(|(name, message)| ValidationExceptionField {
            name: echoed(name),
            message: echoed(message),
        })
        .collect();
    ListRecommendedActionsErrorModeled::ValidationException(ValidationException {
        message: echoed(detail),
        reason: REASON_CANNOT_PARSE.to_string(),
        field_list: (!field_list.is_empty()).then_some(field_list),
    })
}

/// A 400 `ValidationException` about ONE named member: `message` is the
/// overarching explanation, `reason` says which KIND of validation failure it was,
/// and the single `fieldList` entry names the member and why it failed.
///
/// The reason is a parameter because it is the one member of the refusal that
/// cannot be derived from the wording: it classifies the failure, so each caller
/// states the class it actually found ([`cannot_parse`] for a value that could not
/// be read, [`member_value_refused`] for one that was read and then refused).
fn member_refusal(
    reason: &str,
    message: impl Into<String>,
    field: (String, String),
) -> ListRecommendedActionsError {
    ListRecommendedActionsErrorModeled::ValidationException(ValidationException {
        message: message.into(),
        reason: reason.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: field.0,
            message: field.1,
        }]),
    })
    .into()
}

/// A 400 `ValidationException` for a value the service could not read at all —
/// `ValidationExceptionReason$cannotParse` — naming the member it could not read.
fn cannot_parse(
    message: impl Into<String>,
    field: (String, String),
) -> ListRecommendedActionsError {
    member_refusal(REASON_CANNOT_PARSE, message, field)
}

/// A 400 `ValidationException` for a value the service DID read and then refused
/// because it is not one this member accepts for this request —
/// `ValidationExceptionReason$fieldValidationFailed`, the same reason every other
/// member-level refusal of this operation carries ([`field_validation_failed`]) —
/// naming the member it refused.
///
/// Separate from [`cannot_parse`] because the two are different answers to a
/// client: `cannotParse` says the value was unreadable, so there is nothing to act
/// on but the member's syntax, while this says the value was understood and is
/// wrong for this request, which a client can correct by changing the request it
/// accompanies.
fn member_value_refused(
    message: impl Into<String>,
    field: (String, String),
) -> ListRecommendedActionsError {
    member_refusal(REASON_FIELD_VALIDATION_FAILED, message, field)
}

/// The dotted path of a member of the `index`-th `ActionFilter` in the request,
/// spelled through the framework's own path rule ([`constraints::path`]) so a path
/// this handler builds reads exactly like one the boundary validator builds:
/// `filter.actions.1.member.key`.
fn filter_member_path(index: usize, member: &str) -> String {
    constraints::path::join(&[
        constraints::path::member("filter"),
        constraints::path::member("actions"),
        constraints::path::element(index + 1),
        constraints::path::member(member),
    ])
}

/// One violation of an enumerated value set, stated the way the boundary validator
/// states one.
fn enum_violation(path: String, value: &str, allowed: Vec<String>) -> Violation {
    Violation {
        path,
        kind: ViolationKind::EnumValue(allowed),
        value: Some(echoed(value)),
        sensitive: false,
    }
}

/// A caller-supplied value, capped before a refusal echoes it: a message must not
/// amplify an arbitrarily long request value into the response. Both the cap
/// (`MAX_ECHOED_VALUE` = 256) and the elision text are the boundary validator's own
/// (`vendor/smithy-schema-server/src/constraints.rs`), so a long value refused here
/// reads exactly as the same value refused there. This is rendering only: the
/// ORIGINAL value is what the membership test above reads.
fn echoed(value: &str) -> String {
    const MAX_ECHOED_VALUE: usize = 256;
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// A 500 `InternalServerException`: the service could not reach its own store.
fn internal_server_error(message: impl Into<String>) -> ListRecommendedActionsError {
    ListRecommendedActionsErrorModeled::InternalServerException(InternalServerException {
        message: message.into(),
    })
    .into()
}


/// `FilterValues` is modelled `length: {min: 1}` (`smithy_query(query="shape",
/// shape="ActionFilter,FilterValues")`).
const FILTER_VALUES_MIN_LEN: i64 = 1;

/// Ceiling on the violations ONE refusal accumulates, matching the boundary
/// validator's own (`MAX_VIOLATIONS` in
/// `vendor/smithy-schema-server/src/constraints.rs`).
///
/// Every violation is rendered TWICE into the refusal body — a clause in the message
/// and a `fieldList` entry — and each clause carries the rejected value and the value
/// set it failed, so the COUNT is what turns a request-chosen list length into an
/// arbitrarily large response. The walk below STOPS at the cap (a caller cannot act on
/// thousands of clauses) and, like the framework, the envelope keeps counting what was
/// recorded.
const MAX_VIOLATIONS: usize = 100;

/// One validated `ActionFilter`: the field it reads, whether it keeps or excludes a
/// match, and the values it compares against.
#[derive(Debug)]
struct CompiledFilter {
    key: FilterName,
    match_option: MatchOption,
    values: Vec<String>,
}

impl CompiledFilter {
    /// Whether this filter keeps `action`.
    fn keeps(&self, action: &StoredAction) -> bool {
        let field = match self.key {
            FilterName::Feature => action.feature.as_str(),
            FilterName::Severity => action.severity.as_str(),
            FilterName::Type => action.action_type.as_str(),
        };
        let matched = self.values.iter().any(|value| value == field);
        match self.match_option {
            MatchOption::Equals => matched,
            MatchOption::NotEquals => !matched,
        }
    }

    /// This filter's contribution to the query fingerprint. Values are sorted and
    /// de-duplicated because a filter tests set membership, so `[A, B]` and
    /// `[B, A, B]` are the same query.
    ///
    /// Every component is encoded through [`push_component`], so a value that
    /// happens to contain a delimiter cannot be read as two components: the
    /// encoding of a filter determines the filter.
    fn canonical(&self) -> String {
        let mut values: Vec<&str> = self.values.iter().map(String::as_str).collect();
        values.sort_unstable();
        values.dedup();
        let mut canonical = String::new();
        push_component(&mut canonical, self.key.as_str());
        push_component(&mut canonical, self.match_option.as_str());
        push_component(&mut canonical, &values.len().to_string());
        for value in values {
            push_component(&mut canonical, value);
        }
        canonical
    }
}

/// Append one component of a canonical encoding to `canonical`, length-prefixed
/// as `<byte length>:<bytes>`.
///
/// The length prefix is what makes the encoding UNAMBIGUOUS, which is what a
/// fingerprint of a selection needs: each component is read by the length that
/// precedes it, never by scanning its bytes for a separator, so two encodings are
/// equal only when their component lists are equal — whatever characters a
/// component carries. The components fingerprinted here include `ActionFilter$values`
/// elements, arbitrary caller-supplied strings (`FilterValue`, pattern `[\S\s]*`),
/// so without the prefix one value carrying a separator would encode exactly like
/// several components and two DIFFERENT selections would share a fingerprint,
/// letting a token resume a listing whose records it never named.
fn push_component(canonical: &mut String, component: &str) {
    canonical.push_str(&component.len().to_string());
    canonical.push(':');
    canonical.push_str(component);
}

/// Validate the request's filters, refusing only what the model does not declare:
/// a `key` or `matchOption` outside its enum, and a `values` list shorter than its
/// modelled minimum.
///
/// EVERY offending member is reported in one refusal, the way the boundary
/// validator reports one: a violation per member, rendered by
/// [`field_validation_failed`].
///
/// `ActionFilter$key` and `$matchOption` target enum shapes, so an unmodelled
/// member of either is normally refused before this handler runs (by the
/// constraint boundary, whose violations this service answers in the same modelled
/// shape); the checks here are the backstop that turns such a value into the same
/// refusal rather than a silent default. The ELEMENTS of `ActionFilter$values` are
/// plain strings (`FilterValue`, length 1..1024, pattern `[\S\s]*`) whose only
/// value set is whatever the records happen to hold, so they are compared, never
/// validated: an element outside every candidate value set is a comparison that
/// matches nothing, not a bad request.
fn compile_filters(
    filter: Option<&RequestFilter>,
) -> Result<Vec<CompiledFilter>, ListRecommendedActionsError> {
    let Some(actions) = filter.and_then(|filter| filter.actions.as_ref()) else {
        return Ok(Vec::new());
    };

    let mut violations: Vec<Violation> = Vec::new();
    let mut compiled = Vec::with_capacity(actions.len());
    for (index, action_filter) in actions.iter().enumerate() {
        // Bounded refusal: stop accumulating (and stop walking) at the cap the
        // boundary validator uses. See [`MAX_VIOLATIONS`].
        if violations.len() >= MAX_VIOLATIONS {
            break;
        }
        let key = FilterName::from_wire(&action_filter.key);
        if key.is_none() {
            violations.push(enum_violation(
                filter_member_path(index, "key"),
                &action_filter.key,
                member_strings(FilterName::VALUE_SET_ORDER),
            ));
        }
        let match_option = MatchOption::from_wire(&action_filter.match_option);
        if match_option.is_none() {
            violations.push(enum_violation(
                filter_member_path(index, "matchOption"),
                &action_filter.match_option,
                member_strings(MatchOption::VALUE_SET_ORDER),
            ));
        }
        // An empty `values` list is refused rather than evaluated: `any()` over no
        // value is `false`, which would silently turn an `EQUALS` filter into
        // "matches nothing" and a `NOT_EQUALS` filter into a no-op.
        if (action_filter.values.len() as i64) < FILTER_VALUES_MIN_LEN {
            violations.push(Violation {
                path: filter_member_path(index, "values"),
                kind: ViolationKind::MinLength(FILTER_VALUES_MIN_LEN),
                value: Some(format!("{} items", action_filter.values.len())),
                sensitive: false,
            });
        }
        // The ELEMENTS of `values` are NOT validated here: the model closes no value
        // set over them, so each one is a string compared against a record's field by
        // `CompiledFilter::keeps`. An element no record carries keeps no record under
        // `EQUALS` and excludes none under `NOT_EQUALS`; its modelled length and
        // pattern are enforced at the constraint boundary.
        if let (Some(key), Some(match_option)) = (key, match_option) {
            compiled.push(CompiledFilter {
                key,
                match_option,
                values: action_filter.values.clone(),
            });
        }
    }
    if !violations.is_empty() {
        return Err(field_validation_failed(&violations).into());
    }
    Ok(compiled)
}

/// The fingerprint of a listing's SELECTION: the filters (order-independent, since
/// they are ANDed, and repetition-independent, since ANDing a filter with itself
/// keeps the same records). A `nextToken` carries it so a token cannot resume a
/// listing of a different selection, where the offset it names would land on an
/// unrelated record.
///
/// Equal selections fingerprint equally and different ones do not: the filters are
/// reduced to a SET (sorted, then de-duplicated) and every part of the encoding —
/// each filter, and each component within it — is length-prefixed by
/// [`push_component`], so no caller-supplied value can make one selection encode
/// like another.
///
/// `maxResults` is NOT part of it: a token names a position in the selection, and the
/// page size is a per-call hint the caller may change or omit while paginating.
fn query_fingerprint(filters: &[CompiledFilter]) -> String {
    let mut parts: Vec<String> = filters.iter().map(CompiledFilter::canonical).collect();
    parts.sort();
    parts.dedup();
    let mut canonical = String::from("filters=");
    push_component(&mut canonical, &parts.len().to_string());
    for part in &parts {
        push_component(&mut canonical, part);
    }
    let digest = hex::encode(sha2::Sha256::digest(canonical.as_bytes()));
    digest[..TOKEN_FINGERPRINT_LEN].to_string()
}

/// The opaque token that resumes this query at `offset`.
fn mint_token(fingerprint: &str, offset: usize) -> String {
    format!("{fingerprint}{offset:0width$x}", width = TOKEN_OFFSET_LEN)
}

/// How a `nextToken` whose payload this service cannot use is refused: the
/// pagination-token exception, naming the operation the token was presented to and
/// the wrapping schema the token's own payload names.
///
/// A pagination token is an OPAQUE payload the service wraps a resume position in,
/// so a token it cannot use is a payload whose wrapping it does not recognise —
/// which is what this refusal says, rather than merely that the member's value was
/// rejected: a caller is told the token failed to DECODE, and which schema its
/// payload claimed, and so cannot mistake it for a token that decoded and named
/// something else (that refusal is [`member_value_refused`], the member-level class
/// every other read-and-refused value of this input carries).
///
/// The schema number is read from the SUPPLIED token ([`payload_wrapping_schema`]),
/// never from anything this service holds, so two different tokens are refused with
/// what each of them actually carries.
fn pagination_token_exception(operation: &str, token: &str) -> ListRecommendedActionsError {
    cannot_parse(
        format!(
            "Pagination token exception in operation '{operation}': invalid payload wrapping \
             schema {}",
            payload_wrapping_schema(token)
        ),
        (
            "nextToken".to_string(),
            "not a pagination token issued by this service".to_string(),
        ),
    )
}

/// The wrapping schema a supplied token's decoded payload names: the first 16 bits
/// of that payload, read big-endian and SIGNED, the way a wrapped payload leads
/// with the schema it was wrapped under.
///
/// A token is base64 text, so the payload is its decoding, and only the LEADING
/// quantum is decoded — the schema is read out of the payload's first bytes, so a
/// token that is too short or too damaged to decode any further still names the
/// schema its first characters spell, and no caller-chosen length is decoded.
/// Characters base64 does not spell are skipped and the URL-safe alphabet's two
/// substitutions are read as the standard characters they stand for, so a token in
/// either alphabet names the same schema; a quantum the token runs out of reads as
/// zero bits, the same value an all-zero payload names.
fn payload_wrapping_schema(token: &str) -> i16 {
    let mut quantum: String = token
        .chars()
        .filter_map(standard_base64_char)
        .take(BASE64_QUANTUM)
        .collect();
    while quantum.len() < BASE64_QUANTUM {
        quantum.push(BASE64_ZERO_BITS);
    }
    // A full quantum of alphabet characters always decodes, so the payload below is
    // the three bytes those characters spell; `unwrap_or_default` only keeps the
    // reading total, and reads an undecodable quantum as an empty payload.
    let payload = base64::engine::general_purpose::STANDARD
        .decode(&quantum)
        .unwrap_or_default();
    let leading = |index: usize| payload.get(index).copied().unwrap_or(0);
    i16::from_be_bytes([leading(0), leading(1)])
}

/// `c` as the STANDARD base64 alphabet spells it: the URL-safe alphabet's two
/// substitutions read as the standard characters they stand for, and anything the
/// alphabet does not spell at all (padding, separators, whitespace) skipped.
fn standard_base64_char(c: char) -> Option<char> {
    match c {
        'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => Some(c),
        '-' => Some('+'),
        '_' => Some('/'),
        _ => None,
    }
}

/// Base64 characters per encoded quantum, and the character spelling six zero bits:
/// the quantum a token shorter than one is padded out with.
const BASE64_QUANTUM: usize = 4;
const BASE64_ZERO_BITS: char = 'A';

/// The offset `token` resumes at, or a `ValidationException` when it is not a token
/// this service minted over this selection.
///
/// `operation` is the operation the token was presented to, named in the refusal of
/// a token whose payload could not be used (see [`pagination_token_exception`]).
fn parse_token(
    token: &str,
    fingerprint: &str,
    operation: &str,
) -> Result<usize, ListRecommendedActionsError> {
    let malformed = || pagination_token_exception(operation, token);
    // A minted token is lower-case hex throughout, so anything else — including an
    // upper-cased copy of a real token — is a token this service did not mint, and is
    // reported as unreadable rather than as a token for another query.
    if token.len() != TOKEN_LEN || !token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err(malformed());
    }
    // READ, then refused: the token parsed into a fingerprint and an offset, and the
    // fingerprint names another selection — a value this member does not accept for
    // THIS request, not a value the service could not read. So it is stated as the
    // member-level validation failure it is, never as `cannotParse`.
    let (token_fingerprint, offset) = token.split_at(TOKEN_FINGERPRINT_LEN);
    if token_fingerprint != fingerprint {
        return Err(member_value_refused(
            "The nextToken was issued for a different request. Repeat the original filter when paginating.",
            (
                "nextToken".to_string(),
                "issued for a different filter".to_string(),
            ),
        ));
    }
    usize::from_str_radix(offset, 16).map_err(|_| malformed())
}

/// Project a stored record onto the wire `RecommendedAction`.
///
/// `context` is carried over as the ORDERED map it is stored in, so the JSON
/// object a response renders it as lists its keys in one order: the projection
/// keeps the store's key order instead of re-bucketing the entries under a fresh
/// per-map hash seed, which would let two identical reads of unchanged state
/// answer with different bytes.
fn to_wire(action: &StoredAction) -> RecommendedAction {
    RecommendedAction {
        id: Some(action.id.clone()),
        r#type: Some(action.action_type.as_str().to_string()),
        account_id: Some(action.account_id.clone()),
        severity: Some(action.severity.as_str().to_string()),
        feature: Some(action.feature.as_str().to_string()),
        context: Some(action.context.clone()),
        next_steps: Some(action.next_steps.clone()),
        last_updated_time_stamp: Some(action.last_updated_timestamp.clone()),
    }
}

/// The `ListRecommendedActions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListRecommendedActions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListRecommendedActions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListRecommendedActions<Eng>
{
    type Input = ListRecommendedActionsInput;
    type Output = ListRecommendedActionsOutput;
    type Error = ListRecommendedActionsError;

    const NAME: &'static str = "ListRecommendedActions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_RECOMMENDED_ACTIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_RECOMMENDED_ACTIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // Refuse a bad filter BEFORE touching the store, so a rejected request is
        // refused on its own terms whatever the store holds.
        let filters = compile_filters(input.filter.as_ref())?;
        let fingerprint = query_fingerprint(&filters);

        // `maxResults` absent means "as large a page as this operation serves":
        // AWS documents no default for this operation, but the model caps a page at
        // `MaxResults`'s maximum, so an unnamed size is served at that cap
        // ([`DEFAULT_PAGE_SIZE`]) and the rest of the selection is reached through
        // `nextToken`. The model's `MaxResults` range (1..1000) is refused at the
        // constraint boundary before this handler runs; the same bound is enforced
        // here as a backstop, because a page size of 0 would otherwise report an
        // empty page while still owing a `nextToken` — a listing that never
        // terminates. The range check and the conversion share ONE refusal, so
        // neither can fall through to a silently unbounded page, and it is stated as
        // the same violation the boundary states.
        let page_size = match input.max_results {
            Some(max_results) => usize::try_from(max_results)
                .ok()
                .filter(|_| (MAX_RESULTS_RANGE.0..=MAX_RESULTS_RANGE.1).contains(&max_results))
                .ok_or_else(|| {
                    let kind = if max_results < MAX_RESULTS_RANGE.0 {
                        ViolationKind::MinValue(MAX_RESULTS_RANGE.0.to_string())
                    } else {
                        ViolationKind::MaxValue(MAX_RESULTS_RANGE.1.to_string())
                    };
                    let violation = Violation {
                        path: constraints::path::member("maxResults"),
                        kind,
                        value: Some(max_results.to_string()),
                        sensitive: false,
                    };
                    ListRecommendedActionsError::from(field_validation_failed(&[violation]))
                })?,
            None => DEFAULT_PAGE_SIZE,
        };

        let resume_at = match input.next_token.as_deref() {
            Some(token) => parse_token(token, &fingerprint, Self::NAME)?,
            None => 0,
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            internal_server_error(
                "the recommended-actions store is unavailable on this engine".to_string(),
            )
        })?;

        // The matching records, in store (insertion) order, so the page boundary a
        // token names stays on the same record across calls.
        let matching: Vec<RecommendedAction> = {
            let guard = state.lock();
            guard
                .recommended_actions
                .actions()
                .iter()
                .filter(|action| filters.iter().all(|filter| filter.keeps(action)))
                .map(to_wire)
                .collect()
        };

        // A token can outlive the records it was minted over (the service recomputes
        // its recommendations), so a resume point past the end reports an empty final
        // page rather than failing a caller that is simply done.
        let start = resume_at.min(matching.len());
        let end = start.saturating_add(page_size).min(matching.len());
        let page = matching[start..end].to_vec();
        // A token is owed only when this page made progress AND records remain, so no
        // answer can ever hand back a token that would repeat the same page.
        let next_token =
            (end > start && end < matching.len()).then(|| mint_token(&fingerprint, end));

        Ok(ListRecommendedActionsOutput {
            recommended_actions: page,
            next_token,
        })
    }
}
