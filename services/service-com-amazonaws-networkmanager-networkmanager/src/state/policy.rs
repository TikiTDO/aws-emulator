// SPDX-License-Identifier: CC0-1.0

//! Core network policy documents: the validation of a submitted document, the topology
//! it describes and the change set its execution produces.

use super::*;

/// Check the structure of a policy document a request asks to store.
///
/// Only the document's SHAPE is judged here, which is what every operation that accepts a
/// policy document has to agree on: it must parse as JSON, its root must be an object,
/// the `version` and `core-network-configuration` the schema requires must be present and
/// of the right type, and a `segments` list, when given, must be a list of objects that
/// each name themselves, and no list may carry more entries than its cap allows, so a
/// stored document expands into a bounded topology. The deeper semantics of the document
/// (which segments may share routes, which attachments map where) belong to the operations
/// that act on a policy.
///
/// Every problem found is reported, not just the first, which is why the answer is a list:
/// `CoreNetworkPolicyException` carries one entry per offending node.
pub fn validate_policy_document(document: &str) -> Result<(), Vec<CoreNetworkPolicyError>> {
    let parsed: ::serde_json::Value = match ::serde_json::from_str(document) {
        Ok(parsed) => parsed,
        Err(problem) => {
            return Err(vec![policy_error(
                POLICY_ERROR_INVALID_JSON,
                format!("the policy document is not valid JSON: {problem}"),
                "$",
            )]);
        }
    };
    let Some(root) = parsed.as_object() else {
        return Err(vec![policy_error(
            POLICY_ERROR_INVALID_TYPE,
            "the policy document must be a JSON object".to_string(),
            "$",
        )]);
    };

    let mut errors = Vec::new();
    for (key, wanted, holds) in [
        (
            POLICY_VERSION,
            "a string",
            ::serde_json::Value::is_string as fn(&::serde_json::Value) -> bool,
        ),
        (
            POLICY_CONFIGURATION,
            "an object",
            ::serde_json::Value::is_object as fn(&::serde_json::Value) -> bool,
        ),
    ] {
        match root.get(key) {
            None => errors.push(policy_error(
                POLICY_ERROR_MISSING_FIELD,
                format!("the policy document must carry {key:?}"),
                &format!("$.{key}"),
            )),
            Some(value) if !holds(value) => errors.push(policy_error(
                POLICY_ERROR_INVALID_TYPE,
                format!("{key:?} must be {wanted}"),
                &format!("$.{key}"),
            )),
            Some(_) => {}
        }
    }

    match root.get(POLICY_SEGMENTS) {
        None => {}
        Some(::serde_json::Value::Array(segments)) => {
            // `.take` bounds how many problems one document can be reported for: a list
            // longer than its cap is refused by the cap check below, so there is nothing
            // to gain from walking the rest of it.
            for (index, segment) in segments.iter().enumerate().take(MAX_POLICY_SEGMENTS) {
                if segment
                    .get(POLICY_NAME)
                    .and_then(::serde_json::Value::as_str)
                    .is_none()
                {
                    errors.push(policy_error(
                        POLICY_ERROR_MISSING_FIELD,
                        format!("every entry of {POLICY_SEGMENTS:?} must carry a {POLICY_NAME:?} string"),
                        &format!("$.{POLICY_SEGMENTS}[{index}].{POLICY_NAME}"),
                    ));
                }
            }
        }
        Some(_) => errors.push(policy_error(
            POLICY_ERROR_INVALID_TYPE,
            format!("{POLICY_SEGMENTS:?} must be an array"),
            &format!("$.{POLICY_SEGMENTS}"),
        )),
    }

    // The caps that bound the derived topology. Each is checked on the list's own length,
    // so a document past one is refused before anything is stored or expanded.
    let edge_locations = root
        .get(POLICY_CONFIGURATION)
        .and_then(::serde_json::Value::as_object)
        .and_then(|configuration| configuration.get(POLICY_EDGE_LOCATIONS))
        .and_then(::serde_json::Value::as_array)
        .map_or(0, Vec::len);
    for (path, held, cap) in [
        (
            format!("$.{POLICY_CONFIGURATION}.{POLICY_EDGE_LOCATIONS}"),
            edge_locations,
            MAX_POLICY_EDGE_LOCATIONS,
        ),
        (
            format!("$.{POLICY_SEGMENTS}"),
            root.get(POLICY_SEGMENTS)
                .and_then(::serde_json::Value::as_array)
                .map_or(0, Vec::len),
            MAX_POLICY_SEGMENTS,
        ),
        (
            format!("$.{POLICY_NETWORK_FUNCTION_GROUPS}"),
            root.get(POLICY_NETWORK_FUNCTION_GROUPS)
                .and_then(::serde_json::Value::as_array)
                .map_or(0, Vec::len),
            MAX_POLICY_NETWORK_FUNCTION_GROUPS,
        ),
    ] {
        if held > cap {
            errors.push(policy_error(
                POLICY_ERROR_TOO_MANY_ENTRIES,
                format!("{path} carries {held} entries, but at most {cap} are allowed"),
                &path,
            ));
        }
    }

    // The per-value lengths the shapes this topology is reported through allow. Capping
    // them is what bounds the derived topology in BYTES as well as in entries.
    for (index, entry) in root
        .get(POLICY_CONFIGURATION)
        .and_then(::serde_json::Value::as_object)
        .and_then(|configuration| configuration.get(POLICY_EDGE_LOCATIONS))
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .enumerate()
        .take(MAX_POLICY_EDGE_LOCATIONS)
    {
        let path =
            format!("$.{POLICY_CONFIGURATION}.{POLICY_EDGE_LOCATIONS}[{index}].{POLICY_LOCATION}");
        match entry
            .get(POLICY_LOCATION)
            .and_then(::serde_json::Value::as_str)
        {
            None => errors.push(policy_error(
                POLICY_ERROR_MISSING_FIELD,
                format!(
                    "every entry of {POLICY_EDGE_LOCATIONS:?} must carry a {POLICY_LOCATION:?} string"
                ),
                &path,
            )),
            Some(location) => check_policy_location(&path, location, &mut errors),
        }
    }

    for (key, cap) in [
        (POLICY_SEGMENTS, MAX_POLICY_SEGMENTS),
        (
            POLICY_NETWORK_FUNCTION_GROUPS,
            MAX_POLICY_NETWORK_FUNCTION_GROUPS,
        ),
    ] {
        for (index, entry) in root
            .get(key)
            .and_then(::serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .enumerate()
            .take(cap)
        {
            let Some(name) = entry.get(POLICY_NAME).and_then(::serde_json::Value::as_str) else {
                continue;
            };
            let length = name.chars().count();
            if length > MAX_POLICY_NAME_CHARS {
                errors.push(policy_error(
                    POLICY_ERROR_INVALID_VALUE,
                    format!(
                        "a {POLICY_NAME:?} must be at most {MAX_POLICY_NAME_CHARS} characters, but {length} were given"
                    ),
                    &format!("$.{key}[{index}].{POLICY_NAME}"),
                ));
            }
            // An entry may restrict itself to some of the core network's edge locations,
            // and THAT list is what the entry is expanded over — once per edge location it
            // names, in the derived topology and in the change set of the policy version.
            // So it is capped exactly like the configuration's own edge-location list, in
            // entries and in characters: without the cap a document carrying a bounded
            // number of entries could still name an unbounded number of locations in each
            // and expand into an unbounded topology.
            let path = format!("$.{key}[{index}].{POLICY_EDGE_LOCATIONS}");
            let locations = entry
                .get(POLICY_EDGE_LOCATIONS)
                .and_then(::serde_json::Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            if locations.len() > MAX_POLICY_EDGE_LOCATIONS {
                errors.push(policy_error(
                    POLICY_ERROR_TOO_MANY_ENTRIES,
                    format!(
                        "{path} carries {} entries, but at most {MAX_POLICY_EDGE_LOCATIONS} are allowed",
                        locations.len()
                    ),
                    &path,
                ));
            }
            // Enumerated BEFORE the entries are read, so the index a refusal reports is the
            // entry's own position in the document, and an entry that is not a string is
            // reported rather than passed over — the same order the configuration's own
            // edge-location walk above uses.
            for (position, location) in locations.iter().enumerate().take(MAX_POLICY_EDGE_LOCATIONS)
            {
                let path = format!("{path}[{position}]");
                let Some(location) = location.as_str() else {
                    errors.push(policy_error(
                        POLICY_ERROR_INVALID_TYPE,
                        format!(
                            "every entry of {POLICY_EDGE_LOCATIONS:?} must be a {POLICY_LOCATION:?} string"
                        ),
                        &path,
                    ));
                    continue;
                };
                check_policy_location(&path, location, &mut errors);
            }
        }
    }

    // `attachment-policies` decides what becomes of every attachment created on this core
    // network, so a document carrying one is judged like the lists above: it has to be an
    // array, each rule has to carry the `rule-number` that orders it and the `action` that
    // says what it does, and both the rule count and each rule's condition count are capped
    // so the walk one attachment create costs stays bounded.
    match root.get(POLICY_ATTACHMENT_POLICIES) {
        None => {}
        Some(::serde_json::Value::Array(rules)) => {
            if rules.len() > MAX_POLICY_ATTACHMENT_POLICIES {
                errors.push(policy_error(
                    POLICY_ERROR_TOO_MANY_ENTRIES,
                    format!(
                        "$.{POLICY_ATTACHMENT_POLICIES} carries {} entries, but at most {MAX_POLICY_ATTACHMENT_POLICIES} are allowed",
                        rules.len()
                    ),
                    &format!("$.{POLICY_ATTACHMENT_POLICIES}"),
                ));
            }
            for (index, rule) in rules
                .iter()
                .enumerate()
                .take(MAX_POLICY_ATTACHMENT_POLICIES)
            {
                if rule
                    .get(POLICY_RULE_NUMBER)
                    .and_then(::serde_json::Value::as_i64)
                    .is_none()
                {
                    errors.push(policy_error(
                        POLICY_ERROR_MISSING_FIELD,
                        format!("every entry of {POLICY_ATTACHMENT_POLICIES:?} must carry a {POLICY_RULE_NUMBER:?} number"),
                        &format!("$.{POLICY_ATTACHMENT_POLICIES}[{index}].{POLICY_RULE_NUMBER}"),
                    ));
                }
                if rule
                    .get(POLICY_ACTION)
                    .and_then(::serde_json::Value::as_object)
                    .is_none()
                {
                    errors.push(policy_error(
                        POLICY_ERROR_MISSING_FIELD,
                        format!("every entry of {POLICY_ATTACHMENT_POLICIES:?} must carry an {POLICY_ACTION:?} object"),
                        &format!("$.{POLICY_ATTACHMENT_POLICIES}[{index}].{POLICY_ACTION}"),
                    ));
                }
                let conditions = rule
                    .get(POLICY_CONDITIONS)
                    .and_then(::serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                if conditions > MAX_POLICY_CONDITIONS_PER_RULE {
                    errors.push(policy_error(
                        POLICY_ERROR_TOO_MANY_ENTRIES,
                        format!(
                            "a rule's {POLICY_CONDITIONS:?} carries {conditions} entries, but at most {MAX_POLICY_CONDITIONS_PER_RULE} are allowed"
                        ),
                        &format!("$.{POLICY_ATTACHMENT_POLICIES}[{index}].{POLICY_CONDITIONS}"),
                    ));
                }
            }
        }
        Some(_) => errors.push(policy_error(
            POLICY_ERROR_INVALID_TYPE,
            format!("{POLICY_ATTACHMENT_POLICIES:?} must be an array"),
            &format!("$.{POLICY_ATTACHMENT_POLICIES}"),
        )),
    }

    // `segment-actions` is the part of the document that says what each segment's routes
    // reach, which the reported topology is derived from, so a document carrying one is
    // judged like the lists above: it has to be an array, and both its entry count and the
    // name lists inside one entry are capped so that derivation stays bounded — a name list
    // longer than the segment or group list it may name can only be repeating names.
    match root.get(POLICY_SEGMENT_ACTIONS) {
        None => {}
        Some(::serde_json::Value::Array(actions)) => {
            if actions.len() > MAX_POLICY_SEGMENT_ACTIONS {
                errors.push(policy_error(
                    POLICY_ERROR_TOO_MANY_ENTRIES,
                    format!(
                        "$.{POLICY_SEGMENT_ACTIONS} carries {} entries, but at most {MAX_POLICY_SEGMENT_ACTIONS} are allowed",
                        actions.len()
                    ),
                    &format!("$.{POLICY_SEGMENT_ACTIONS}"),
                ));
            }
            for (index, entry) in actions.iter().enumerate().take(MAX_POLICY_SEGMENT_ACTIONS) {
                let share_with = entry.get(POLICY_SHARE_WITH);
                for (listed, path, cap) in [
                    (
                        share_with.and_then(::serde_json::Value::as_array),
                        format!("$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_SHARE_WITH}"),
                        MAX_POLICY_SEGMENTS,
                    ),
                    (
                        share_with
                            .and_then(::serde_json::Value::as_object)
                            .and_then(|with| with.get(POLICY_EXCEPT))
                            .and_then(::serde_json::Value::as_array),
                        format!(
                            "$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_SHARE_WITH}.{POLICY_EXCEPT}"
                        ),
                        MAX_POLICY_SEGMENTS,
                    ),
                    (
                        entry
                            .get(POLICY_VIA)
                            .and_then(::serde_json::Value::as_object)
                            .and_then(|via| via.get(POLICY_NETWORK_FUNCTION_GROUPS))
                            .and_then(::serde_json::Value::as_array),
                        format!(
                            "$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_VIA}.{POLICY_NETWORK_FUNCTION_GROUPS}"
                        ),
                        MAX_POLICY_NETWORK_FUNCTION_GROUPS,
                    ),
                    (
                        entry
                            .get(POLICY_DESTINATION_CIDR_BLOCKS)
                            .and_then(::serde_json::Value::as_array),
                        format!(
                            "$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_DESTINATION_CIDR_BLOCKS}"
                        ),
                        MAX_POLICY_ROUTE_PREFIXES,
                    ),
                    (
                        entry
                            .get(POLICY_DESTINATIONS)
                            .and_then(::serde_json::Value::as_array),
                        format!("$.{POLICY_SEGMENT_ACTIONS}[{index}].{POLICY_DESTINATIONS}"),
                        MAX_POLICY_ROUTE_DESTINATIONS,
                    ),
                ] {
                    let held = listed.map_or(0, Vec::len);
                    if held > cap {
                        errors.push(policy_error(
                            POLICY_ERROR_TOO_MANY_ENTRIES,
                            format!("{path} carries {held} entries, but at most {cap} are allowed"),
                            &path,
                        ));
                    }
                }
            }
        }
        Some(_) => errors.push(policy_error(
            POLICY_ERROR_INVALID_TYPE,
            format!("{POLICY_SEGMENT_ACTIONS:?} must be an array"),
            &format!("$.{POLICY_SEGMENT_ACTIONS}"),
        )),
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The modelled `CoreNetworkPolicyException` (HTTP 400) reporting the problems
/// [`validate_policy_document`] found, so every operation that takes a policy document
/// refuses a bad one the same way.
pub fn policy_rejection(errors: Vec<CoreNetworkPolicyError>) -> CoreNetworkPolicyException {
    CoreNetworkPolicyException {
        message: format!(
            "the core network policy document is not valid: {} problem(s) found",
            errors.len()
        ),
        errors: Some(errors),
    }
}

/// What a `CoreNetwork` reports about the policy in force on it, kept in the COMPACT form
/// the document describes: a segment or group that restricts itself to no edge location is
/// held as "every edge location" rather than as a copy of the whole location list, so the
/// stored topology is linear in the document it came from and only a response expands it.
#[derive(Debug, Clone, Default)]
pub struct PolicyTopology {
    /// One entry per `core-network-configuration.edge-locations[]` of the document.
    pub edges: Vec<CoreNetworkEdge>,
    /// One entry per named `segments[]` of the document.
    pub segments: Vec<NamedGroup>,
    /// One entry per named `network-function-groups[]` of the document.
    pub network_function_groups: Vec<NamedGroup>,
    /// One entry per `attachment-policies[]` rule of the document, in the order the rules
    /// are evaluated: ascending `rule-number`.
    pub attachment_policies: Vec<AttachmentPolicyRule>,
    /// One entry per `segment-actions[]` entry of action `share` the document carries, in
    /// the order it lists them: which segment shares its routes, and which segments they
    /// are shared with.
    pub shares: Vec<SegmentShare>,
    /// One entry per `segment-actions[]` entry of action `send-via` / `send-to` the
    /// document carries, in the order it lists them: which segment's traffic is served by
    /// which network function groups.
    pub service_insertions: Vec<ServiceInsertion>,
    /// One entry per `segment-actions[]` entry of action `create-route` the document
    /// carries, in the order it lists them: the static routes it declares for the segment
    /// it acts on.
    pub static_routes: Vec<SegmentStaticRoute>,
}

/// The topology of NO policy: what a core network with no policy version in force reports,
/// and what one being deleted reports ([`CoreNetworkRecord::to_shape_with_state`]).
///
/// A constant rather than a value built per read, so answering either costs nothing and
/// [`CoreNetworkRecord::topology`] can hand a reference back.
pub static NO_POLICY_IN_FORCE: PolicyTopology = PolicyTopology {
    edges: Vec::new(),
    segments: Vec::new(),
    network_function_groups: Vec::new(),
    attachment_policies: Vec::new(),
    shares: Vec::new(),
    service_insertions: Vec::new(),
    static_routes: Vec::new(),
};

/// One `segment-actions[]` entry of action `create-route`: the STATIC routes a policy
/// document declares for one segment.
///
/// A static route is not a function of anything attached: the document states the prefix
/// and what it is reached through, so the routes a segment holds follow from the policy in
/// force on the core network and from nothing else. Kept as the document spells them — a
/// destination is resolved against the stored attachments where the routing information is
/// reported, so a destination naming an attachment that has since gone reaches nothing
/// rather than being reported as a route to it.
#[derive(Debug, Clone)]
pub struct SegmentStaticRoute {
    /// The entry's `segment`: the segment whose route table holds these routes. Always a
    /// segment the document declares, since an entry naming any other segment is not kept.
    pub segment: String,
    /// The entry's `destination-cidr-blocks`: the prefix each route it declares carries, in
    /// the order the document lists them, bounded by [`MAX_POLICY_ROUTE_PREFIXES`].
    pub destination_cidr_blocks: Vec<String>,
    /// The entry's `destinations`: what those prefixes are reached through — the id of an
    /// attachment of the segment, or [`POLICY_DESTINATION_BLACKHOLE`] for a route that
    /// reaches nothing — bounded by [`MAX_POLICY_ROUTE_DESTINATIONS`].
    pub destinations: Vec<String>,
}

/// One `segment-actions[]` entry of action `share`: the segment whose routes are shared,
/// and how the entry names the segments they are shared with.
#[derive(Debug, Clone)]
pub struct SegmentShare {
    /// The entry's `segment`: the segment that shares its routes. Always a segment the
    /// document declares, since an entry naming any other segment is not kept.
    pub segment: String,
    /// The entry's `share-with`.
    pub with: ShareWith,
}

/// How a `share` entry names the segments the sharing segment's routes are shared with:
/// either the names it lists, or every other declared segment save the ones it excepts.
///
/// Both forms hold their names as a SET of declared segment names, so what one entry
/// retains is bounded by [`MAX_POLICY_SEGMENTS`] however long — and however repetitive —
/// the list the document spelled was.
#[derive(Debug, Clone)]
pub enum ShareWith {
    /// The array form: the segments the entry lists, each one the document declares.
    Named(BTreeSet<String>),
    /// The `*` form (nothing excepted) and the `{"except": [...]}` form: every other
    /// segment the document declares, less the ones listed here.
    AllExcept(BTreeSet<String>),
}

/// One `segment-actions[]` entry of action `send-via` / `send-to`: the segment the entry
/// acts on, and the network function groups it sends that segment's traffic through.
#[derive(Debug, Clone)]
pub struct ServiceInsertion {
    /// Whether the entry's action is `send-via`; `send-to` otherwise. The two are reported
    /// through separate members of `ServiceInsertionSegments`, so which one it is is kept.
    pub send_via: bool,
    /// The entry's `segment`, always one the document declares.
    pub segment: String,
    /// The entry's `via.network-function-groups`, each one the document declares. Held as
    /// a SET, so it is bounded by [`MAX_POLICY_NETWORK_FUNCTION_GROUPS`] however long the
    /// list the document spelled was, and a membership test over it is logarithmic.
    pub network_function_groups: BTreeSet<String>,
}

/// One named `segments[]` / `network-function-groups[]` entry of a policy document.
#[derive(Debug, Clone)]
pub struct NamedGroup {
    /// The entry's `name`.
    pub name: String,
    /// The edge locations the entry restricts itself to, or `None` for "every edge
    /// location this core network has" — the case a stored copy would duplicate.
    pub edge_locations: Option<Vec<String>>,
}

/// One `attachment-policies[]` rule of a core network policy document: which attachments
/// the rule claims, and what it then does with one.
///
/// Compared for equality where two policy documents are diffed ([`policy_changes`]): the
/// `attachment-policies` block of one document differs from the other's exactly when their
/// rule lists do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentPolicyRule {
    /// The rule's `rule-number`, which orders it against the others and is what an
    /// attachment reports as `Attachment$AttachmentPolicyRuleNumber` (an `Integer`,
    /// `smithy_query shape=Attachment`).
    pub rule_number: i32,
    /// Whether EVERY condition has to hold for the rule to claim an attachment — the
    /// `condition-logic` default `and` — or any one of them ([`POLICY_LOGIC_OR`]).
    pub all_conditions: bool,
    /// The rule's `conditions[]`, each capped at [`MAX_POLICY_CONDITIONS_PER_RULE`].
    pub conditions: Vec<AttachmentCondition>,
    /// The rule's `action.require-attachment-acceptance`: whether an attachment this rule
    /// claims has to be accepted before it is created.
    pub require_acceptance: bool,
    /// How the rule's action names the segment it puts the attachment in, or `None` where
    /// it names none.
    pub segment: Option<SegmentAssociation>,
    /// The rule's `action.add-to-network-function-group`, where it adds the attachment to a
    /// network function group instead of a segment.
    pub network_function_group: Option<String>,
}

/// How an attachment-policy rule's action names the segment the attachment joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentAssociation {
    /// `association-method: constant` — the one name the action's `segment` carries.
    Constant(String),
    /// `association-method: tag` — the value of the attachment's tag whose key the action's
    /// `tag-value-of-key` names, so an attachment carrying no such tag joins no segment.
    TagValueOfKey(String),
}

/// One `conditions[]` entry of an attachment-policy rule, kept as the document spells it:
/// the property compared ([`CONDITION_ANY`] and friends), the `operator`
/// ([`OPERATOR_EQUALS`] and friends) and the `key` / `value` the comparison uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentCondition {
    pub condition_type: String,
    pub operator: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
}

/// The attachment an `attachment-policies` rule is matched against: the properties of it
/// the published condition types compare.
#[derive(Debug, Clone, Copy)]
pub struct AttachmentCandidate<'a> {
    /// One of `ops::types::ATTACHMENT_TYPE_VALUES`, what `attachment-type` compares.
    pub attachment_type: &'a str,
    /// The Region of the attachment's edge location, what `region` compares.
    pub edge_location: &'a str,
    /// The ARN of the resource the attachment attaches, what `resource-id` compares.
    pub resource_arn: &'a str,
    /// The account that owns the attachment, what `account-id` compares.
    pub account_id: &'a str,
    /// The attachment's tags, what `tag-exists` and `tag-value` look in.
    pub tags: &'a [Tag],
}

/// What the policy in force decided about one attachment: the rule that claimed it, where
/// that rule put it, and whether it has to be accepted first. The default — every member
/// absent, no acceptance required — is the answer for an attachment no rule claims, which
/// includes every attachment on a core network whose policy carries no
/// `attachment-policies` at all.
#[derive(Debug, Clone, Default)]
pub struct AttachmentClaim {
    /// The `rule-number` of the claiming rule.
    pub rule_number: Option<i32>,
    /// The segment that rule put the attachment in.
    pub segment_name: Option<String>,
    /// The network function group that rule added the attachment to.
    pub network_function_group_name: Option<String>,
    /// Whether that rule asks for the attachment request to be accepted, which is what
    /// holds the attachment in [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`].
    pub require_acceptance: bool,
}

/// The value of the tag `key` names, or `None` when the list carries no such tag.
pub(super) fn tag_value_of<'a>(tags: &'a [Tag], key: &str) -> Option<&'a str> {
    tags.iter()
        .find(|tag| tag.key.as_deref() == Some(key))
        .map(|tag| tag.value.as_deref().unwrap_or_default())
}

impl AttachmentCondition {
    /// Whether this condition holds for `candidate`.
    ///
    /// A condition type this service does not know, or one missing the `key` or `value` its
    /// comparison needs, holds for NOTHING: an unreadable condition must not silently widen
    /// the rule it belongs to into one that claims every attachment.
    pub(super) fn holds(&self, candidate: &AttachmentCandidate<'_>) -> bool {
        match self.condition_type.as_str() {
            CONDITION_ANY => true,
            CONDITION_ACCOUNT_ID => self.compares(candidate.account_id),
            CONDITION_REGION => self.compares(candidate.edge_location),
            CONDITION_ATTACHMENT_TYPE => self.compares(candidate.attachment_type),
            CONDITION_RESOURCE_ID => self.compares(candidate.resource_arn),
            CONDITION_TAG_EXISTS => self
                .key
                .as_deref()
                .is_some_and(|key| tag_value_of(candidate.tags, key).is_some()),
            CONDITION_TAG_VALUE => self
                .key
                .as_deref()
                .and_then(|key| tag_value_of(candidate.tags, key))
                .is_some_and(|held| self.compares(held)),
            _ => false,
        }
    }

    /// Whether `held`, the attachment's own value, satisfies this condition's `operator`
    /// against its `value`.
    pub(super) fn compares(&self, held: &str) -> bool {
        let Some(wanted) = self.value.as_deref() else {
            return false;
        };
        match self.operator.as_deref() {
            Some(OPERATOR_EQUALS) => held == wanted,
            Some(OPERATOR_NOT_EQUALS) => held != wanted,
            Some(OPERATOR_CONTAINS) => held.contains(wanted),
            Some(OPERATOR_BEGINS_WITH) => held.starts_with(wanted),
            Some(_) | None => false,
        }
    }
}

impl AttachmentPolicyRule {
    /// Whether this rule claims `candidate`: every condition holds, or — under
    /// [`POLICY_LOGIC_OR`] — any one does. A rule carrying no condition claims nothing.
    pub(super) fn claims(&self, candidate: &AttachmentCandidate<'_>) -> bool {
        if self.conditions.is_empty() {
            return false;
        }
        if self.all_conditions {
            self.conditions
                .iter()
                .all(|condition| condition.holds(candidate))
        } else {
            self.conditions
                .iter()
                .any(|condition| condition.holds(candidate))
        }
    }

    /// The segment this rule puts `candidate` in: the constant it names, or the value of the
    /// tag it takes the name from — which is absent when the attachment carries no such tag.
    pub(super) fn segment_for(&self, candidate: &AttachmentCandidate<'_>) -> Option<String> {
        match self.segment.as_ref()? {
            SegmentAssociation::Constant(name) => Some(name.clone()),
            SegmentAssociation::TagValueOfKey(key) => {
                tag_value_of(candidate.tags, key).map(str::to_string)
            }
        }
    }
}

impl PolicyTopology {
    /// The edge locations this core network has, in the order the document configures
    /// them: what an entry restricting itself to none is present in.
    pub(super) fn everywhere(&self) -> Vec<String> {
        self.edges
            .iter()
            .filter_map(|edge| edge.edge_location.clone())
            .collect()
    }

    /// The (edge location, name) pairs the entries `groups` are placed at: an entry
    /// restricting itself to no edge location is placed at every edge location this core
    /// network has, which is the expansion [`Self::to_segments`] makes too.
    pub(super) fn placements(&self, groups: &[NamedGroup]) -> BTreeSet<(String, String)> {
        let everywhere = self.everywhere();
        let mut placed = BTreeSet::new();
        for group in groups {
            let locations = group.edge_locations.as_deref().unwrap_or(&everywhere);
            for location in locations {
                placed.insert((location.clone(), group.name.clone()));
            }
        }
        placed
    }

    /// Whether this core network configures an edge in `location`.
    ///
    /// An attachment joins a resource to one of the core network's EDGES, so a create
    /// naming a location the policy in force configures none for is refused: the edges are
    /// a function of the stored policy document, so this answer comes from the same
    /// derived topology a read of the core network reports.
    pub fn has_edge_location(&self, location: &str) -> bool {
        self.edges
            .iter()
            .any(|edge| edge.edge_location.as_deref() == Some(location))
    }

    /// Whether this core network declares a segment named `name`.
    ///
    /// The sibling of [`Self::has_edge_location`] for the other half of the topology: a
    /// segment of a core network is one its policy document's `segments` declares, so a
    /// request naming any other segment addresses nothing ([`no_such_segment`]).
    pub fn has_segment(&self, name: &str) -> bool {
        self.segments.iter().any(|group| group.name == name)
    }

    /// Whether the segment `name` is present at the edge location `location`: it is
    /// declared, and either restricts itself to locations holding that one or restricts
    /// itself to none at all — which places it at every edge this core network has, the same
    /// expansion [`Self::to_segments`] reports, so ONE statement of that rule serves both.
    ///
    /// A segment that is not present at an edge has no route table there, so it holds no
    /// routing information at that edge however much the core network holds elsewhere.
    pub fn segment_present_at(&self, name: &str, location: &str) -> bool {
        self.segments
            .iter()
            .filter(|group| group.name == name)
            .any(|group| match group.edge_locations.as_deref() {
                Some(listed) => listed.iter().any(|held| held == location),
                None => self.everywhere().iter().any(|held| held == location),
            })
    }

    /// The static routes this policy declares for the segment `name`, in the order the
    /// document lists the `create-route` entries that declare them.
    pub fn static_routes_of<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a SegmentStaticRoute> {
        self.static_routes
            .iter()
            .filter(move |route| route.segment == name)
    }

    /// The ASN this policy assigns the edge it configures in `location`, or `None` where it
    /// configures no edge there or assigns that edge none.
    ///
    /// The ASN of an edge is the core network's own end of every BGP session that edge
    /// peers over, which is what a Connect peer reports as
    /// `ConnectPeerBgpConfiguration$CoreNetworkAsn`: it comes from the same derived topology
    /// a read of the core network reports, so the two cannot disagree.
    pub fn edge_asn(&self, location: &str) -> Option<i64> {
        self.edges
            .iter()
            .find(|edge| edge.edge_location.as_deref() == Some(location))
            .and_then(|edge| edge.asn)
    }

    /// What this policy decides about the attachment `candidate` describes.
    ///
    /// The rules of `attachment-policies` are evaluated in ascending `rule-number` and the
    /// FIRST one whose conditions claim the attachment decides it (Cloud WAN "Attachment
    /// policies" documentation), so a later rule never overrides an earlier one. Only the
    /// claiming rule's action asks for acceptance: an attachment no rule claims \u2014 every
    /// attachment, where the document carries no `attachment-policies` \u2014 needs none, since
    /// the action is the only place a document asks for one.
    ///
    /// Called ONCE, where the attachment is created, and the answer is stored on the
    /// attachment record: an attachment keeps the decision the policy in force made about
    /// it, so a later read never re-derives it from a policy that has since changed.
    pub fn attachment_claim(&self, candidate: &AttachmentCandidate<'_>) -> AttachmentClaim {
        let Some(rule) = self
            .attachment_policies
            .iter()
            .find(|rule| rule.claims(candidate))
        else {
            return AttachmentClaim::default();
        };
        AttachmentClaim {
            rule_number: Some(rule.rule_number),
            segment_name: rule.segment_for(candidate),
            network_function_group_name: rule.network_function_group.clone(),
            require_acceptance: rule.require_acceptance,
        }
    }

    /// The modelled `Segments` of this topology.
    ///
    /// A segment's `SharedSegments` are the segments its routes are shared with, which the
    /// document's `segment-actions` entries of action `share` declare: a segment no such
    /// entry acts on shares with nothing and reports the empty list. The answer is in the
    /// order the document declares the segments, and never names the segment itself.
    pub fn to_segments(&self) -> Vec<CoreNetworkSegment> {
        let everywhere = self.everywhere();
        let declared = self.declared_segments();
        self.segments
            .iter()
            .map(|group| CoreNetworkSegment {
                name: Some(group.name.clone()),
                edge_locations: Some(
                    group
                        .edge_locations
                        .clone()
                        .unwrap_or_else(|| everywhere.clone()),
                ),
                shared_segments: Some(self.shared_segments_of(&group.name, &declared)),
            })
            .collect()
    }

    /// The modelled `NetworkFunctionGroups` of this topology.
    ///
    /// A group's `Segments` are the segments whose traffic the document's `segment-actions`
    /// send through this group, split by the action that sent them: `SendVia` for the
    /// `send-via` entries and `SendTo` for the `send-to` ones. A group no such entry names
    /// serves no segment and reports two empty lists. Each list is in the order the
    /// document declares the segments.
    pub fn to_network_function_groups(&self) -> Vec<CoreNetworkNetworkFunctionGroup> {
        let everywhere = self.everywhere();
        let declared = self.declared_segments();
        self.network_function_groups
            .iter()
            .map(|group| CoreNetworkNetworkFunctionGroup {
                name: Some(group.name.clone()),
                edge_locations: Some(
                    group
                        .edge_locations
                        .clone()
                        .unwrap_or_else(|| everywhere.clone()),
                ),
                segments: Some(ServiceInsertionSegments {
                    send_via: Some(self.inserted_segments_of(&group.name, true, &declared)),
                    send_to: Some(self.inserted_segments_of(&group.name, false, &declared)),
                }),
            })
            .collect()
    }

    /// The names of the segments this document declares, in the order it declares them and
    /// each named once: the order every segment list of a reported topology is in, and the
    /// only names such a list may carry — a `segment-actions` entry naming anything else
    /// names no segment of this core network.
    pub(super) fn declared_segments(&self) -> Vec<&str> {
        let mut declared: Vec<&str> = Vec::with_capacity(self.segments.len());
        for group in &self.segments {
            if !declared.contains(&group.name.as_str()) {
                declared.push(group.name.as_str());
            }
        }
        declared
    }

    /// The segments the routes of the segment `name` are shared with: the union of what
    /// every `share` entry acting on it declares, in `declared` order.
    ///
    /// Bounded by the caps: at most [`MAX_POLICY_SEGMENT_ACTIONS`] entries are walked, each
    /// holding at most [`MAX_POLICY_SEGMENTS`] names, and the answer is at most `declared`
    /// long.
    pub(super) fn shared_segments_of(&self, name: &str, declared: &[&str]) -> Vec<String> {
        let mut shared: BTreeSet<&str> = BTreeSet::new();
        for share in self.shares.iter().filter(|share| share.segment == name) {
            match &share.with {
                ShareWith::Named(names) => shared.extend(names.iter().map(String::as_str)),
                ShareWith::AllExcept(excepted) => shared.extend(
                    declared
                        .iter()
                        .copied()
                        .filter(|candidate| !excepted.contains(*candidate)),
                ),
            }
        }
        declared
            .iter()
            .copied()
            // A segment never shares with itself, however the entry names it: its own
            // routes are already its own.
            .filter(|candidate| *candidate != name && shared.contains(candidate))
            .map(str::to_string)
            .collect()
    }

    /// The segments whose traffic the network function group `name` serves under the
    /// `send-via` action (`send_via`) or the `send-to` one, in `declared` order.
    ///
    /// Bounded by the caps the same way [`Self::shared_segments_of`] is: at most
    /// [`MAX_POLICY_SEGMENT_ACTIONS`] entries are walked, each answering whether it names
    /// this group in logarithmic time.
    pub(super) fn inserted_segments_of(&self, name: &str, send_via: bool, declared: &[&str]) -> Vec<String> {
        let served: BTreeSet<&str> = self
            .service_insertions
            .iter()
            .filter(|insertion| {
                insertion.send_via == send_via && insertion.network_function_groups.contains(name)
            })
            .map(|insertion| insertion.segment.as_str())
            .collect();
        declared
            .iter()
            .copied()
            .filter(|candidate| served.contains(candidate))
            .map(str::to_string)
            .collect()
    }

    /// The distinct service insertion actions the `send-via` / `send-to` entries acting on
    /// the segment `name` declare.
    ///
    /// Bounded by [`MAX_POLICY_SEGMENT_ACTIONS`] entries, each naming at most
    /// [`MAX_POLICY_NETWORK_FUNCTION_GROUPS`] groups.
    pub(super) fn insertions_of(&self, name: &str) -> BTreeSet<SegmentInsertion> {
        self.service_insertions
            .iter()
            .filter(|insertion| insertion.segment == name)
            .map(|insertion| SegmentInsertion {
                send_via: insertion.send_via,
                network_function_groups: insertion
                    .network_function_groups
                    .iter()
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    /// What this policy states about each of its SEGMENTS at each edge location that segment
    /// is present in: the objects a change set compares to decide what a version does to one
    /// segment ([`policy_changes`]).
    ///
    /// The key is the placement [`Self::placements`] derives and the value is everything else
    /// the document states about that segment — what the `segment-actions` acting on it
    /// declare — so two documents placing one segment at the same edge location but sharing
    /// its routes with other segments, or serving it through other network function groups,
    /// configure it differently.
    ///
    /// What those actions declare is a property of the SEGMENT and not of one of its
    /// placements, so it is derived once per declared segment and SHARED by that segment's
    /// placements: a segment present in every edge location of the core network holds one
    /// copy rather than one per location, which keeps this linear in the document it came
    /// from however many edge locations [`MAX_POLICY_EDGE_LOCATIONS`] allows.
    pub(super) fn segment_placements(&self) -> BTreeMap<(String, String), Arc<SegmentConfiguration>> {
        let declared = self.declared_segments();
        let configurations: BTreeMap<&str, Arc<SegmentConfiguration>> = declared
            .iter()
            .map(|name| {
                (
                    *name,
                    Arc::new(SegmentConfiguration {
                        shared_segments: self.shared_segments_of(name, &declared),
                        service_insertions: self.insertions_of(name),
                    }),
                )
            })
            .collect();
        self.placements(&self.segments)
            .into_iter()
            .map(|(location, name)| {
                let configuration = configurations
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or_default();
                ((location, name), configuration)
            })
            .collect()
    }

    /// Every STATIC ROUTE this policy declares, as (segment, route).
    ///
    /// A `create-route` entry declares one route per prefix and destination it names — the
    /// routes [`Self::static_routes_of`] hands the listing of a segment's routing
    /// information. The entry declares them for the SEGMENT rather than for one edge
    /// location: the segment holds them in its route table wherever it is present, so one
    /// declaration is one route here and is not multiplied over the segment's edge
    /// locations. Held as a SET, so an entry declaring one prefix twice, or two entries
    /// declaring the same route, keep it once.
    ///
    /// Bounded by the caps, and so linear in the document: at most
    /// [`MAX_POLICY_SEGMENT_ACTIONS`] entries, each declaring at most
    /// [`MAX_POLICY_ROUTE_PREFIXES`] prefixes through at most
    /// [`MAX_POLICY_ROUTE_DESTINATIONS`] destinations.
    pub(super) fn static_route_placements(&self) -> BTreeSet<(String, StaticRouteIdentity)> {
        let mut declared = BTreeSet::new();
        for route in &self.static_routes {
            for prefix in &route.destination_cidr_blocks {
                for destination in &route.destinations {
                    declared.insert((
                        route.segment.clone(),
                        StaticRouteIdentity {
                            prefix: prefix.clone(),
                            destination: destination.clone(),
                        },
                    ));
                }
            }
        }
        declared
    }
}

/// One change of a policy version's change set: what the difference between two policy
/// documents does to ONE object either of them configures.
///
/// Kept in this compact form rather than as a modelled shape because the SAME change is
/// reported two ways — as a `CoreNetworkChange` of the change set and as a
/// `CoreNetworkChangeEvent` of its execution — and because a change event's status and
/// timestamp are functions of the clock, not of the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyChange {
    /// What kind of object changed: one of the [`CHANGE_TYPE_CORE_NETWORK_EDGE`] family.
    pub change_type: &'static str,
    /// What the change does to it: one of the [`CHANGE_ACTION_ADD`] family.
    pub action: &'static str,
    /// The edge location the changed object sits at, for a kind that sits at one.
    pub edge_location: Option<String>,
    /// The changed segment's or network function group's name, for a kind that has one —
    /// and, for a change to one static route, the segment whose route tables hold it.
    pub name: Option<String>,
    /// Which static route this change acts on, for a change to one and `None` for every
    /// other change: a segment's route table holds one route per prefix and destination, so
    /// the pair is what identifies the route inside the segment [`Self::name`] gives.
    pub route: Option<StaticRouteIdentity>,
    /// What the policy IN FORCE states about the object this change acts on beyond the
    /// identity above, for a change that modifies or removes an object of a kind that
    /// carries any such values — `None` for every other change: these are the only values a
    /// reported change carries that its own identity does not already name, so the only ones
    /// the diff has to keep.
    pub previous: Option<ChangedObject>,
    /// What the SUBMITTED policy states about that object, for a change that adds or
    /// modifies it. See [`Self::previous`].
    pub new: Option<ChangedObject>,
}

/// What one of the two compared policies states about the object a change acts on, beyond
/// the identity that change already names.
///
/// A kind whose identity is everything a document states about it carries none of these — a
/// network function group at an edge location, one static route, the `attachment-policies`
/// block — so a change to one holds `None` on both sides and reports only that identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedObject {
    /// One edge of `core-network-configuration`: see [`EdgeConfiguration`].
    Edge(EdgeConfiguration),
    /// One segment of `segments` at one edge location: see [`SegmentConfiguration`]. Shared
    /// between that segment's placements, which a document states the same thing about.
    Segment(Arc<SegmentConfiguration>),
}

/// Which ONE static route of a segment's route table a change acts on: the prefix it
/// carries, and what that prefix is reached through.
///
/// A `create-route` entry declares one route per prefix and destination it names — the
/// routes [`PolicyTopology::static_routes_of`] hands the listing of a segment's routing
/// information — so the pair identifies one route, and is what a change to it reports as
/// `Cidr` and `DestinationIdentifier`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StaticRouteIdentity {
    /// The route's prefix: one `destination-cidr-blocks` entry of the `create-route` entry
    /// that declared it.
    pub prefix: String,
    /// What that prefix is reached through: one `destinations` entry of the same entry — an
    /// attachment of the segment, or [`POLICY_DESTINATION_BLACKHOLE`].
    pub destination: String,
}

/// What a policy document states about one segment beyond its name and the edge locations
/// it is present in: what the `segment-actions` acting on that segment declare.
///
/// These are the values a read of the core network reports for the segment and for the
/// groups that serve it (`CoreNetworkSegment$SharedSegments` through
/// [`PolicyTopology::to_segments`], and `ServiceInsertionSegments` through
/// [`PolicyTopology::to_network_function_groups`]), so two documents giving one segment
/// different ones configure it differently and a version that moves them modifies it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentConfiguration {
    /// The segments this segment's routes are shared with, in the order the document
    /// declares the segments ([`PolicyTopology::shared_segments_of`]).
    pub shared_segments: Vec<String>,
    /// The distinct service insertion actions acting on this segment
    /// ([`PolicyTopology::insertions_of`]), held as a SET: what a document sends a segment
    /// through does not depend on the order it lists the entries in, nor on one entry being
    /// listed twice, so neither makes two documents configure the segment differently.
    pub service_insertions: BTreeSet<SegmentInsertion>,
}

/// ONE service insertion action acting on a segment: which of the two actions it is, and the
/// network function groups it sends that segment's traffic through.
///
/// Ordered by the action and then by the groups it names, which is the order a change reports
/// them in.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentInsertion {
    /// Whether the action is `send-via`; `send-to` otherwise.
    pub send_via: bool,
    /// The declared network function groups the action names, in name order.
    pub network_function_groups: Vec<String>,
}

impl SegmentInsertion {
    /// The modelled `ServiceInsertionAction` for this action: the action it takes, and the
    /// groups it names through `Via$NetworkFunctionGroups`.
    ///
    /// A `mode` and a `when-sent-to` are parts of the entry the derived topology does not
    /// keep — no operation of this slice acts on either — so neither is reported rather than
    /// being reported as a value the document did not state.
    pub(super) fn to_shape(&self) -> ServiceInsertionAction {
        ServiceInsertionAction {
            action: Some(
                if self.send_via {
                    POLICY_ACTION_SEND_VIA
                } else {
                    POLICY_ACTION_SEND_TO
                }
                .to_string(),
            ),
            mode: None,
            when_sent_to: None,
            via: Some(Via {
                network_function_groups: Some(
                    self.network_function_groups
                        .iter()
                        .map(|name| NetworkFunctionGroup {
                            name: Some(name.clone()),
                        })
                        .collect(),
                ),
                with_edge_overrides: None,
            }),
        }
    }
}

/// How one policy document configures one edge, beyond the edge location that identifies
/// it: the ASN that edge peers with and the inside CIDR blocks it carries.
///
/// Compared for equality to decide whether two documents configure the same edge
/// differently, and reported as the values a change to that edge replaces or puts in place.
/// An edge the document gives no inside CIDR blocks carries none, which is why the list is
/// held flat rather than as an absent option: "no blocks" and "an empty list of blocks"
/// configure the edge the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeConfiguration {
    /// The edge's ASN, or `None` where the document assigns it none.
    pub asn: Option<i64>,
    /// The edge's inside CIDR blocks, empty where the document gives it none.
    pub inside_cidr_blocks: Vec<String>,
}

impl EdgeConfiguration {
    /// How `edge` is configured.
    pub(super) fn of(edge: &CoreNetworkEdge) -> Self {
        Self {
            asn: edge.asn,
            inside_cidr_blocks: edge.inside_cidr_blocks.clone().unwrap_or_default(),
        }
    }

    /// The modelled change values for an edge configured this way at `edge_location`.
    pub(super) fn to_change_values(&self, edge_location: &str) -> CoreNetworkChangeValues {
        CoreNetworkChangeValues {
            edge_locations: Some(vec![edge_location.to_string()]),
            asn: self.asn,
            inside_cidr_blocks: (!self.inside_cidr_blocks.is_empty())
                .then(|| self.inside_cidr_blocks.clone()),
            ..CoreNetworkChangeValues::default()
        }
    }
}

impl PolicyChange {
    /// This change's `IdentifierPath`: the type, then the edge location and the name where
    /// the kind carries them — "the `IdentifierPath` for a core network segment change
    /// might be `CORE_NETWORK_SEGMENT/us-east-1/devsegment`" (`smithy_query shape=
    /// CoreNetworkChangeEvent member=IdentifierPath`) — and then, for a change to one static
    /// route, the prefix that route carries and what it is reached through, which is what
    /// tells two routes of one segment apart.
    ///
    /// It "uniquely identifies the path for a change within the changeset" (same doc), so
    /// it is also the key one page of a listing resumes after.
    pub fn identifier_path(&self) -> String {
        let mut path = self.change_type.to_string();
        let route = self.route.as_ref();
        for component in [
            self.edge_location.as_deref(),
            self.name.as_deref(),
            route.map(|route| route.prefix.as_str()),
            route.map(|route| route.destination.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            path.push('/');
            path.push_str(component);
        }
        path
    }

    /// The resource this change acts on: the prefix for a change to one static route, which
    /// is what names a route in its segment's route table, the changed segment's or network
    /// function group's own name, the edge location for an edge, and nothing for a change to
    /// an object none of a policy's named entries identifies (the `attachment-policies`
    /// block as a whole).
    pub(super) fn identifier(&self) -> Option<String> {
        self.route
            .as_ref()
            .map(|route| route.prefix.clone())
            .or_else(|| self.name.clone())
            .or_else(|| self.edge_location.clone())
    }

    /// The values the object this change acts on holds under one of the two policies: its
    /// identity, plus what `configured` says that policy states about it. `None` for a
    /// change to an object no member of the shape names, as [`Self::to_change_event`]
    /// reports no values for one.
    pub(super) fn to_change_values(
        &self,
        configured: Option<&ChangedObject>,
    ) -> Option<CoreNetworkChangeValues> {
        match (self.change_type, self.edge_location.as_deref(), self.name.as_deref()) {
            (CHANGE_TYPE_CORE_NETWORK_EDGE, Some(location), _) => {
                // An edge change always carries the configuration of the side it reports —
                // the policy that configured the edge is the side that has one — so a side
                // with none reports no values rather than an edge configured with nothing.
                match configured {
                    Some(ChangedObject::Edge(edge)) => Some(edge.to_change_values(location)),
                    _ => None,
                }
            }
            (CHANGE_TYPE_CORE_NETWORK_SEGMENT, Some(location), Some(name)) => {
                // What the `segment-actions` of that side's document declare for this
                // segment. A segment no such action acts on is shared with nothing and
                // served by nothing, so each list is absent rather than empty.
                let segment = match configured {
                    Some(ChangedObject::Segment(held)) => Some(held),
                    _ => None,
                };
                Some(CoreNetworkChangeValues {
                    segment_name: Some(name.to_string()),
                    edge_locations: Some(vec![location.to_string()]),
                    shared_segments: segment
                        .map(|held| held.shared_segments.clone())
                        .filter(|shared| !shared.is_empty()),
                    service_insertion_actions: segment
                        .map(|held| {
                            held.service_insertions
                                .iter()
                                .map(SegmentInsertion::to_shape)
                                .collect::<Vec<_>>()
                        })
                        .filter(|actions| !actions.is_empty()),
                    ..CoreNetworkChangeValues::default()
                })
            }
            (CHANGE_TYPE_NETWORK_FUNCTION_GROUP, Some(location), Some(name)) => {
                Some(CoreNetworkChangeValues {
                    network_function_group_name: Some(name.to_string()),
                    edge_locations: Some(vec![location.to_string()]),
                    ..CoreNetworkChangeValues::default()
                })
            }
            (CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC, _, Some(segment)) => {
                // A static route is identified completely by the segment whose route tables
                // hold it, its prefix and what that prefix is reached through, so its
                // identity is also everything the document states about it.
                self.route.as_ref().map(|route| CoreNetworkChangeValues {
                    segment_name: Some(segment.to_string()),
                    cidr: Some(route.prefix.clone()),
                    destination_identifier: Some(route.destination.clone()),
                    ..CoreNetworkChangeValues::default()
                })
            }
            _ => None,
        }
    }

    /// The modelled `CoreNetworkChange` for this change: what it does to the object it
    /// names, the values that object held under the policy in force (`PreviousValues`) and
    /// the ones the submitted policy gives it (`NewValues`).
    ///
    /// Nothing precedes an addition and nothing follows a removal, so an
    /// [`CHANGE_ACTION_ADD`] carries no previous values and a [`CHANGE_ACTION_REMOVE`] no
    /// new ones; a [`CHANGE_ACTION_MODIFY`] carries both.
    pub fn to_core_network_change(&self) -> CoreNetworkChange {
        CoreNetworkChange {
            r#type: Some(self.change_type.to_string()),
            action: Some(self.action.to_string()),
            identifier: self.identifier(),
            previous_values: (self.action != CHANGE_ACTION_ADD)
                .then(|| self.to_change_values(self.previous.as_ref()))
                .flatten(),
            new_values: (self.action != CHANGE_ACTION_REMOVE)
                .then(|| self.to_change_values(self.new.as_ref()))
                .flatten(),
            identifier_path: Some(self.identifier_path()),
        }
    }

    /// The modelled `CoreNetworkChangeEvent` for this change, in `status` as of
    /// `event_time`.
    pub fn to_change_event(
        &self,
        status: &str,
        event_time: SystemTime,
    ) -> CoreNetworkChangeEvent {
        // `Values` describes the changed object, so a change to an object none of its
        // members names (the `attachment-policies` block as a whole) carries none rather
        // than an empty structure.
        let values = (self.edge_location.is_some() || self.name.is_some()).then(|| {
            // A static route sits in the route table of the segment it was declared for, so
            // its event names that segment too — "the segment name if the change event is
            // associated with a segment" (`smithy_query shape=CoreNetworkChangeEventValues
            // member=SegmentName`).
            let names_segment = self.change_type == CHANGE_TYPE_CORE_NETWORK_SEGMENT
                || self.change_type == CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC;
            CoreNetworkChangeEventValues {
                edge_location: self.edge_location.clone(),
                segment_name: names_segment.then(|| self.name.clone()).flatten(),
                network_function_group_name: (self.change_type
                    == CHANGE_TYPE_NETWORK_FUNCTION_GROUP)
                    .then(|| self.name.clone())
                    .flatten(),
                // The prefix of the static route this change acts on, which is what the
                // member reports for a static-route event.
                cidr: self.route.as_ref().map(|route| route.prefix.clone()),
                // The remaining members describe objects no policy document configures — a
                // peer edge and a routing policy association — so a change derived from two
                // documents fills none. `AttachmentId` is left out too: a route's
                // destination is whatever the document spelled, which names no attachment at
                // all for a blackholed route, so the destination travels in the change's own
                // `DestinationIdentifier` rather than as an attachment id.
                peer_edge_location: None,
                routing_policy_direction: None,
                attachment_id: None,
                routing_policy_association_details: None,
            }
        });
        CoreNetworkChangeEvent {
            r#type: Some(self.change_type.to_string()),
            action: Some(self.action.to_string()),
            identifier_path: Some(self.identifier_path()),
            event_time: Some(epoch_seconds(event_time)),
            status: Some(status.to_string()),
            values,
        }
    }
}

/// The changes a change set carries: the difference between the policy `in_force` and the
/// `submitted` one, one entry per object the two configure differently, ordered by
/// [`PolicyChange::identifier_path`].
///
/// An object only the submitted policy configures is [`CHANGE_ACTION_ADD`]ed, one only the
/// policy in force configures is [`CHANGE_ACTION_REMOVE`]d, and one both configure
/// differently is [`CHANGE_ACTION_MODIFY`]ed; an object both configure the same way is no
/// change at all and produces no entry, so two identical documents differ in nothing. The
/// objects compared are every one a document configures and [`policy_topology`] therefore
/// derives, with everything the document states about it:
///
/// * each edge of `core-network-configuration`, whose ASN and inside CIDR blocks are what a
///   modification of it is;
/// * each named `segments` entry at each edge location it is present in, carrying what the
///   `segment-actions` acting on that segment declare — the segments its routes are shared
///   with and the service insertion actions that serve it — so a document differing ONLY in
///   those modifies the segment rather than changing nothing;
/// * each named `network-function-groups` entry at each edge location it is present in,
///   which its placement describes completely: which segments it serves is stated on those
///   segments, through the `segment-actions` entries that act on them;
/// * each static route a `create-route` entry declares for a segment, identified by that
///   segment, its prefix and what that prefix reaches — one route per declaration, since a
///   segment holds its route table wherever it is present;
/// * the `attachment-policies` block as a whole, the one block that configures no object of
///   its own.
///
/// Both lists are bounded by the caps [`validate_policy_document`] enforces before a
/// document is stored, so the comparison and its result are bounded too.
pub fn policy_changes(in_force: &PolicyTopology, submitted: &PolicyTopology) -> Vec<PolicyChange> {
    let mut changes: Vec<PolicyChange> = Vec::new();

    let edges_before = edges_by_location(in_force);
    let edges_after = edges_by_location(submitted);
    for location in edges_before.keys().chain(edges_after.keys()).collect::<BTreeSet<_>>() {
        // An edge is configured by its ASN and its inside CIDR blocks, so a difference in
        // either is what makes the change a modification rather than no change.
        let previous_edge = edges_before.get(location).map(|edge| EdgeConfiguration::of(edge));
        let new_edge = edges_after.get(location).map(|edge| EdgeConfiguration::of(edge));
        let action = match (&previous_edge, &new_edge) {
            (None, Some(_)) => CHANGE_ACTION_ADD,
            (Some(_), None) => CHANGE_ACTION_REMOVE,
            (Some(held), Some(given)) if held != given => CHANGE_ACTION_MODIFY,
            _ => continue,
        };
        changes.push(PolicyChange {
            change_type: CHANGE_TYPE_CORE_NETWORK_EDGE,
            action,
            edge_location: Some((*location).to_string()),
            name: None,
            route: None,
            previous: previous_edge.map(ChangedObject::Edge),
            new: new_edge.map(ChangedObject::Edge),
        });
    }

    // A segment's placement carries what the `segment-actions` acting on it declare, so a
    // placement both policies make is a MODIFY exactly where those differ — which is what a
    // version changing only the sharing or the service insertion of a segment does to it.
    let segments_before = in_force.segment_placements();
    let segments_after = submitted.segment_placements();
    for placement in segments_before
        .keys()
        .chain(segments_after.keys())
        .collect::<BTreeSet<_>>()
    {
        let previous = segments_before.get(placement);
        let new = segments_after.get(placement);
        let action = match (previous, new) {
            (None, Some(_)) => CHANGE_ACTION_ADD,
            (Some(_), None) => CHANGE_ACTION_REMOVE,
            (Some(held), Some(given)) if held != given => CHANGE_ACTION_MODIFY,
            _ => continue,
        };
        let (location, name) = placement;
        changes.push(PolicyChange {
            change_type: CHANGE_TYPE_CORE_NETWORK_SEGMENT,
            action,
            edge_location: Some(location.clone()),
            name: Some(name.clone()),
            route: None,
            previous: previous.cloned().map(ChangedObject::Segment),
            new: new.cloned().map(ChangedObject::Segment),
        });
    }

    // A network function group's placement, and one static route, carry nothing beyond what
    // identifies them, so one both policies make is never a MODIFY: it is the same object,
    // unchanged.
    let groups_before = in_force.placements(&in_force.network_function_groups);
    let groups_after = submitted.placements(&submitted.network_function_groups);
    for (placements, action) in [
        (groups_after.difference(&groups_before), CHANGE_ACTION_ADD),
        (groups_before.difference(&groups_after), CHANGE_ACTION_REMOVE),
    ] {
        for (location, name) in placements {
            changes.push(PolicyChange {
                change_type: CHANGE_TYPE_NETWORK_FUNCTION_GROUP,
                action,
                edge_location: Some(location.clone()),
                name: Some(name.clone()),
                route: None,
                previous: None,
                new: None,
            });
        }
    }

    let routes_before = in_force.static_route_placements();
    let routes_after = submitted.static_route_placements();
    for (routes, action) in [
        (routes_after.difference(&routes_before), CHANGE_ACTION_ADD),
        (routes_before.difference(&routes_after), CHANGE_ACTION_REMOVE),
    ] {
        for (segment, route) in routes {
            changes.push(PolicyChange {
                change_type: CHANGE_TYPE_ATTACHMENT_ROUTE_STATIC,
                action,
                edge_location: None,
                name: Some(segment.clone()),
                route: Some(route.clone()),
                previous: None,
                new: None,
            });
        }
    }

    if in_force.attachment_policies != submitted.attachment_policies {
        changes.push(PolicyChange {
            change_type: CHANGE_TYPE_ATTACHMENT_POLICIES_CONFIGURATION,
            action: match (
                in_force.attachment_policies.is_empty(),
                submitted.attachment_policies.is_empty(),
            ) {
                (true, false) => CHANGE_ACTION_ADD,
                (false, true) => CHANGE_ACTION_REMOVE,
                _ => CHANGE_ACTION_MODIFY,
            },
            edge_location: None,
            name: None,
            route: None,
            previous: None,
            new: None,
        });
    }

    changes.sort_by_cached_key(PolicyChange::identifier_path);
    changes
}

/// The edges a topology configures, by edge location. An entry the document spells without
/// a location names no edge and is already absent from the topology.
pub(super) fn edges_by_location(topology: &PolicyTopology) -> BTreeMap<&str, &CoreNetworkEdge> {
    topology
        .edges
        .iter()
        .filter_map(|edge| edge.edge_location.as_deref().map(|location| (location, edge)))
        .collect()
}

/// Derive the reported topology of a core network from its policy document.
///
/// The rules are the published policy schema's own: an edge exists in each location the
/// `core-network-configuration` lists, carrying that entry's `asn` — or, where the entry
/// names none, the next ASN of the configuration's `asn-ranges` that no entry claims, as
/// Cloud WAN assigns one from the ranges — and its `inside-cidr-blocks`; a segment exists
/// for each named entry of `segments`, present in the edge locations it restricts itself
/// to or, where it names none, in every edge location of the core network; the same
/// location rule holds for each named entry of `network-function-groups`; and each
/// `attachment-policies` rule is kept with its conditions and its action, ordered by the
/// ascending `rule-number` the rules are evaluated in. The `segment-actions` entries say
/// what each segment's routes reach: a `share` entry shares the routes of the segment it
/// acts on with the segments its `share-with` names, a `send-via` / `send-to` entry
/// sends that segment's traffic through the network function groups its
/// `via.network-function-groups` names, and a `create-route` entry declares the static
/// routes that segment holds — the prefixes its `destination-cidr-blocks` lists, reached
/// through the attachments its `destinations` names ([`segment_actions`]).
///
/// A document this service cannot parse describes no topology, so it answers the empty
/// one rather than guessing: a document is only stored once
/// [`validate_policy_document`] has accepted it, which also caps every list this walks,
/// so the work and the result are bounded by those caps.
///
/// Called ONCE per document, where that document is accepted and BEFORE the store is
/// locked, and the answer is stored on the record beside it
/// ([`CoreNetworkPolicyRecord::topology`]): a read of a core network reports the derived
/// topology, and no operation — a read, an execution or a restore — re-parses the
/// document or parses one while holding the store lock.
pub fn policy_topology(document: &str) -> PolicyTopology {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return PolicyTopology::default();
    };
    let Some(root) = parsed.as_object() else {
        return PolicyTopology::default();
    };
    let configuration = root
        .get(POLICY_CONFIGURATION)
        .and_then(::serde_json::Value::as_object);
    let declared = configuration
        .and_then(|configuration| configuration.get(POLICY_EDGE_LOCATIONS))
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let ranges = configuration
        .and_then(|configuration| configuration.get(POLICY_ASN_RANGES))
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    let mut pool = AsnPool::new(ranges, declared);
    let edges: Vec<CoreNetworkEdge> = declared
        .iter()
        .filter_map(|entry| {
            let location = entry
                .get(POLICY_LOCATION)
                .and_then(::serde_json::Value::as_str)?;
            let asn = entry
                .get(POLICY_ASN)
                .and_then(::serde_json::Value::as_i64)
                .or_else(|| pool.take());
            Some(CoreNetworkEdge {
                edge_location: Some(location.to_string()),
                asn,
                inside_cidr_blocks: entry
                    .get(POLICY_INSIDE_CIDR_BLOCKS)
                    .and_then(::serde_json::Value::as_array)
                    .map(|blocks| strings_of(blocks)),
            })
        })
        .collect();

    let named = |key: &str, cap: usize| -> Vec<NamedGroup> {
        root.get(key)
            .and_then(::serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            // Both counts are already capped where the document is accepted; the `take`s
            // here keep the derived topology bounded for any document that reaches the
            // deriver, so no caller can make one entry expand without limit.
            .take(cap)
            .filter_map(|entry| {
                let name = entry
                    .get(POLICY_NAME)
                    .and_then(::serde_json::Value::as_str)?;
                Some(NamedGroup {
                    name: name.to_string(),
                    // An entry naming no edge locations is held as "every edge location",
                    // not as a copy of the list: the expansion happens where a response
                    // is built, so nothing quadratic is retained.
                    edge_locations: entry
                        .get(POLICY_EDGE_LOCATIONS)
                        .and_then(::serde_json::Value::as_array)
                        .map(|listed| {
                            strings_of(
                                &listed[..listed.len().min(MAX_POLICY_EDGE_LOCATIONS)],
                            )
                        }),
                })
            })
            .collect()
    };

    let segments = named(POLICY_SEGMENTS, MAX_POLICY_SEGMENTS);
    let network_function_groups =
        named(POLICY_NETWORK_FUNCTION_GROUPS, MAX_POLICY_NETWORK_FUNCTION_GROUPS);
    // The segment actions are read LAST, because an action is only kept where the segments
    // and groups it names are ones the document declares.
    let actions = segment_actions(root, &segments, &network_function_groups);
    PolicyTopology {
        segments,
        network_function_groups,
        edges,
        attachment_policies: attachment_policies(root),
        shares: actions.shares,
        service_insertions: actions.service_insertions,
        static_routes: actions.static_routes,
    }
}

/// What [`segment_actions`] reads out of a document's `segment-actions`, one list per kind
/// of action it derives something from.
pub(super) struct SegmentActions {
    pub(super) shares: Vec<SegmentShare>,
    pub(super) service_insertions: Vec<ServiceInsertion>,
    pub(super) static_routes: Vec<SegmentStaticRoute>,
}

/// The `segment-actions` entries of a policy document's root object this slice derives the
/// reported topology from: the `share` entries, which decide which segments a segment's
/// routes are shared with, the `send-via` / `send-to` ones, which decide which segments a
/// network function group serves, and the `create-route` ones, which declare the static
/// routes a segment's route tables hold.
///
/// An entry is kept only where it is readable AND names declared resources: a segment or a
/// network function group no entry of `segments` / `network-function-groups` declares is
/// none of this core network's, so it is dropped rather than reported. An action of any
/// other kind decides none of the three and is passed over.
///
/// Every list this walks is capped, which is what bounds both the walk and what it retains:
/// the entry count at [`MAX_POLICY_SEGMENT_ACTIONS`], and one entry's name lists at the
/// segment and group caps, or at [`MAX_POLICY_ROUTE_PREFIXES`] /
/// [`MAX_POLICY_ROUTE_DESTINATIONS`] for a `create-route` entry — the counts
/// [`validate_policy_document`] refuses a document past, so a STORED document carries none
/// longer. The kept names are held as SETS where an entry names things once, so a list
/// repeating one name retains it once; a route's prefixes and destinations keep the
/// document's order, which is the order the routes are reported in.
pub(super) fn segment_actions(
    root: &::serde_json::Map<String, ::serde_json::Value>,
    segments: &[NamedGroup],
    network_function_groups: &[NamedGroup],
) -> SegmentActions {
    fn declares(groups: &[NamedGroup], name: &str) -> bool {
        groups.iter().any(|group| group.name == name)
    }

    let mut shares = Vec::new();
    let mut service_insertions = Vec::new();
    let mut static_routes = Vec::new();
    for entry in root
        .get(POLICY_SEGMENT_ACTIONS)
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .take(MAX_POLICY_SEGMENT_ACTIONS)
    {
        let Some(action) = entry
            .get(POLICY_ACTION)
            .and_then(::serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(segment) = entry
            .get(POLICY_SEGMENT)
            .and_then(::serde_json::Value::as_str)
            .filter(|name| declares(segments, name))
        else {
            continue;
        };
        match action {
            POLICY_ACTION_SHARE => {
                if let Some(with) = share_with(entry.get(POLICY_SHARE_WITH), segments) {
                    shares.push(SegmentShare {
                        segment: segment.to_string(),
                        with,
                    });
                }
            }
            POLICY_ACTION_SEND_VIA | POLICY_ACTION_SEND_TO => {
                let groups: BTreeSet<String> = entry
                    .get(POLICY_VIA)
                    .and_then(::serde_json::Value::as_object)
                    .and_then(|via| via.get(POLICY_NETWORK_FUNCTION_GROUPS))
                    .and_then(::serde_json::Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
                    .iter()
                    .take(MAX_POLICY_NETWORK_FUNCTION_GROUPS)
                    .filter_map(::serde_json::Value::as_str)
                    .filter(|name| declares(network_function_groups, name))
                    .map(str::to_string)
                    .collect();
                // An entry sending a segment through no declared group serves nothing, so
                // there is nothing to keep.
                if !groups.is_empty() {
                    service_insertions.push(ServiceInsertion {
                        send_via: action == POLICY_ACTION_SEND_VIA,
                        segment: segment.to_string(),
                        network_function_groups: groups,
                    });
                }
            }
            POLICY_ACTION_CREATE_ROUTE => {
                let listed = |key: &str, cap: usize| -> Vec<String> {
                    entry
                        .get(key)
                        .and_then(::serde_json::Value::as_array)
                        .map_or(&[][..], Vec::as_slice)
                        .iter()
                        .take(cap)
                        .filter_map(::serde_json::Value::as_str)
                        .map(str::to_string)
                        .collect()
                };
                let destination_cidr_blocks =
                    listed(POLICY_DESTINATION_CIDR_BLOCKS, MAX_POLICY_ROUTE_PREFIXES);
                let destinations = listed(POLICY_DESTINATIONS, MAX_POLICY_ROUTE_DESTINATIONS);
                // An entry declaring no prefix declares no route, and one naming no
                // destination says nothing about what its prefixes reach — the published
                // schema requires both of a `create-route` entry — so neither is kept
                // rather than being completed with a rule the document did not state.
                if !destination_cidr_blocks.is_empty() && !destinations.is_empty() {
                    static_routes.push(SegmentStaticRoute {
                        segment: segment.to_string(),
                        destination_cidr_blocks,
                        destinations,
                    });
                }
            }
            _ => {}
        }
    }
    SegmentActions {
        shares,
        service_insertions,
        static_routes,
    }
}

/// How a `share` entry's `share-with` names the segments it shares with, or `None` where
/// the entry carries none or spells it in a form this slice cannot read — such an entry
/// shares with NOTHING, since an unreadable one must not silently widen the sharing it
/// belongs to.
///
/// The published spellings are the list of segment names, the string `*` for every other
/// segment, and an object excepting some of them. Each list is walked only as far as
/// [`MAX_POLICY_SEGMENTS`] entries — the count [`validate_policy_document`] refuses a
/// document past — and only names `segments` declares are kept, each once, so what one
/// entry retains is bounded by that cap.
pub(super) fn share_with(value: Option<&::serde_json::Value>, segments: &[NamedGroup]) -> Option<ShareWith> {
    let declared = |name: &&str| segments.iter().any(|group| group.name == *name);
    let kept = |names: &[::serde_json::Value]| -> BTreeSet<String> {
        names
            .iter()
            .take(MAX_POLICY_SEGMENTS)
            .filter_map(::serde_json::Value::as_str)
            .filter(declared)
            .map(str::to_string)
            .collect()
    };
    match value? {
        ::serde_json::Value::String(spelt) if spelt == POLICY_SHARE_WITH_ALL => {
            Some(ShareWith::AllExcept(BTreeSet::new()))
        }
        ::serde_json::Value::Array(names) => Some(ShareWith::Named(kept(names))),
        ::serde_json::Value::Object(object) => object
            .get(POLICY_EXCEPT)
            .and_then(::serde_json::Value::as_array)
            .map(|excepted| ShareWith::AllExcept(kept(excepted))),
        _ => None,
    }
}

/// The `attachment-policies` rules of a policy document's root object, in the order they
/// are evaluated: ascending `rule-number`. A rule the document spells without the
/// `rule-number` that orders it, or without the `action` that says what it does, describes
/// no decision and is skipped — [`validate_policy_document`] refuses such a document, which
/// is what keeps a STORED document from carrying one. Both the rule count and each rule's
/// condition count are capped there too, so this walk is bounded.
pub(super) fn attachment_policies(
    root: &::serde_json::Map<String, ::serde_json::Value>,
) -> Vec<AttachmentPolicyRule> {
    let mut rules: Vec<AttachmentPolicyRule> = root
        .get(POLICY_ATTACHMENT_POLICIES)
        .and_then(::serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .take(MAX_POLICY_ATTACHMENT_POLICIES)
        .filter_map(|entry| {
            let rule_number = entry
                .get(POLICY_RULE_NUMBER)
                .and_then(::serde_json::Value::as_i64)?;
            let action = entry
                .get(POLICY_ACTION)
                .and_then(::serde_json::Value::as_object)?;
            let conditions = entry
                .get(POLICY_CONDITIONS)
                .and_then(::serde_json::Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .take(MAX_POLICY_CONDITIONS_PER_RULE)
                .filter_map(|condition| {
                    let condition_type = condition
                        .get(POLICY_TYPE)
                        .and_then(::serde_json::Value::as_str)?;
                    Some(AttachmentCondition {
                        condition_type: condition_type.to_string(),
                        operator: text_of(condition, POLICY_OPERATOR),
                        key: text_of(condition, POLICY_KEY),
                        value: text_of(condition, POLICY_VALUE),
                    })
                })
                .collect();
            // The segment the action names, by the method it names it with. An action
            // asking for neither method names no segment, so the attachment joins none.
            let segment = match action
                .get(POLICY_ASSOCIATION_METHOD)
                .and_then(::serde_json::Value::as_str)
            {
                Some(POLICY_ASSOCIATION_CONSTANT) => action
                    .get(POLICY_SEGMENT)
                    .and_then(::serde_json::Value::as_str)
                    .map(|name| SegmentAssociation::Constant(name.to_string())),
                Some(POLICY_ASSOCIATION_TAG) => action
                    .get(POLICY_TAG_VALUE_OF_KEY)
                    .and_then(::serde_json::Value::as_str)
                    .map(|key| SegmentAssociation::TagValueOfKey(key.to_string())),
                Some(_) | None => None,
            };
            Some(AttachmentPolicyRule {
                // A `rule-number` outside the `Integer` the attachment reports it through
                // is clamped to that range rather than dropped: the rule still decides,
                // and the number it reports stays inside the modelled type.
                rule_number: rule_number.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                all_conditions: entry
                    .get(POLICY_CONDITION_LOGIC)
                    .and_then(::serde_json::Value::as_str)
                    != Some(POLICY_LOGIC_OR),
                conditions,
                require_acceptance: action
                    .get(POLICY_REQUIRE_ATTACHMENT_ACCEPTANCE)
                    .and_then(::serde_json::Value::as_bool)
                    .unwrap_or(false),
                segment,
                network_function_group: action
                    .get(POLICY_ADD_TO_NETWORK_FUNCTION_GROUP)
                    .and_then(::serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect();
    // Evaluated lowest rule number first, so the order is fixed here rather than at every
    // attachment create. `sort_by_key` is stable, so two rules sharing a number keep the
    // order the document listed them in.
    rules.sort_by_key(|rule| rule.rule_number);
    rules
}

/// How many entries of one list a policy document may carry.
///
/// The derived topology is the PRODUCT of the edge-location count and the segment (or
/// network-function-group) count — a segment that restricts itself to no edge location
/// exists in all of them — so a document whose lists are unbounded would expand into a
/// response orders of magnitude larger than the request. These three caps are CHOSEN,
/// like [`MAX_GLOBAL_NETWORKS`]: AWS's own Cloud WAN quotas for edge locations and
/// segments per core network are in the tens, the model carries no number (the document
/// is an opaque `CoreNetworkPolicyDocument`, len 0..10000000), and their purpose here is
/// that one caller cannot make this emulator hold or compute without bound. A document
/// past a cap is refused with `CoreNetworkPolicyException` (HTTP 400) and stored nowhere,
/// so the derived topology is at most
/// `MAX_POLICY_EDGE_LOCATIONS * (MAX_POLICY_SEGMENTS + MAX_POLICY_NETWORK_FUNCTION_GROUPS)`
/// edge-location strings.
pub const MAX_POLICY_EDGE_LOCATIONS: usize = 100;
/// See [`MAX_POLICY_EDGE_LOCATIONS`].
pub const MAX_POLICY_SEGMENTS: usize = 100;
/// See [`MAX_POLICY_EDGE_LOCATIONS`].
pub const MAX_POLICY_NETWORK_FUNCTION_GROUPS: usize = 100;
/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many `attachment-policies` rules one document may
/// carry, and how many conditions one of those rules may carry. CHOSEN for the same reason
/// — every rule of the policy in force is walked, in order, for each attachment created, so
/// the work one create costs is bounded by their product.
pub const MAX_POLICY_ATTACHMENT_POLICIES: usize = 100;
/// See [`MAX_POLICY_ATTACHMENT_POLICIES`].
pub const MAX_POLICY_CONDITIONS_PER_RULE: usize = 20;
/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many `segment-actions` entries one document may
/// carry. CHOSEN for the same reason — every entry is walked to derive which segments a
/// segment's routes reach and which segments a network function group serves, so a document
/// past this cap is refused rather than silently applied in part. The name lists INSIDE one
/// entry are capped at [`MAX_POLICY_SEGMENTS`] /
/// [`MAX_POLICY_NETWORK_FUNCTION_GROUPS`] — the counts of the things they may name — so one
/// entry's kept names, and the work of reporting them, are bounded by those caps too.
pub const MAX_POLICY_SEGMENT_ACTIONS: usize = 100;

/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many `asn-ranges` entries of one document the
/// ASN pool reads. CHOSEN for the same reason — the pool keeps one `(low, high)` pair per
/// entry, and only as many entries as there can be edges to assign an ASN to are ever
/// reached, so reading past this cap could add nothing to any answer.
pub const MAX_POLICY_ASN_RANGES: usize = 100;

/// See [`MAX_POLICY_EDGE_LOCATIONS`]: how many prefixes one `create-route` entry may
/// declare, and how many destinations it may name. CHOSEN for the same reason — the routes
/// one segment holds are the PRODUCT of the three counts, and every one of them is
/// materialised to report that segment's routing information, so the work one listing costs
/// and the answer it can grow to are bounded by
/// `MAX_POLICY_SEGMENT_ACTIONS * MAX_POLICY_ROUTE_PREFIXES * MAX_POLICY_ROUTE_DESTINATIONS`
/// routes rather than by the document's length. The numbers are generous against what an
/// entry can usefully name: a destination names one attachment of the core network, or the
/// blackhole, and a prefix is reached once per destination however often the document
/// repeats it. AWS publishes no quota these follow, so a document real Cloud WAN would
/// accept is refused here past either number — the same trade every other policy cap of
/// this slice makes ([`MAX_POLICY_EDGE_LOCATIONS`]).
pub const MAX_POLICY_ROUTE_PREFIXES: usize = 50;
/// See [`MAX_POLICY_ROUTE_PREFIXES`].
pub const MAX_POLICY_ROUTE_DESTINATIONS: usize = 10;

/// How long one value inside a policy document may be, in characters.
///
/// These two are the MODEL's own numbers, not chosen: the shapes the derived topology is
/// reported through constrain them. An edge location is reported as
/// `CoreNetworkEdge$EdgeLocation`, an `ExternalRegionCode` of length 1..63, and a segment
/// or group name as a `ConstrainedString` of length 0..256 (`smithy_query
/// shape=CoreNetworkEdge,CoreNetworkSegment`). Capping them is also what bounds the
/// derived topology in BYTES, not merely in entries: at most
/// `MAX_POLICY_EDGE_LOCATIONS * MAX_POLICY_LOCATION_CHARS` characters of location text,
/// expanded into at most `MAX_POLICY_SEGMENTS + MAX_POLICY_NETWORK_FUNCTION_GROUPS`
/// entries, however large the document that carried them.
pub const MAX_POLICY_LOCATION_CHARS: usize = 63;
/// See [`MAX_POLICY_LOCATION_CHARS`].
pub const MAX_POLICY_NAME_CHARS: usize = 256;

/// The ASN domain an `asn-ranges` entry has to lie inside: a BGP AS number is a 32-bit
/// number and AS 0 is reserved, so the assignable numbers are 1..=4294967295. A range
/// reaching outside it assigns nothing (see [`parse_asn_range`]).
pub(super) const ASN_MIN: i64 = 1;
/// See [`ASN_MIN`].
pub(super) const ASN_MAX: i64 = 4_294_967_295;

/// The `ErrorCode` each policy refusal carries. The model types
/// `CoreNetworkPolicyError$ErrorCode` as a free-form string and enumerates no values
/// (`smithy_query shape=CoreNetworkPolicyError`), so each code names its own rule and the
/// identifying detail travels in the shape's `Path` (the JSON path of the offending node).
pub const POLICY_ERROR_INVALID_JSON: &str = "INVALID_JSON";
/// See [`POLICY_ERROR_INVALID_JSON`].
pub const POLICY_ERROR_MISSING_FIELD: &str = "MISSING_REQUIRED_FIELD";
/// See [`POLICY_ERROR_INVALID_JSON`].
pub const POLICY_ERROR_INVALID_TYPE: &str = "INVALID_TYPE";
/// See [`POLICY_ERROR_INVALID_JSON`]; raised past one of the [`MAX_POLICY_EDGE_LOCATIONS`]
/// list caps.
pub const POLICY_ERROR_TOO_MANY_ENTRIES: &str = "TOO_MANY_ENTRIES";
/// See [`POLICY_ERROR_INVALID_JSON`]; raised for a value outside the length the shape it
/// is reported through allows ([`MAX_POLICY_LOCATION_CHARS`],
/// [`MAX_POLICY_NAME_CHARS`]).
pub const POLICY_ERROR_INVALID_VALUE: &str = "INVALID_VALUE";

/// One entry of a `CoreNetworkPolicyException`'s `Errors` list.
pub(super) fn policy_error(code: &str, message: String, path: &str) -> CoreNetworkPolicyError {
    CoreNetworkPolicyError {
        error_code: code.to_string(),
        message,
        path: Some(path.to_string()),
    }
}

/// Check one edge-location string of a policy document, at the JSON path `path`: a location
/// is reported through `ExternalRegionCode` (len 1..63, `smithy_query
/// shape=ExternalRegionCode`), so one outside that length is refused. The same rule holds
/// wherever a document names a location — the configuration's own `edge-locations`, and the
/// list a segment or network function group restricts itself to — so both walks use this.
pub(super) fn check_policy_location(path: &str, location: &str, errors: &mut Vec<CoreNetworkPolicyError>) {
    let length = location.chars().count();
    if length == 0 || length > MAX_POLICY_LOCATION_CHARS {
        errors.push(policy_error(
            POLICY_ERROR_INVALID_VALUE,
            format!(
                "a {POLICY_LOCATION:?} must be 1 to {MAX_POLICY_LOCATION_CHARS} characters, but {length} were given"
            ),
            path,
        ));
    }
}

/// The string at `key` of a JSON object, or `None` when it carries none there.
pub(super) fn text_of(value: &::serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(::serde_json::Value::as_str)
        .map(str::to_string)
}

/// The strings of a JSON array, skipping any entry that is not one.
pub(super) fn strings_of(values: &[::serde_json::Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(::serde_json::Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Hands out the ASNs a policy document's `asn-ranges` makes available, in ascending
/// order, skipping every ASN an edge location claims explicitly.
///
/// A range is written `"<low>-<high>"` (inclusive) or as a single ASN, and a range can
/// span millions of numbers, so the candidates are walked with a cursor rather than
/// materialised.
#[derive(Debug)]
pub(super) struct AsnPool {
    /// The inclusive `(low, high)` ranges, in the order the document lists them.
    pub(super) ranges: Vec<(i64, i64)>,
    /// How far into `ranges` the walk has got: the index and the next candidate in it.
    pub(super) cursor: usize,
    /// The next candidate inside `ranges[cursor]`.
    pub(super) next: i64,
    /// The ASNs edge locations claim explicitly, which an assignment skips.
    pub(super) claimed: BTreeSet<i64>,
}

impl AsnPool {
    /// The pool `ranges` describes, with every ASN the `edge-locations` entries claim
    /// explicitly already taken.
    ///
    /// Both caller-written lists are read under the cap their count is bounded by
    /// ([`MAX_POLICY_ASN_RANGES`], [`MAX_POLICY_EDGE_LOCATIONS`]), so what one document can
    /// make this pool hold is bounded by those caps rather than by the document.
    pub(super) fn new(ranges: &[::serde_json::Value], edge_locations: &[::serde_json::Value]) -> Self {
        let ranges: Vec<(i64, i64)> = ranges
            .iter()
            .take(MAX_POLICY_ASN_RANGES)
            .filter_map(parse_asn_range)
            .collect();
        let claimed = edge_locations
            .iter()
            .take(MAX_POLICY_EDGE_LOCATIONS)
            .filter_map(|entry| {
                entry
                    .get(POLICY_ASN)
                    .and_then(::serde_json::Value::as_i64)
            })
            .collect();
        let next = ranges.first().map_or(0, |(low, _)| *low);
        Self {
            ranges,
            cursor: 0,
            next,
            claimed,
        }
    }

    /// The next free ASN, or `None` once the ranges are exhausted (or none were given).
    pub(super) fn take(&mut self) -> Option<i64> {
        while let Some((low, high)) = self.ranges.get(self.cursor).copied() {
            let candidate = self.next.max(low);
            if candidate > high {
                self.advance();
                continue;
            }
            match candidate.checked_add(1) {
                Some(next) => self.next = next,
                // The candidate is the largest number there is, so this range holds
                // nothing after it: the walk moves on to the next range whether or not
                // the candidate itself is free. Advancing unconditionally is what keeps
                // the walk strictly monotonic — a saturating increment would re-derive
                // this same candidate forever once it is claimed.
                None => self.advance(),
            }
            if self.claimed.insert(candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Move the walk on to the next range.
    pub(super) fn advance(&mut self) {
        self.cursor = self.cursor.saturating_add(1);
        self.next = self.ranges.get(self.cursor).map_or(0, |(low, _)| *low);
    }
}

/// One `asn-ranges` entry: `"64512-64555"`, `"64512"` or a bare number.
///
/// A range that is empty, or that reaches outside the [`ASN_MIN`]..=[`ASN_MAX`] domain a
/// BGP AS number lives in, names no assignable ASN and is answered `None` — an entry no
/// assignment can be made from, rather than one that is quietly reinterpreted.
pub(super) fn parse_asn_range(value: &::serde_json::Value) -> Option<(i64, i64)> {
    let (low, high) = if let Some(single) = value.as_i64() {
        (single, single)
    } else {
        let text = value.as_str()?.trim();
        match text.split_once('-') {
            Some((low, high)) => (low.trim().parse::<i64>().ok()?, high.trim().parse::<i64>().ok()?),
            None => {
                let single = text.parse::<i64>().ok()?;
                (single, single)
            }
        }
    };
    (low <= high && low >= ASN_MIN && high <= ASN_MAX).then_some((low, high))
}
