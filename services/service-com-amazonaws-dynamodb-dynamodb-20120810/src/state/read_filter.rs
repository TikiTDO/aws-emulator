// SPDX-License-Identifier: CC0-1.0

//! WHICH of the items a read evaluated it answers with: the filter one request states,
//! in either generation of the member that states it.
//!
//! "A filter expression is applied after a Query finishes, but before the results are
//! returned" (`Query$FilterExpression`), and `Scan$FilterExpression` says the same of a
//! scan. The legacy pair the expression replaced — `QueryFilter` / `ScanFilter` with
//! `ConditionalOperator` — states the same filter one generation back ("This is a legacy
//! parameter. Use FilterExpression instead": those members' docs), so both readers meet
//! the same three cases and decide them here ONCE rather than each on its own.
//!
//! A file of its own beside the item store rather than inside it: this is about what one
//! REQUEST narrows its page with, not about what the store holds. It is re-exported up to
//! `crate::state`, so every caller names it `crate::state::…`.

use std::collections::{BTreeSet, HashMap};

use crate::ops::types::Condition;
use crate::state::{
    check_legacy_conditions, expression_conflict, legacy_conditions_match, Item, ParsedCondition,
    RequestError, Substitutions,
};

/// The member a `FilterExpression` arrives in, which is what its refusals name.
const FILTER_EXPRESSION: &str = "FilterExpression";

/// The filter one read applies to the attributes each row it evaluated SAW.
#[derive(Debug)]
pub enum ReadFilter<'a> {
    /// No filter at all: every item the page evaluated is answered, so "Count is the
    /// same as ScannedCount".
    Unfiltered,
    /// A `FilterExpression`: the `ConditionExpression` language — one grammar, parsed by
    /// the one parser this crate has for it — judged against each item the read reached
    /// rather than against a key.
    Expression(ParsedCondition),
    /// The legacy filter map, its entries combined by `ConditionalOperator`.
    Legacy {
        conditions: &'a HashMap<String, Condition>,
        conditional_operator: Option<&'a str>,
    },
}

impl<'a> ReadFilter<'a> {
    /// The filter this request states, refusing a request that states it twice, states a
    /// comparison the service does not serve or with the operands that comparison does
    /// not take, or combines entries it does not have — all facts of the REQUEST, so all
    /// of them settled before anything is read.
    ///
    /// `legacy_member` is the member this reader's legacy filter arrives in
    /// (`QueryFilter` for a query, `ScanFilter` for a scan), which is what the refusals
    /// of a request that used it name.
    pub fn resolve(
        legacy_member: &'static str,
        expression: Option<&str>,
        legacy: Option<&'a HashMap<String, Condition>>,
        conditional_operator: Option<&'a str>,
        substitutions: Substitutions<'_>,
    ) -> Result<Self, RequestError> {
        let legacy = legacy.filter(|map| !map.is_empty());
        match (expression, legacy) {
            // One filter stated in both generations has already been refused
            // ([`crate::state::check_expression_parameter_mix`]); the arm stays total
            // rather than unreachable.
            (Some(_), Some(_)) => Err(expression_conflict(legacy_member, FILTER_EXPRESSION)),
            (Some(expression), None) => Ok(Self::Expression(ParsedCondition::parse(
                FILTER_EXPRESSION,
                expression,
                substitutions,
            )?)),
            (None, Some(conditions)) => {
                // How many entries the map states, whether each names a comparison this
                // service serves with the operands it takes, and whether the operator
                // combining them is one the model declares, are all facts of the
                // request: settled once here rather than per item, so the work the
                // filter does per row is bounded by this service's own constants.
                crate::state::check_conditional_operator_companion(
                    legacy_member,
                    conditional_operator,
                    conditions.len(),
                )?;
                // Every value the filter compares against is held to the documented
                // value rules, before anything is read against them.
                crate::state::check_legacy_condition_values(
                    legacy_member,
                    conditions.iter().flat_map(|(name, condition)| {
                        crate::state::legacy_operands(condition)
                            .iter()
                            .map(move |value| (name.as_str(), value))
                    }),
                )?;
                check_legacy_conditions(conditions, conditional_operator)?;
                Ok(Self::Legacy {
                    conditions,
                    conditional_operator,
                })
            }
            (None, None) => {
                // The member combines the entries of the legacy filter map, so a
                // request that sends it with no map states how to combine nothing:
                // the one sentence every surface carrying the member states.
                crate::state::check_conditional_operator_companion(
                    legacy_member,
                    conditional_operator,
                    0,
                )?;
                Ok(Self::Unfiltered)
            }
        }
    }

    /// Whether this filter answers the attributes one read saw.
    pub fn matches(&self, seen: &Item) -> Result<bool, RequestError> {
        match self {
            Self::Unfiltered => Ok(true),
            Self::Expression(condition) => condition.evaluate(Some(seen)),
            Self::Legacy {
                conditions,
                conditional_operator,
            } => legacy_conditions_match(conditions, *conditional_operator, seen),
        }
    }

    /// Every top-level attribute this filter READS, which is what a read served from an
    /// index must have in order to judge it — and what a key condition's own attributes
    /// are held against, since "a FilterExpression does not allow key attributes".
    pub fn attributes(&self) -> BTreeSet<String> {
        match self {
            Self::Unfiltered => BTreeSet::new(),
            Self::Expression(condition) => condition.attribute_reads().into_keys().collect(),
            Self::Legacy { conditions, .. } => conditions.keys().cloned().collect(),
        }
    }
}
