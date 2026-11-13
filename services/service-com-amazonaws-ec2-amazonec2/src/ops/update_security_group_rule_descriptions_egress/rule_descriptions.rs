// SPDX-License-Identifier: CC0-1.0

//! Which stored security group rules an update-descriptions request names, and the
//! description each of them is to carry afterwards.
//!
//! "You can replace an existing description, or add a description to a rule that did not have
//! one previously … you can remove a description for a security group rule by omitting the
//! description parameter in the request"
//! (`UpdateSecurityGroupRuleDescriptionsEgress`): the request names rules either by
//! `SecurityGroupRuleDescriptions` — a rule id beside the description it takes — or by
//! `IpPermissions`, where each grant's own members identify the stored rule it describes and
//! its `Description` member is what that rule takes. Both readings answer the one question
//! this module exists for: WHICH stored rule, and WHICH description.
//!
//! A rule is matched by [`state::SecurityGroupRuleAtom::identity`] — the same comparison an
//! authorize weighs a duplicate by and a revoke finds a rule by — so a description update
//! reaches exactly the rule a revoke naming the same members would reach, and the description
//! is left out of that comparison because it is what the request is about to replace.
//!
//! The whole request is read into a plan BEFORE anything is written, so a request naming one
//! rule the group does not hold writes none of the descriptions beside it.
//!
//! This reading is shared by both DIRECTIONS of the operation, inbound and outbound: a rule
//! is identified the same way whichever side of the group holds it, which is why `is_egress`
//! is a parameter here rather than a fact of this module. It lives beside the outbound half
//! because that is the operation this file was written for.

use std::collections::{HashMap, HashSet};

use crate::ops::types::{IpPermission, SecurityGroupRuleDescription};
use crate::scaffolded::Scaffolded;
use crate::state::{self, ComAmazonawsEc2Amazonec2State, SecurityGroupRuleKey};

/// One stored rule the request names, and the description it carries once the request is
/// served.
#[derive(Debug, Clone)]
pub struct PlannedDescription {
    /// The `SecurityGroupRuleId` the store holds the rule under, which an update never
    /// changes: the rule is a resource and only its description is being written.
    pub rule_id: String,
    /// The description the rule takes — `None` for the request that "remove[s] a description
    /// … by omitting the description parameter".
    pub description: Option<String>,
}

/// Hold the request to everything that can be decided from the REQUEST ALONE, before the
/// store is reached at all: a request naming no servable update costs no lookup, no
/// allocation and no lock.
///
/// Both lists are CALLER-SIZED and the model declares no `@length` for either, so they are
/// capped the way every other operation of this crate caps a caller-sized list, at the same
/// [`state::MAX_RESOURCE_IDS`] — and so is the number of RULES the permissions name, one
/// permission naming one rule per grant.
pub fn validate_request<E>(
    descriptions: &[SecurityGroupRuleDescription],
    permissions: &[IpPermission],
) -> Result<(), Scaffolded<E>> {
    if descriptions.len() > state::MAX_RESOURCE_IDS {
        return Err(state::invalid_parameter_value(format!(
            "SecurityGroupRuleDescriptions carries {} entries, above the maximum of {max}",
            descriptions.len(),
            max = state::MAX_RESOURCE_IDS,
        )));
    }
    if permissions.len() > state::MAX_RESOURCE_IDS {
        return Err(state::invalid_parameter_value(format!(
            "IpPermissions carries {} entries, above the maximum of {max}",
            permissions.len(),
            max = state::MAX_RESOURCE_IDS,
        )));
    }
    let named_rules: usize = permissions
        .iter()
        .map(state::ip_permission_grant_count)
        .sum();
    if named_rules > state::MAX_RESOURCE_IDS {
        return Err(state::invalid_parameter_value(format!(
            "IpPermissions names {named_rules} rules, above the maximum of {max}",
            max = state::MAX_RESOURCE_IDS,
        )));
    }
    // "You must specify either the IP permissions or the description"
    // (`$SecurityGroupRuleDescriptions`) — either, not both: the two name rules in two ways
    // and each carries its own description, so a request carrying both has not said which of
    // them it means.
    if !descriptions.is_empty() && !permissions.is_empty() {
        return Err(state::invalid_parameter_combination(
            "An update names its rules either by SecurityGroupRuleDescriptions or by \
             IpPermissions, not by both",
        ));
    }
    if descriptions.is_empty() && permissions.is_empty() {
        return Err(state::missing_parameter(
            "The request must contain either IpPermissions or SecurityGroupRuleDescriptions",
        ));
    }
    Ok(())
}

/// The updates the request asks for, as the stored rules they will be written onto — or the
/// refusal the request has earned, with nothing written.
///
/// `vpc_id` is the VPC of the group being updated, which is where a grant naming a referenced
/// security group by NAME is resolved: "[Default VPC] The name of the source security group"
/// (`UserIdGroupPair$GroupName`) is the default-VPC spelling of the group a stored rule names
/// by id, and names are unique per VPC.
pub fn planned_descriptions<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    group_id: &str,
    vpc_id: &str,
    is_egress: bool,
    descriptions: &[SecurityGroupRuleDescription],
    permissions: &[IpPermission],
) -> Result<Vec<PlannedDescription>, Scaffolded<E>> {
    let mut planned: Vec<PlannedDescription> = Vec::new();
    let mut named: HashSet<String> = HashSet::new();

    if !descriptions.is_empty() {
        // "Not an id of this kind" is asked BEFORE "no such rule", for the WHOLE list: the
        // order [`state::require_well_formed_id`] documents, so a malformed id is never
        // reported as a rule the group does not hold just because a well-formed unknown one
        // came earlier in the list.
        for (index, entry) in descriptions.iter().enumerate() {
            let member = format!(
                "securityGroupRuleDescription.{position}.securityGroupRuleId",
                position = index + 1,
            );
            let rule_id = entry.security_group_rule_id.as_deref().unwrap_or_default();
            // An entry that names no rule at all has nothing to describe: a form-encoded
            // member sent with no value has not named one, and reading it as one would look
            // for a rule under the empty id.
            if rule_id.is_empty() {
                return Err(state::missing_parameter(format!(
                    "The request must contain the parameter {member}"
                )));
            }
            state::require_well_formed_id(
                state::SECURITY_GROUP_RULE_ID_PREFIX,
                rule_id,
                state::INVALID_SECURITY_GROUP_RULE_ID_MALFORMED,
            )?;
        }
        for entry in descriptions {
            let rule_id = entry.security_group_rule_id.as_deref().unwrap_or_default();
            // The description a rule is about to carry is held to the bounds and the alphabet
            // the model publishes for one, through the SAME reader an authorize holds a new
            // rule's description to.
            state::validate_security_group_rule_description(entry.description.as_ref())?;
            // A rule of ANOTHER group, or of the other SIDE of this one, is not a rule this
            // request can describe: the operation updates "an egress (outbound) security
            // group rule" of the group its handle names, so from here such an id names
            // nothing.
            let holds_the_rule = store
                .security_group_rules
                .get(rule_id)
                .is_some_and(|record| {
                    record.rule.group_id.as_deref() == Some(group_id)
                        && record.rule.is_egress.unwrap_or(false) == is_egress
                });
            if !holds_the_rule {
                return Err(state::invalid_security_group_rule_id_not_found(rule_id));
            }
            plan_one(&mut planned, &mut named, rule_id, entry.description.clone())?;
        }
        return Ok(planned);
    }

    // "The IP protocol name (tcp, udp, icmp) or number" (`IpPermission$IpProtocol`)
    // identifies a rule as much as its ports and its peer do, so a permission that names no
    // protocol names no rule — and reading an absent one as all-protocols would let such a
    // request describe the group's `-1` rule.
    if let Some(index) = permissions.iter().position(|permission| {
        permission
            .ip_protocol
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    }) {
        return Err(state::missing_parameter(format!(
            "The request must contain the parameter IpPermissions.{}.IpProtocol",
            index + 1
        )));
    }
    let mut asked: Vec<state::SecurityGroupRuleAtom> = permissions
        .iter()
        .flat_map(state::security_group_rule_atoms)
        .collect();
    if asked.is_empty() {
        return Err(state::missing_parameter(
            "Every rule must name a peer: a CIDR IP address range, a prefix list or a \
             security group",
        ));
    }
    for rule in &mut asked {
        state::validate_security_group_rule_description(rule.description.as_ref())?;
        let state::SecurityGroupRuleSource::ReferencedGroup(peer) = &rule.source else {
            continue;
        };
        if state::security_group_id_is_well_formed(peer) {
            continue;
        }
        // A name no group of that VPC answers to is left as it stands: it matches no stored
        // rule, which is the answer such a request has earned.
        if let Some(resolved) = store.security_group_id_for_name(vpc_id, peer) {
            rule.source = state::SecurityGroupRuleSource::ReferencedGroup(resolved);
        }
    }

    // The group's own rules on this side, by identity, each remembered by the id it is stored
    // under — one hash lookup per requested rule, so a caller-sized batch costs time
    // proportional to the batch rather than to the batch times the rules the group holds.
    // First one wins: a store that somehow held one rule twice still has one rule to describe.
    let held = store.held_security_group_rules(group_id, is_egress);
    let mut by_identity: HashMap<SecurityGroupRuleKey, &str> = HashMap::with_capacity(held.len());
    for (rule_id, atom) in &held {
        by_identity.entry(atom.identity()).or_insert(rule_id);
    }
    for rule in &asked {
        // "The values that you specify … must match the existing rule's values exactly":
        // a rule the group does not hold has no description to update, and the refusal is
        // the one EC2 gives for a rule a request named that the group does not hold.
        let Some(rule_id) = by_identity.get(&rule.identity()) else {
            return Err(state::invalid_permission_not_found());
        };
        plan_one(&mut planned, &mut named, rule_id, rule.description.clone())?;
    }
    Ok(planned)
}

/// What serving `planned` would charge the store's byte budget, and what it would give back:
/// the bytes each named rule will hold once its description is written, beside the bytes it
/// holds now.
///
/// Asked over the WHOLE request before any of it is written, with the same arithmetic the
/// records will be charged with ([`state::security_group_rule_payload_bytes`]), so a request
/// that would pass the budget is refused with nothing written rather than part-way through.
/// A rule the store no longer holds is charged nothing, there being no record to write.
pub fn stored_bytes_change(
    store: &ComAmazonawsEc2Amazonec2State,
    planned: &[PlannedDescription],
) -> (usize, usize) {
    let mut charge = 0usize;
    let mut refund = 0usize;
    for update in planned {
        let Some(record) = store.security_group_rules.get(&update.rule_id) else {
            continue;
        };
        refund = refund.saturating_add(record.payload_bytes);
        let mut would_hold = record.clone();
        would_hold.rule.description = update.description.clone();
        charge =
            charge.saturating_add(state::security_group_rule_payload_bytes(&would_hold));
    }
    (charge, refund)
}

/// Write `planned` onto the stored rules, through the one writer of a rule's description
/// ([`ComAmazonawsEc2Amazonec2State::set_security_group_rule_description`]), which recharges
/// each record's bytes and recomputes the group's aggregate `IpPermissions`.
///
/// Every rule named here was resolved by [`planned_descriptions`], so each update writes the
/// rule it named; asserted rather than refused, because a refusal at this point would answer
/// an error with the earlier descriptions already written.
pub fn apply(store: &mut ComAmazonawsEc2Amazonec2State, planned: &[PlannedDescription]) {
    for update in planned {
        let written = store
            .set_security_group_rule_description(&update.rule_id, update.description.clone());
        debug_assert!(
            written.is_some(),
            "every planned description names a stored rule"
        );
    }
}

/// Add one update to the plan, refusing a rule the request has already named.
///
/// One call cannot say two different things about one rule: the second entry for a rule would
/// silently win over the first, so the request is refused instead — the same reading
/// [`crate::ops::modify_security_group_rules`] holds a batch of modifications to.
fn plan_one<E>(
    planned: &mut Vec<PlannedDescription>,
    named: &mut HashSet<String>,
    rule_id: &str,
    description: Option<String>,
) -> Result<(), Scaffolded<E>> {
    if !named.insert(rule_id.to_string()) {
        return Err(state::invalid_parameter_value(format!(
            "The security group rule ID '{rule_id}' is named more than once; one request \
             describes a rule once"
        )));
    }
    planned.push(PlannedDescription {
        rule_id: rule_id.to_string(),
        description,
    });
    Ok(())
}
