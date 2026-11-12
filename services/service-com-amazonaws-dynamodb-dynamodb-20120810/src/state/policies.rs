// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy a table carries, and the decision it drives.
//!
//! What makes a document one this emulator can attach, the statements it is read as,
//! and whether the policies in force let one caller take one action on one table. They
//! live beside [`super`] rather than inside it because they are about a DOCUMENT and
//! the decision it drives, while the parent module is the table and item store itself;
//! `super` re-exports them, so every caller still names them `crate::state::…`.

use super::*;

/// Why a resource-based policy document is not one this emulator can attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyFault {
    /// Past the documented size limit ([`MAX_RESOURCE_POLICY_BYTES`]), which is a bound
    /// on the member's VALUE and states only the limit it crossed.
    TooLarge,
    /// Not a policy document at all, carrying what is wrong with it — a
    /// `ValidationException`.
    Invalid(String),
}

/// The clause every refusal of a policy DOCUMENT is stated behind: AWS words a document
/// defect as an invalid parameter value naming the document, and then says what is wrong
/// with it ("One or more parameter values were invalid: Invalid policy document: This
/// policy contains invalid Json" for a document that is not JSON at all).
pub(super) const INVALID_POLICY_DOCUMENT_CLAUSE: &str =
    "One or more parameter values were invalid: Invalid policy document";

/// How a document that is not JSON is refused: AWS's FIXED sentence for that defect,
/// which reports nothing of WHERE the parse stopped — a position is a fact of this
/// parser, not of the request.
pub(super) fn invalid_policy_json() -> PolicyFault {
    PolicyFault::Invalid(format!(
        "{INVALID_POLICY_DOCUMENT_CLAUSE}: This policy contains invalid Json"
    ))
}

/// How a document defect other than unparseable JSON is refused: the same clause, over
/// what is wrong with the document.
pub(super) fn invalid_policy_document(detail: &str) -> PolicyFault {
    PolicyFault::Invalid(format!("{INVALID_POLICY_DOCUMENT_CLAUSE}: {detail}"))
}

/// The `Version` an IAM policy document that states none is read with: "If you do not
/// include a Version element, the value defaults to 2008-10-17" (the IAM policy grammar
/// reference), so a document attached without one is stored carrying that version — which
/// is what a later read of it reports.
pub const DEFAULT_POLICY_VERSION: &str = "2008-10-17";

/// The service every action of a DynamoDB resource-based policy belongs to: a policy
/// attached to a DynamoDB resource governs DynamoDB actions, so an action of another
/// service could never be decided by it.
const POLICY_ACTION_SERVICE: &str = "dynamodb";

/// A resource-based policy attached to a table.
///
/// The document is kept as the service READ it, because `GetResourcePolicy` "returns the
/// resource-based policy document attached to the resource ... in JSON format": a
/// document that states a `Version` is kept exactly as it arrived, and one that states
/// none is kept carrying the version IAM reads it under ([`DEFAULT_POLICY_VERSION`]), so
/// a later read reports the document the decision is actually made on. Its statements are
/// parsed ONCE here so evaluating a data-plane request never re-parses it. The document is
/// capped at [`MAX_RESOURCE_POLICY_BYTES`] before it is parsed, which is what bounds the
/// statements, values and patterns one request can ask the matcher below for.
#[derive(Debug, Clone)]
pub struct AttachedPolicy {
    /// The document as it was attached, carrying the `Version` it is read under.
    pub document: String,
    /// This revision's TOKEN, rendered on the wire as the `RevisionId` by
    /// [`Self::revision_id`]: the instant the revision was made, in milliseconds since
    /// the epoch ([`policy_revision_token`]).
    ///
    /// A revision id is a token of the SERVICE's own, never a digest of the document: the
    /// modelled `PolicyRevisionId` is a 1..255-character string with no pattern
    /// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#PolicyRevisionId")`),
    /// and what a caller does with one is compare it — pass it back as an
    /// `ExpectedRevisionId` to make a write conditional on the revision it read. So two
    /// revisions carrying the same text are still told apart, the tokens of one policy's
    /// revisions are ordered, and a revision never reuses the token of the one it
    /// replaced ([`Self::revise_after`]).
    pub revision: u64,
    /// When the clock owes this policy its application to the data plane.
    ///
    /// "PutResourcePolicy is an asynchronous operation" and "the policy application is
    /// eventually consistent" (the `PutResourcePolicy` doc), so a request made straight
    /// after the attach is still authorized as it was — while a policy that arrives
    /// WITH the table is applied at once, because "when you attach a resource-based
    /// policy while creating a table, the policy application is strongly consistent"
    /// (`CreateTable$ResourcePolicy`). The attaching operation chooses which of the two
    /// it is; nothing sleeps for the span.
    pub effective_at: SystemTime,
    /// The parsed statements, in document order.
    pub(super) statements: Vec<PolicyStatement>,
}

impl AttachedPolicy {
    /// Parse `document` into the policy to attach, or say why it cannot be attached.
    ///
    /// `effective_at` is when the data plane starts honouring it (see the field) and
    /// `revised_at` is when the revision is being made, which is the instant its
    /// `RevisionId` names ([`policy_revision_token`]). The policy is numbered as the
    /// revision that replaces the one the resource carries when the attaching operation
    /// places it there ([`Self::revise_after`]).
    pub fn attach(
        document: &str,
        effective_at: SystemTime,
        revised_at: SystemTime,
    ) -> Result<Self, PolicyFault> {
        if document.len() > MAX_RESOURCE_POLICY_BYTES {
            return Err(PolicyFault::TooLarge);
        }
        let parsed: serde_json::Value =
            serde_json::from_str(document).map_err(|_| invalid_policy_json())?;
        let object = parsed.as_object().ok_or_else(invalid_policy_json)?;
        let listed: Vec<&serde_json::Value> = match object.get("Statement") {
            Some(serde_json::Value::Array(list)) => list.iter().collect(),
            Some(single @ serde_json::Value::Object(_)) => vec![single],
            _ => {
                return Err(invalid_policy_document(
                    "Policy must contain a valid Statement",
                ))
            }
        };
        let mut statements = Vec::with_capacity(listed.len());
        for statement in listed {
            statements.push(PolicyStatement::parse(statement)?);
        }
        // "If you do not include a Version element, the value defaults to 2008-10-17":
        // the document is stored as the service READ it, so a later read of the policy
        // reports the version it is being evaluated under rather than leaving the client
        // to apply the default itself. A document that states a version is kept exactly
        // as it arrived.
        let document = match object.contains_key("Version") {
            true => document.to_string(),
            false => versioned_document(object),
        };
        Ok(Self {
            revision: policy_revision_token(revised_at),
            document,
            effective_at,
            statements,
        })
    }

    /// `RevisionId` as the wire carries it: this revision's token in decimal.
    pub fn revision_id(&self) -> String {
        self.revision.to_string()
    }

    /// Number this policy as the revision that REPLACES `previous`, the policy the
    /// resource carries now (`None` while it carries none).
    ///
    /// A re-attach of the document the resource already carries keeps the token that
    /// revision has: "PutResourcePolicy is an idempotent operation; running it multiple
    /// times on the same resource using the same policy document will return the same
    /// revision id" (the `PutResourcePolicy` doc) — compared on the document as the
    /// service STORED it, which is the document a read of the policy reports and the one
    /// the decision is made on.
    ///
    /// Any other document is a NEW revision, and a revision is never handed the token of
    /// one before it: two revisions made inside the same millisecond would otherwise
    /// share a token and a client could not tell which of them it holds, so such a
    /// revision takes the next token after the one it replaces.
    pub fn revise_after(&mut self, previous: Option<&Self>) {
        let Some(previous) = previous else {
            return;
        };
        if previous.document == self.document {
            self.revision = previous.revision;
        } else if self.revision <= previous.revision {
            self.revision = previous.revision.saturating_add(1);
        }
    }

    /// Whether this policy EXPLICITLY DENIES `action` on `resource_arn` to a caller any
    /// of `principals` names, and whether the statement that denied it carried a
    /// `Condition` the emulator did not evaluate.
    ///
    /// Only a Deny decides anything here: "an explicit deny in any policy overrides any
    /// allow" (the IAM policy evaluation logic), so a Deny that matches the request is the
    /// one outcome this half of the decision reaches. `principals` is every spelling the
    /// caller may be named by — its own ARN, the ARN of the role an assumed-role session
    /// is a session of, and the spellings that name its whole account, since a statement
    /// denying the account denies each of its principals. What GRANTS is
    /// [`Self::grants_any`], which is decided against the caller's own spellings alone.
    pub fn denies_any(
        &self,
        principals: &[String],
        action: &str,
        resource_arn: &str,
    ) -> Option<bool> {
        self.statements
            .iter()
            .find(|statement| statement.denies(principals, action, resource_arn))
            .map(PolicyStatement::conditioned)
    }

    /// Whether this policy denies `action` on `resource_arn` to a caller any of
    /// `principals` names in EVERY request context — a matching Deny that carries no
    /// `Condition`.
    ///
    /// What [`denies_any`](Self::denies_any) reports with its condition flag set is a
    /// deny of only the requests whose context satisfies the condition, so it is not an
    /// answer to "would this document refuse the action whatever the request looks
    /// like"; this is. Used where the question is what the document DEFINITELY does
    /// ([`locks_the_caller_out`]) rather than what to do with one request.
    pub fn denies_unconditionally(
        &self,
        principals: &[String],
        action: &str,
        resource_arn: &str,
    ) -> bool {
        self.statements.iter().any(|statement| {
            statement.denies(principals, action, resource_arn) && !statement.conditioned()
        })
    }

    /// Whether this policy GRANTS `action` on `resource_arn` to a caller any of
    /// `principals` names.
    ///
    /// A resource-based policy's `Allow` is a permission of its own where it names the
    /// CALLER: "the resource-based policy and the identity-based policy are evaluated
    /// together, and access is granted if either allows it". Only the caller's OWN
    /// spellings may be passed — a statement naming the account delegates to that
    /// account's identity-based policies rather than granting (see [`authorization`]).
    pub fn grants_any(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.statements
            .iter()
            .any(|statement| statement.grants(principals, action, resource_arn))
    }
}

/// Which DynamoDB resource a resource-based policy is attached to: "Attaches a
/// resource-based policy document to the resource, which can be a table or stream"
/// (the `PutResourcePolicy` doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyTarget {
    /// The table itself, whose policy governs the table's own actions.
    Table,
    /// The table's stream, whose policy governs the stream's actions and is held
    /// independently of the table's ([`Table::stream_resource_policy`]).
    Stream,
}

/// The resource a `*ResourcePolicy` call's `ResourceArn` names: the region it lives in,
/// the table it belongs to, which of the two resources it IS, and the ARN as the request
/// spelled it (the only identifier such a request carries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyResource {
    pub region: String,
    pub table: String,
    pub arn: String,
    pub target: PolicyTarget,
}

/// How an ARN that is not one this surface addresses at all is refused: AWS's sentence
/// for a resource-policy `ResourceArn` naming something other than a table or a stream —
/// an index, a backup, another service's resource — echoing the value the request sent.
pub fn only_table_or_stream_message(resource_arn: &str) -> String {
    format!(
        "One or more parameter values were invalid: Invalid resource arn provided, only table \
         or stream is accepted. Provided resource arn: {resource_arn}"
    )
}

/// How an ARN of the right SHAPE that does not address a resource of this endpoint —
/// another region, account or partition — is refused: AWS's sentence for an ARN that is
/// not this service's to resolve, echoing the value the request sent.
pub fn invalid_policy_resource_arn_message(resource_arn: &str) -> String {
    format!("One or more parameter values were invalid: Invalid resource Arn: {resource_arn}")
}

/// Resolve a `*ResourcePolicy` call's `ResourceArn` to the resource it names, in the
/// scope the request itself is made in.
///
/// The one place an ARN is classified for the resource-policy surface, in the same two
/// parts the tagging surface is classified in ([`resolve_resource_arn`]) and with this
/// surface's own two sentences:
///
/// * SHAPE — the value must be an ARN of this service naming a TABLE
///   (`…:table/<name>`) or that table's STREAM (`…:table/<name>/stream/<label>`), which
///   are the two resources a policy attaches to; anything else, an index ARN included,
///   is [`only_table_or_stream_message`];
/// * SCOPE — the ARN must address a resource of the endpoint the request reached: the
///   same partition, account and REGION this request is signed for. Anything else is
///   [`invalid_policy_resource_arn_message`] — an out-of-scope ARN is not "absent here",
///   it is unaddressable from here.
///
/// Either way the value names no resource whose existence could be looked up, so it is
/// an INPUT failure (400 `ValidationException`) and not a missing resource.
pub fn resolve_policy_resource_arn(
    resource_arn: &str,
    request_region: &str,
) -> Result<PolicyResource, RequestError> {
    let out_of_scope =
        || RequestError::Validation(invalid_policy_resource_arn_message(resource_arn));
    let unsupported = || RequestError::Validation(only_table_or_stream_message(resource_arn));
    // A value that is not an ARN at all is refused by THAT rule, whichever ARN-addressed
    // surface it was sent to ([`invalid_resource_arn`]): a caller that sent a bare
    // resource name is told what the member takes.
    if !resource_arn.starts_with(ARN_PREFIX) {
        return Err(invalid_resource_arn(resource_arn));
    }
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and may itself carry `/`, so the split stops at six.
    let fields: Vec<&str> = resource_arn.splitn(6, ':').collect();
    let (partition, service, region, resource) = match fields.as_slice() {
        ["arn", partition, service, region, _account, resource] => {
            (*partition, *service, *region, *resource)
        }
        _ => return Err(out_of_scope()),
    };
    if partition.is_empty() || region.is_empty() {
        return Err(out_of_scope());
    }
    if service != "dynamodb" {
        return Err(unsupported());
    }
    let path = resource.strip_prefix("table/").ok_or_else(unsupported)?;
    let (name, target) = match path.split_once('/') {
        None => (path, PolicyTarget::Table),
        Some((name, sub)) => match sub.strip_prefix("stream/") {
            // A stream label is minted by this service, so what it has to be is
            // non-empty; which stream it names is the lookup's question, not the shape's.
            Some(label) if !label.is_empty() => (name, PolicyTarget::Stream),
            _ => return Err(unsupported()),
        },
    };
    let table = resolve_table_name(name).map_err(|_| unsupported())?;
    // The scope half of the rule, decided over the TABLE ARN this endpoint would mint:
    // the stream suffix is this service's own and is already read above.
    let scoped = match target {
        PolicyTarget::Table => resource_arn == table_arn(request_region, &table),
        PolicyTarget::Stream => resource_arn
            .starts_with(&format!("{}/stream/", table_arn(request_region, &table))),
    };
    if !scoped {
        return Err(out_of_scope());
    }
    Ok(PolicyResource {
        region: request_region.to_string(),
        table,
        arn: resource_arn.to_string(),
        target,
    })
}

/// The actions a STREAM's policy may carry: the Streams data plane's own actions ("the
/// following actions are supported for stream resource-based policies" — the Amazon
/// DynamoDB Developer Guide's "Resource-based policies" entry for streams), each named
/// without its service prefix, which [`foreign_service_action`] has already held every
/// action to.
const STREAM_POLICY_ACTIONS: [&str; 4] = [
    "getrecords",
    "getsharditerator",
    "describestream",
    "liststreams",
];

/// Hold every action of a STREAM's policy to the actions a stream has.
///
/// A policy attached to a stream decides requests to that stream, so a TABLE action in
/// it names a permission the stream could never grant or refuse — the same rule that
/// refuses another service's action in either policy ([`foreign_service_action`]), one
/// resource further in. A wildcard (`*`, `dynamodb:Get*`) is left alone: it selects
/// whatever actions the resource has.
pub fn check_stream_policy_actions(policy: &AttachedPolicy) -> Result<(), PolicyFault> {
    for statement in &policy.statements {
        for value in &statement.action.values {
            let named = value
                .split_once(':')
                .map_or(value.as_str(), |(_, action)| action)
                .to_ascii_lowercase();
            if named.contains('*') {
                continue;
            }
            if !STREAM_POLICY_ACTIONS.contains(&named.as_str()) {
                return Err(invalid_policy_document(&format!(
                    "Policy has an action that is not supported for the resource: {value}"
                )));
            }
        }
    }
    Ok(())
}

/// The fixed opening of every `PolicyNotFoundException` message, whichever
/// resource-policy operation raises it and whichever of the two situations the
/// modelled error covers ("tried to access a nonexistent resource-based policy" and
/// "a policy is present for the resource but its revision ID didn't match the
/// expected value"). What follows it is the detail built from the request and from
/// this store: see [`policy_not_found_message`] and
/// [`policy_revision_mismatch_message`].
pub const POLICY_NOT_FOUND_PREFIX: &str =
    "Resource-based policy not found for the provided ResourceArn: ";

/// The refusal of an UNCONDITIONED request for a policy that is not there: the fixed
/// prefix and the `ResourceArn` the request sent, spelled as the request spelled it.
pub fn policy_not_found_message(resource_arn: &str) -> String {
    format!("{POLICY_NOT_FOUND_PREFIX}{resource_arn}")
}

/// The refusal of a request whose `ExpectedRevisionId` the target does not satisfy:
/// the fixed prefix, the revision id the request asked for, and what the target
/// actually carries — the revision id of the policy attached to it, or that it
/// carries no policy at all. The two tails are what lets a client doing optimistic
/// concurrency tell a stale revision apart from a policy that is gone.
pub fn policy_revision_mismatch_message(
    expected_revision_id: &str,
    table: &str,
    attached_revision_id: Option<&str>,
) -> String {
    match attached_revision_id {
        Some(current) => format!(
            "{POLICY_NOT_FOUND_PREFIX}Requested update for policy with revision id \
             {expected_revision_id}, but the policy associated to target table {table} has \
             revision id {current}."
        ),
        None => format!(
            "{POLICY_NOT_FOUND_PREFIX}Requested update for policy with revision id \
             {expected_revision_id}, but table {table} has no associated policy."
        ),
    }
}

/// How a document past the documented size limit is refused: the size limit on a
/// resource-based policy document ([`MAX_RESOURCE_POLICY_BYTES`]) is a bound on the
/// VALUE of a member, so crossing it is a parameter-value refusal naming the limit, and
/// nothing of the document or the resource it was offered for.
pub fn policy_too_large_message() -> String {
    format!(
        "One or more parameter values were invalid: Policy size exceeds the maximum allowed \
         size of {MAX_RESOURCE_POLICY_BYTES} bytes"
    )
}

/// One document carrying the `Version` an IAM document that states none is read with
/// ([`DEFAULT_POLICY_VERSION`]), beside the rest of the document as it arrived.
fn versioned_document(object: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut versioned = serde_json::Map::new();
    versioned.insert(
        "Version".to_string(),
        serde_json::Value::String(DEFAULT_POLICY_VERSION.to_string()),
    );
    for (key, value) in object {
        versioned.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(versioned).to_string()
}

/// The revision token a policy revision made at `at` carries: that instant in
/// milliseconds since the epoch.
///
/// A `RevisionId` is a short opaque numeric token that changes per revision, and what a
/// caller does with one is compare it — hold it and pass it back as an
/// `ExpectedRevisionId`. The instant of the revision is such a token: it changes with
/// every revision, the tokens of one policy's revisions are ordered, and it says nothing
/// about the document, so a document re-attached unchanged is still a revision of its own
/// to everything but the idempotent re-attach ([`AttachedPolicy::revise_after`]).
pub fn policy_revision_token(at: SystemTime) -> u64 {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(since_epoch).unwrap_or(u64::MAX)
}

/// One statement of a resource-based policy, reduced to what decides a request.
#[derive(Debug, Clone)]
pub(super) struct PolicyStatement {
    /// `Effect: Deny`. An Allow statement is parsed and kept so the document round
    /// trips, but it changes no outcome (see [`AttachedPolicy::denies`]).
    pub(super) deny: bool,
    pub(super) principal: Selector,
    pub(super) action: Selector,
    pub(super) resource: Selector,
    /// Whether the statement carries a `Condition`.
    ///
    /// A condition is matched against the REQUEST CONTEXT (the source VPC, the time of
    /// day, a tag on the caller), and this emulator has no identity or network context
    /// to supply, so it cannot evaluate one. A Deny that carries one is therefore
    /// applied anyway — FAIL CLOSED: the emulator never serves a request the attached
    /// policy might refuse, which is the error that matters for a policy whose whole
    /// job is to refuse. The flag is kept so the refusal can say the condition was not
    /// evaluated.
    pub(super) conditioned: bool,
}

impl PolicyStatement {
    /// Parse one statement, refusing a document this emulator would otherwise store
    /// and then not honour.
    pub(super) fn parse(value: &serde_json::Value) -> Result<Self, PolicyFault> {
        let object = value.as_object().ok_or_else(|| {
            PolicyFault::Invalid("Each Statement must be a JSON object".to_string())
        })?;
        let conditioned = object.contains_key("Condition");
        let effect = object
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                PolicyFault::Invalid(
                    "Each Statement must carry an Effect of Allow or Deny".to_string(),
                )
            })?;
        let deny = if effect.eq_ignore_ascii_case("Deny") {
            true
        } else if effect.eq_ignore_ascii_case("Allow") {
            false
        } else {
            return Err(PolicyFault::Invalid(format!(
                "Statement Effect must be Allow or Deny, got {}",
                clipped(effect)
            )));
        };
        // "Resource-based policies ... require a Principal" (the IAM user guide's
        // policy grammar), so a statement without one is not a resource policy.
        let principal = Selector::parse(object, "Principal", "NotPrincipal", principal_values)?
            .ok_or_else(|| {
                PolicyFault::Invalid(
                    "Each Statement of a resource-based policy must carry a Principal".to_string(),
                )
            })?;
        let action = Selector::parse(object, "Action", "NotAction", string_values)?.ok_or_else(
            || invalid_policy_document("Policy statement must contain actions"),
        )?;
        // A policy attached to a DynamoDB resource decides DynamoDB requests, so an
        // action of another service could never be decided by it: the document names a
        // permission this resource cannot grant or refuse, and is refused rather than
        // stored and never honoured.
        for value in &action.values {
            if let Some(foreign) = foreign_service_action(value) {
                return Err(invalid_policy_document(&format!(
                    "Policy has an action that is not supported for the resource: {foreign}"
                )));
            }
        }
        // "Resource-based policies ... require a Principal", and a PUBLIC one — `*`,
        // which names every identity there is — would grant the resource to anyone, so
        // an Allow may not carry it. A DENY of everyone takes nothing away from anyone
        // in particular and stands.
        if !deny && principal.is_public() {
            return Err(invalid_policy_document(
                "Policy statement must not grant access to a public principal",
            ));
        }
        // A resource-based policy is attached to ONE resource, so a statement that
        // names no Resource applies to the resource it is attached to.
        let resource = Selector::parse(object, "Resource", "NotResource", string_values)?
            .unwrap_or_else(Selector::everything);
        Ok(Self {
            deny,
            principal,
            action,
            resource,
            conditioned,
        })
    }

    /// Whether this statement denies the request.
    ///
    /// A `Condition` is not evaluated (see the field's doc) and does not stop a Deny
    /// from applying. `principals` is every spelling the caller may be named by; a
    /// statement naming any of them is about this request.
    pub(super) fn denies(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.deny && self.about(principals, action, resource_arn)
    }

    /// Whether this statement GRANTS the request.
    ///
    /// A conditioned `Allow` grants nothing: the condition is not evaluated (see the
    /// field's doc), so the same fail-closed reading that applies a conditioned Deny
    /// withholds a conditioned grant — the emulator never serves on a condition it did
    /// not check.
    pub(super) fn grants(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        !self.deny && !self.conditioned && self.about(principals, action, resource_arn)
    }

    /// Whether this statement is ABOUT the request at all: its principal, action and
    /// resource elements each select it.
    pub(super) fn about(&self, principals: &[String], action: &str, resource_arn: &str) -> bool {
        self.principal
            .matches(|value| principal_names(value, principals))
            && self.action.matches(|pattern| action_matches(pattern, action))
            && self
                .resource
                .matches(|pattern| resource_matches(pattern, resource_arn))
    }

    /// Whether this statement's decision rests on a `Condition` the emulator did not
    /// evaluate, so a refusal can say so.
    pub(super) fn conditioned(&self) -> bool {
        self.conditioned
    }
}

/// One statement element: the values it lists, and whether the statement applies to
/// those values (`Action`) or to everything BUT them (`NotAction`).
#[derive(Debug, Clone)]
pub(super) struct Selector {
    pub(super) values: Vec<String>,
    pub(super) negated: bool,
}

impl Selector {
    /// Read `positive` or its `Not…` counterpart out of a statement. A statement that
    /// carries both forms of one element is not a policy document.
    pub(super) fn parse(
        object: &serde_json::Map<String, serde_json::Value>,
        positive: &'static str,
        negative: &'static str,
        extract: fn(&serde_json::Value) -> Option<Vec<String>>,
    ) -> Result<Option<Self>, PolicyFault> {
        let (raw, negated) = match (object.get(positive), object.get(negative)) {
            (Some(_), Some(_)) => {
                return Err(PolicyFault::Invalid(format!(
                    "A Statement may carry {positive} or {negative}, not both"
                )))
            }
            (Some(value), None) => (value, false),
            (None, Some(value)) => (value, true),
            (None, None) => return Ok(None),
        };
        let values = extract(raw).ok_or_else(|| {
            PolicyFault::Invalid(format!(
                "{positive} must be a string, a list of strings, or a map of principal kinds"
            ))
        })?;
        Ok(Some(Self { values, negated }))
    }

    /// The element a statement that names none of its own carries: everything.
    pub(super) fn everything() -> Self {
        Self {
            values: vec!["*".to_string()],
            negated: false,
        }
    }

    /// Whether this element names EVERY identity there is: the bare `*` wildcard, listed
    /// positively. A negated element (`NotPrincipal`) names everyone BUT what it lists,
    /// which is a different statement and not a public one.
    pub(super) fn is_public(&self) -> bool {
        !self.negated && self.values.iter().any(|value| value == "*")
    }

    /// Whether the request's value satisfies this element, honouring the negation.
    pub(super) fn matches(&self, matched: impl Fn(&str) -> bool) -> bool {
        self.values.iter().any(|value| matched(value)) != self.negated
    }
}

/// The action of ANOTHER service one `Action` value names, or `None` when it is one this
/// resource's policy can decide.
///
/// An IAM action is `<service>:<action>`, and the bare `*` wildcard stands for every
/// action of the resource it is attached to rather than for another service's.
pub(super) fn foreign_service_action(value: &str) -> Option<&str> {
    if value == "*" {
        return None;
    }
    let (service, _) = value.split_once(':')?;
    if service.eq_ignore_ascii_case(POLICY_ACTION_SERVICE) {
        return None;
    }
    Some(value)
}

/// A statement element given as a string or a list of strings.
pub(super) fn string_values(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(text) => Some(vec![text.clone()]),
        serde_json::Value::Array(list) => list
            .iter()
            .map(|entry| entry.as_str().map(str::to_string))
            .collect(),
        _ => None,
    }
}

/// A `Principal` element: `"*"`, or a map from principal kind to one or more values.
///
/// A kind other than `AWS` — `Service`, `Federated`, `CanonicalUser` — names no IAM
/// identity, so its values are kept WITH their kind and can match no caller rather
/// than being silently dropped.
pub(super) fn principal_values(value: &serde_json::Value) -> Option<Vec<String>> {
    match value {
        serde_json::Value::String(_) | serde_json::Value::Array(_) => string_values(value),
        serde_json::Value::Object(map) => {
            let mut values = Vec::new();
            for (kind, listed) in map {
                let listed = string_values(listed)?;
                if kind.eq_ignore_ascii_case("AWS") {
                    values.extend(listed);
                } else {
                    values.extend(listed.into_iter().map(|value| format!("{kind}:{value}")));
                }
            }
            Some(values)
        }
        _ => None,
    }
}

/// The identity every request to this emulator carries when nothing more is known about
/// it: the root of the single account it serves.
///
/// What the POLICY-MANAGEMENT lock-out is decided against ([`locks_the_caller_out`]),
/// which asks whether a document would take the ACCOUNT's own ability to change that
/// policy away. Who a DATA-PLANE request speaks for is resolved from the credential it
/// signed with instead ([`authorization::request_caller`]).
pub fn caller_principal() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// Whether a policy `Principal` value names the identity this emulator serves.
///
/// `principals` is every spelling the caller may be named by — its own ARN, the role
/// behind an assumed-role session, and (where a DENY is being decided) the ARN and bare
/// id of its account, because a statement about the account is about each of its
/// principals. A value matches when it names any of them, with IAM's wildcards.
///
/// A principal of a kind that names no IAM identity — a `Service`, a `Federated` or a
/// `CanonicalUser` — is kept WITH its kind by [`principal_values`], so it matches no
/// spelling here rather than being read as one.
pub(super) fn principal_names(value: &str, principals: &[String]) -> bool {
    principals
        .iter()
        .any(|named| wildcard_match(value, named))
}

/// Whether a policy `Action` value names the action being performed. IAM action names
/// are case-insensitive.
pub(super) fn action_matches(pattern: &str, action: &str) -> bool {
    wildcard_match(
        &pattern.to_ascii_lowercase(),
        &action.to_ascii_lowercase(),
    )
}

/// Whether a policy `Resource` value names the resource the request names.
///
/// The partition, region and account segments of an ARN are not compared: this
/// emulator serves one partition and one account, and a policy is attached to a table
/// in a known region, so what distinguishes one resource from another here is the
/// SERVICE and the resource path (`table/<name>`, `table/<name>/index/<index>`).
pub(super) fn resource_matches(pattern: &str, resource_arn: &str) -> bool {
    if wildcard_match(pattern, resource_arn) {
        return true;
    }
    match (arn_tail(pattern), arn_tail(resource_arn)) {
        (Some(pattern), Some(actual)) => wildcard_match(&pattern, &actual),
        _ => false,
    }
}

/// The service and resource path of an ARN — `dynamodb:table/x` — or `None` when the
/// value is not an ARN at all.
pub(super) fn arn_tail(arn: &str) -> Option<String> {
    let mut fields = arn.strip_prefix("arn:")?.splitn(5, ':');
    let (_partition, service) = (fields.next()?, fields.next()?);
    let (_region, _account) = (fields.next()?, fields.next()?);
    let resource = fields.next()?;
    Some(format!("{service}:{resource}"))
}

/// IAM's glob matching: `*` stands for any run of characters and `?` for exactly one.
///
/// Iterative with a single backtrack point, so the work is bounded by the lengths of
/// the two strings rather than by their structure; both come from a document already
/// capped at [`MAX_RESOURCE_POLICY_BYTES`].
pub(super) fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut at_pattern, mut at_value) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;
    while at_value < value.len() {
        if at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == value[at_value])
        {
            at_pattern += 1;
            at_value += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some(at_pattern);
            resume = at_value;
            at_pattern += 1;
        } else if let Some(at_star) = star {
            at_pattern = at_star + 1;
            resume += 1;
            at_value = resume;
        } else {
            return false;
        }
    }
    pattern[at_pattern..].iter().all(|glob| *glob == '*')
}

/// The actions that let a caller CHANGE a resource's policy again.
///
/// A policy that denies the caller any of them removes "your permissions to change the
/// policy of this resource in the future"
/// (`PutResourcePolicy$ConfirmRemoveSelfResourceAccess`), which is what that member has
/// to be set to `true` to accept.
pub(super) const POLICY_MANAGEMENT_ACTIONS: [&str; 3] = [
    "dynamodb:PutResourcePolicy",
    "dynamodb:DeleteResourcePolicy",
    "dynamodb:GetResourcePolicy",
];

/// Whether `policy`, attached to the table at `resource_arn`, would deny THIS CALLER the
/// ability to change that policy again.
///
/// Checked against the policy the request is attaching rather than the one the table
/// carries: the lock-out is what the NEW document would do, and it is decided before
/// anything is stored so a refused request leaves the table exactly as it was.
///
/// Only a deny that applies to EVERY request counts. A statement carrying a `Condition`
/// applies only to the requests whose context satisfies it, so such a statement does not
/// establish that the caller could never change the policy again — which is the one thing
/// `ConfirmRemoveSelfResourceAccess` is there to confirm — and a document whose deny of
/// the policy-management actions is conditioned is attached without it. That the emulator
/// does not evaluate a condition is a fact about the emulator, not about the document: it
/// is no ground for refusing to STORE one.
///
/// Every spelling a statement may name this account by is compared — the root user's ARN
/// and the bare account id that stands for it ([`::service_common::iam_authorization::account_principals`]) —
/// because a statement that denies the ACCOUNT denies each of its principals, so a
/// document that names the account by its id takes the same access away as one that names
/// its root.
pub fn locks_the_caller_out(policy: &AttachedPolicy, resource_arn: &str) -> bool {
    let caller = caller_principal();
    let mut named = vec![caller.clone()];
    named.extend(::service_common::iam_authorization::account_principals(
        &caller,
    ));
    POLICY_MANAGEMENT_ACTIONS
        .iter()
        .any(|action| policy.denies_unconditionally(&named, action, resource_arn))
}

/// The `AccessDeniedException` message a data-plane request earns when the policies in
/// force do not permit it, or `None` when they do.
///
/// `caller` is who the request speaks for ([`authorization::request_caller`], resolved
/// before this store was locked) and `action` is the IAM action the operation performs
/// (`dynamodb:GetItem`). The decision itself is [`authorization::denial`]'s, which is the
/// one place this service makes it.
pub fn policy_denial(
    caller: &authorization::Caller,
    table: &Table,
    action: &str,
    now: SystemTime,
) -> Option<String> {
    authorization::denial(caller, table, action, now)
}
