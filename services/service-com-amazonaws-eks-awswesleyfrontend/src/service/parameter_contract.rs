// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of a request parameter the framework's
//! validating walk raised.
//!
//! RULE: a parameter Amazon EKS states its own refusal for is refused in THAT
//! sentence, whether the rule it broke is one the model declares or one only the
//! service enforces — the two cannot be told apart by a caller, and only one of them
//! is the contract this API publishes.
//!
//! WHERE it is worded cannot be a handler for the rules the model DOES declare: a
//! modelled `@required`, `@pattern`, `@range` or enum value set travels into the
//! operation's generated input schema, so the framework's walk refuses the value
//! before any handler runs and words it as one more constrained leaf — `Value
//! '<value>' at '<member>' failed to satisfy constraint: …`
//! (`vendor/smithy-schema-server/src/constraints.rs`). So the wording is decided in
//! the two seams a service has before a handler answers:
//!
//! * [`crate::service::EmulatedService::override_refusal`], which the walk calls with
//!   the violations it found, for the MESSAGE; and
//! * [`super::precedence`], for a `CreateCluster` whose refusal also carries the
//!   `clusterName` the request asked for — a member of the refusal payload, which the
//!   per-service message seam cannot reach.
//!
//! The sentences themselves are not here: they belong to the parameters, so they live
//! beside the rules each one obeys, in [`crate::state`]. What lives here is the map
//! from a violation the walk describes structurally to the sentence this API publishes
//! for it, so one violated rule earns one sentence no matter which seam answers it.

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};

use crate::ops::types::{
    InvalidParameterException, CLUSTER_VERSION_STATUS_VALUES, LOG_TYPE_VALUES,
};
use crate::state;

/// The member every listing of this service bounds its page size with
/// (`smithy_query(query="operation",operation="ListClusters")` reports `maxResults`
/// `@range {min: 1, max: 100}`).
const MAX_RESULTS_MEMBER: &str = "maxResults";

/// The members this service binds a CLUSTER NAME to, which is what makes a length
/// maximum broken on one a cluster name's own
/// ([`crate::state::MAX_CLUSTER_NAME_LEN`]): `name` on the cluster operations and
/// `clusterName` on the operations of a resource inside one.
const CLUSTER_NAME_MEMBERS: &[&str] = &["name", "clusterName"];

/// The member a violation is about: the last segment of the dotted path the walk
/// spells its position with, or `None` when the position is INSIDE a collection.
///
/// A collection's element is spelled `<member>.<position>.member`
/// (`vendor/smithy-schema-server/src/constraints.rs`), so the sentences here — each of
/// which names one parameter — cannot state where in the collection the value sat. A
/// violation inside one keeps the framework's own report, which does.
fn member_of(path: &str) -> Option<&str> {
    let member = path.rsplit('.').next().unwrap_or(path);
    (member != COLLECTION_STEP && !member.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some(member)
}

/// The path segment the walk marks a step from a collection into its element with.
const COLLECTION_STEP: &str = "member";

/// Whether `declared` is `value_set`, read through the generated constant for that
/// enum so this cannot disagree with the walk about what the model declares. Compared
/// as a SET: the walk states the values in the model's own order, which is not the
/// order the generated constant holds them in.
fn same_value_set(declared: &[String], value_set: &[&str]) -> bool {
    declared.len() == value_set.len()
        && value_set
            .iter()
            .all(|value| declared.iter().any(|declared| declared == value))
}

/// The sentence Amazon EKS states for the rule these violations broke, or `None` when
/// it states none — in which case the framework's own report stands, which is what
/// every other constrained leaf of this service still answers with.
///
/// Only the FIRST violation is worded: each of these sentences is about ONE parameter,
/// and the recorded service answers a request that breaks one of these rules with that
/// one sentence rather than with a list.
pub fn refusal_message(violations: &[Violation]) -> Option<String> {
    let violation = violations.first()?;
    // The sentences keyed on the ENUM a value was judged against come first: each
    // names the rejected VALUE rather than the member it arrived in, so it holds
    // wherever the model puts that member — including inside a collection, where the
    // member-named sentences below cannot reach.
    if let ViolationKind::EnumValue(declared) = &violation.kind {
        let value = violation.value.as_deref().unwrap_or_default();
        // The cluster-version catalog's support-status filter.
        if same_value_set(declared, CLUSTER_VERSION_STATUS_VALUES) {
            return Some(state::cluster_version_status_message(value));
        }
        // One control-plane log type of a cluster's logging configuration.
        if same_value_set(declared, LOG_TYPE_VALUES) {
            return Some(state::unsupported_log_type_message(value));
        }
    }
    let member = member_of(&violation.path)?;
    match &violation.kind {
        // "The parameter <member> is required." — the sentence for a member this API
        // requires and the request omitted, whichever member it is.
        ViolationKind::MissingRequiredMember => Some(state::parameter_required_message(member)),
        // A member bound to the cluster-name grammar: the violated pattern IS that
        // grammar, so this words every member of this service the model binds to it
        // and nothing else.
        ViolationKind::Pattern(pattern) if pattern == state::CLUSTER_NAME_PATTERN => {
            Some(state::parameter_invalid_characters_message(member))
        }
        // A cluster-name member past its published maximum. The maximum stated is the
        // one the model declares for that member, read off the violation.
        ViolationKind::MaxLength(maximum) if CLUSTER_NAME_MEMBERS.contains(&member) => {
            Some(state::parameter_too_long_message(member, *maximum))
        }
        // A page size outside its bounds. The bound the value broke is compared
        // against the published one, so a differently bounded member of some later
        // model revision falls through to the framework's report rather than being
        // told a bound it does not have.
        ViolationKind::MinValue(min)
            if member == MAX_RESULTS_MEMBER
                && min.as_str() == state::MIN_LIST_MAX_RESULTS.to_string() =>
        {
            Some(state::max_results_range_message())
        }
        ViolationKind::MaxValue(max)
            if member == MAX_RESULTS_MEMBER
                && max.as_str() == state::MAX_LIST_MAX_RESULTS.to_string() =>
        {
            Some(state::max_results_range_message())
        }
        // Every member judged by an enum's value set that this API states no sentence
        // of its own for: the sentence names the member and the values it accepts,
        // read off the violation so they are the set the model declares, in the order
        // it declares them.
        //
        // The two members whose sentence is their operation's own — a capability
        // `type` and an add-on `resolveConflicts` — never reach this seam: both are
        // answered before dispatch, in the sentence that operation states (see
        // [`super::off_enum_member_contract`]).
        ViolationKind::EnumValue(declared) => Some(state::member_must_be_one_of_message(
            member,
            declared,
        )),
        _ => None,
    }
}

/// The refusal `CreateCluster` answers a parameter the walk refused with: Amazon EKS's
/// own sentence for the rule it broke, and — for the rules whose refusal names it —
/// the `clusterName` the request asked for.
///
/// `None` when this API states no sentence for that rule, which leaves the request to
/// the modelled operation and the framework's own report — the answer it has always
/// given for one.
///
/// WHICH refusals name the cluster is recorded behaviour: the ones stated in the
/// parameter family of sentences ("The parameter … is required.", the grammar's and
/// the length maximum's) carry the `clusterName` the request supplied, while the
/// value-set sentence ([`state::member_must_be_one_of_message`]) names only the member
/// it is about. A request that supplied no name — the omitted `name` this same
/// function words the refusal of — names no cluster in its refusal either.
pub fn create_cluster_refusal(
    violations: &[Violation],
    requested_name: &str,
) -> Option<InvalidParameterException> {
    let message = refusal_message(violations)?;
    let names_cluster = !matches!(violations.first()?.kind, ViolationKind::EnumValue(_));
    Some(InvalidParameterException {
        cluster_name: (names_cluster && !requested_name.is_empty())
            .then(|| requested_name.to_string()),
        message: Some(message),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// RULE: a member this API requires and the request omitted earns EKS's
    /// required-parameter sentence, whichever member it is.
    #[test]
    fn an_omitted_required_member_earns_the_required_sentence() {
        for member in ["name", "roleArn", "resourcesVpcConfig", "clusterName"] {
            let refused = refusal_message(&[violation(
                member,
                ViolationKind::MissingRequiredMember,
                None,
            )]);
            assert_eq!(
                refused.as_deref(),
                Some(state::parameter_required_message(member).as_str()),
                "case {member:?}: expected the required sentence, got {refused:?}"
            );
        }
    }

    /// RULE: a value outside the cluster-name grammar earns EKS's invalid-characters
    /// sentence naming the member it arrived in — and a pattern that is NOT that
    /// grammar earns the framework's report instead.
    #[test]
    fn a_value_outside_the_cluster_name_grammar_earns_the_characters_sentence() {
        for (member, value) in [("name", "probe cluster"), ("name", "-leading")] {
            let refused = refusal_message(&[violation(
                member,
                ViolationKind::Pattern(state::CLUSTER_NAME_PATTERN.to_string()),
                Some(value),
            )]);
            assert_eq!(
                refused.as_deref(),
                Some(state::parameter_invalid_characters_message(member).as_str()),
                "case {value:?}: expected the invalid-characters sentence, got {refused:?}"
            );
        }

        assert_eq!(
            refusal_message(&[violation(
                "configurationValues",
                ViolationKind::Pattern("^[0-9]+$".to_string()),
                Some("x"),
            )]),
            None,
            "a pattern that is not the cluster-name grammar keeps the framework's report"
        );
    }

    /// RULE: a page size outside the published bounds earns EKS's range sentence,
    /// whichever end it broke; a bound that is not the published one does not.
    #[test]
    fn a_page_size_outside_the_bounds_earns_the_range_sentence() {
        let expected = state::max_results_range_message();
        for (case, kind) in [
            (
                "below the minimum",
                ViolationKind::MinValue(state::MIN_LIST_MAX_RESULTS.to_string()),
            ),
            (
                "above the maximum",
                ViolationKind::MaxValue(state::MAX_LIST_MAX_RESULTS.to_string()),
            ),
        ] {
            let refused = refusal_message(&[violation("maxResults", kind, Some("101"))]);
            assert_eq!(
                refused.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected the range sentence, got {refused:?}"
            );
        }

        for (case, path, kind) in [
            (
                "another member's range",
                "diskSize",
                ViolationKind::MinValue("1".to_string()),
            ),
            (
                "a maximum this model does not publish",
                "maxResults",
                ViolationKind::MaxValue("1000".to_string()),
            ),
        ] {
            assert_eq!(
                refusal_message(&[violation(path, kind, Some("0"))]),
                None,
                "case {case:?}: expected the framework's report"
            );
        }
    }

    /// RULE: the catalog's support-status filter earns EKS's own sentence naming the
    /// rejected value; so does a control-plane log type, wherever it sits; another
    /// enum member earns the sentence naming it and the values it accepts.
    #[test]
    fn an_out_of_set_status_filter_earns_the_status_sentence() {
        // The walk states the value set in the MODEL's order, which is not the order
        // the generated constant holds; both are the same set.
        let declared: Vec<String> = ["unsupported", "standard-support", "extended-support"]
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        for value in ["standard_support", "STANDARD-SUPPORT", "", "bogus"] {
            let refused = refusal_message(&[violation(
                "status",
                ViolationKind::EnumValue(declared.clone()),
                Some(value),
            )]);
            assert_eq!(
                refused.as_deref(),
                Some(state::cluster_version_status_message(value).as_str()),
                "case {value:?}: expected the status sentence, got {refused:?}"
            );
        }

        // A log type is judged inside two nested lists, so its sentence has to hold
        // for a position the member-named sentences cannot name.
        let log_types: Vec<String> = LOG_TYPE_VALUES
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        for value in ["API", "AUDIT", "nothing"] {
            let refused = refusal_message(&[violation(
                "logging.clusterLogging.1.member.types.1.member",
                ViolationKind::EnumValue(log_types.clone()),
                Some(value),
            )]);
            assert_eq!(
                refused.as_deref(),
                Some(state::unsupported_log_type_message(value).as_str()),
                "case {value:?}: expected the log-type sentence, got {refused:?}"
            );
        }

        assert_eq!(
            refusal_message(&[violation(
                "authenticationMode",
                ViolationKind::EnumValue(
                    ["API", "API_AND_CONFIG_MAP", "CONFIG_MAP"]
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect()
                ),
                Some("api"),
            )])
            .as_deref(),
            Some("authenticationMode must be one of [API, API_AND_CONFIG_MAP, CONFIG_MAP]"),
            "another enum member earns the sentence naming it and the values it accepts"
        );

        // A value INSIDE a collection, judged against an enum this API states no
        // sentence of its own for, keeps the framework's report: these sentences name
        // one parameter and cannot say which element of a list it was.
        assert_eq!(
            refusal_message(&[violation(
                "filter.categories.1.member",
                ViolationKind::EnumValue(vec!["UPGRADE_READINESS".to_string()]),
                Some("COST_OPTIMIZATION"),
            )]),
            None,
            "a violation inside a collection keeps the framework's report"
        );
    }

    /// RULE: a `CreateCluster` refusal carries the cluster name the request asked for
    /// beside the sentence — and names none when the request named none.
    #[test]
    fn a_create_refusal_names_the_requested_cluster() {
        let violations = vec![violation(
            "name",
            ViolationKind::Pattern(state::CLUSTER_NAME_PATTERN.to_string()),
            Some("probe cluster"),
        )];
        let refusal = create_cluster_refusal(&violations, "probe cluster")
            .expect("a pattern violation is worded");
        assert_eq!(
            refusal.cluster_name.as_deref(),
            Some("probe cluster"),
            "expected the requested name, got {:?}",
            refusal.cluster_name
        );
        assert_eq!(
            refusal.message.as_deref(),
            Some(state::parameter_invalid_characters_message("name").as_str()),
            "expected the invalid-characters sentence, got {:?}",
            refusal.message
        );

        let omitted = vec![violation(
            "name",
            ViolationKind::MissingRequiredMember,
            None,
        )];
        let refusal =
            create_cluster_refusal(&omitted, "").expect("an omitted required member is worded");
        assert_eq!(
            refusal.cluster_name, None,
            "a request that named no cluster names none in its refusal, got {:?}",
            refusal.cluster_name
        );
        assert_eq!(
            refusal.message.as_deref(),
            Some(state::parameter_required_message("name").as_str()),
            "expected the required sentence, got {:?}",
            refusal.message
        );

        assert!(
            create_cluster_refusal(
                &[violation(
                    "tags",
                    ViolationKind::MaxLength(50),
                    Some("{}"),
                )],
                "probe",
            )
            .is_none(),
            "a rule this API states no sentence for keeps the framework's report"
        );

        // The value-set sentence names the member it is about and no cluster.
        let value_set = create_cluster_refusal(
            &[violation(
                "authenticationMode",
                ViolationKind::EnumValue(vec!["API".to_string()]),
                Some("api"),
            )],
            "probe",
        )
        .expect("a value outside an enum's set is worded");
        assert_eq!(
            value_set.cluster_name, None,
            "the value-set refusal names no cluster, got {:?}",
            value_set.cluster_name
        );
    }

    /// RULE: a cluster-name member past its published maximum earns EKS's length
    /// sentence, naming the member and the maximum the model states for it.
    #[test]
    fn a_cluster_name_past_its_maximum_earns_the_length_sentence() {
        for member in CLUSTER_NAME_MEMBERS {
            let refused = refusal_message(&[violation(
                member,
                ViolationKind::MaxLength(state::MAX_CLUSTER_NAME_LEN as i64),
                Some("c".repeat(state::MAX_CLUSTER_NAME_LEN + 1).as_str()),
            )]);
            assert_eq!(
                refused.as_deref(),
                Some(
                    state::parameter_too_long_message(
                        member,
                        state::MAX_CLUSTER_NAME_LEN as i64
                    )
                    .as_str()
                ),
                "case {member:?}: expected the length sentence, got {refused:?}"
            );
        }

        assert_eq!(
            refusal_message(&[violation(
                "tags",
                ViolationKind::MaxLength(50),
                Some("{}"),
            )]),
            None,
            "a length maximum on a member that carries no cluster name keeps the \
             framework's report"
        );
    }
}
