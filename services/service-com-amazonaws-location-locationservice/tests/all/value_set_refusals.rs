// SPDX-License-Identifier: CC0-1.0

//! The wording of a refusal about a member whose value has to come from a
//! documented set, over the wire, for every operation that makes such a check.
//!
//! Several members of this API are plain strings in the model rather than enums
//! (`CreateRouteCalculatorRequest$DataSource`, `MapConfiguration$Style`), so the
//! request boundary accepts any text and the service itself refuses a value outside
//! the set its documentation publishes. Every one of those refusals reads as one
//! sentence: the member name as the model spells it in single quotes, "must be one
//! of", the documented values separated by ", ", then a period — no brackets and no
//! echo of the value the request sent.
//!
//! The table below spans that class: one row per operation that makes the check,
//! and per row the shapes a rejected value takes (an unknown value, a case variant
//! of a documented one, a padded one, an empty one). Each row's sentence is both
//! compared literally and taken apart again, so a caller that re-reads the allowed
//! set out of the message recovers exactly the documented set.

use serde_json::{json, Value};

use crate::harness::{call, emulator, Answer};

/// The data providers `CreateRouteCalculator` publishes, in the order its
/// `DataSource` member doc lists them ("Valid values include: Esri … Grab … Here").
const DATA_SOURCES: [&str; 3] = ["Esri", "Grab", "Here"];

/// The map styles `CreateMap` publishes, in the order the `MapConfiguration$Style`
/// member doc lists them (`smithy_query(query="doc",shape="MapConfiguration",
/// member="Style")`): the Esri group, the HERE Technologies group, the GrabMaps
/// group, then the Open Data group.
const MAP_STYLES: [&str; 18] = [
    "VectorEsriDarkGrayCanvas",
    "RasterEsriImagery",
    "VectorEsriLightGrayCanvas",
    "VectorEsriTopographic",
    "VectorEsriStreets",
    "VectorEsriNavigation",
    "VectorHereContrast",
    "VectorHereBerlin",
    "VectorHereExplore",
    "VectorHereExploreTruck",
    "RasterHereExploreSatellite",
    "HybridHereExploreSatellite",
    "VectorGrabStandardLight",
    "VectorGrabStandardDark",
    "VectorOpenDataStandardLight",
    "VectorOpenDataStandardDark",
    "VectorOpenDataVisualizationLight",
    "VectorOpenDataVisualizationDark",
];

/// One member this service constrains to a documented value set, and how to send a
/// request that carries a chosen value for it.
struct ValueSet {
    /// The operation the check runs in, for the assertion messages.
    operation: &'static str,
    /// The operation's `http` URI.
    path: &'static str,
    /// The member the refusal must name, as the model spells it.
    member: &'static str,
    /// The documented set, in the order the documentation lists it.
    allowed: &'static [&'static str],
    /// A request body carrying `value` for that member.
    request: fn(name: &str, value: &str) -> Value,
    /// Values outside the documented set: an unknown one, case variants of a
    /// documented one, a padded one, two values in one, and an empty one.
    rejected: &'static [&'static str],
}

fn value_sets() -> Vec<ValueSet> {
    vec![
        ValueSet {
            operation: "CreateRouteCalculator",
            path: "/routes/v0/calculators",
            member: "DataSource",
            allowed: &DATA_SOURCES,
            request: |name, value| json!({ "CalculatorName": name, "DataSource": value }),
            // `DataSource` carries no `@length` and no pattern, so the request
            // boundary accepts every row below — including the empty value — and
            // each is refused by the value-set check alone.
            rejected: &["Bing", "HERE", "esri", " Grab", "Grab ", "Esri,Here", ""],
        },
        ValueSet {
            operation: "CreateMap",
            path: "/maps/v0/maps",
            member: "Style",
            allowed: &MAP_STYLES,
            request: |name, value| {
                json!({ "MapName": name, "Configuration": { "Style": value } })
            },
            // `Style` carries `@length(min: 1)` and a pattern, so the request
            // boundary refuses an empty or punctuated value before this check runs;
            // every row below satisfies those constraints and is refused by the
            // value-set check alone.
            rejected: &[
                "VectorEsriStreet",
                "VECTORESRISTREETS",
                "vectoresristreets",
                "VectorTomTomStreets",
                "VectorEsriStreets.1",
                "x",
            ],
        },
    ]
}

/// The sentence `member` carrying a value outside `allowed` must be refused with.
fn expected_sentence(member: &str, allowed: &[&str]) -> String {
    format!("'{member}' must be one of {}.", allowed.join(", "))
}

/// The members of the allowed set a caller recovers from `sentence`, or the reason
/// the sentence does not carry the set in the service's form.
fn parse_allowed(member: &str, sentence: &str) -> Result<Vec<String>, String> {
    let prefix = format!("'{member}' must be one of ");
    let rest = sentence
        .strip_prefix(prefix.as_str())
        .ok_or_else(|| format!("sentence does not start with {prefix:?}"))?;
    let listed = rest
        .strip_suffix('.')
        .ok_or_else(|| "sentence does not end with a period".to_string())?;
    if listed.contains('[') || listed.contains(']') {
        return Err("the allowed values are wrapped in brackets".to_string());
    }
    Ok(listed.split(", ").map(str::to_string).collect())
}

#[tokio::test]
async fn a_value_outside_a_documented_set_is_refused_with_the_services_sentence() {
    for set in value_sets() {
        let sentence = expected_sentence(set.member, set.allowed);
        for (index, value) in set.rejected.iter().enumerate() {
            let router = emulator();
            let name = format!("refused-{index}");
            let refused: Answer = call(
                &router,
                "POST",
                set.path,
                Some((set.request)(&name, value)),
            )
            .await;
            let case = format!("{} {:?}={value:?}", set.operation, set.member);

            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected a 400 refusal, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {case:?}: expected error code ValidationException, got {:?} ({:?})",
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                sentence,
                "case {case:?}: expected message {sentence:?}, got {:?}",
                refused.message()
            );

            // The sentence must name only the member and the documented set: the
            // value the request sent is never quoted back. Checked on the rows whose
            // value shares no text with the set, so the assertion cannot be
            // satisfied by the set's own listing.
            let shares_text = value.is_empty()
                || set
                    .allowed
                    .iter()
                    .any(|allowed| value.contains(allowed) || allowed.contains(*value));
            if !shares_text {
                assert!(
                    !refused.message().contains(value),
                    "case {case:?}: the refusal must not echo the rejected value, got {:?}",
                    refused.message()
                );
            }

            // The whole documented set, in documentation order, must be readable
            // back out of the message by splitting on ", ".
            let recovered = parse_allowed(set.member, &refused.message()).unwrap_or_else(|why| {
                panic!(
                    "case {case:?}: {why}: {:?}",
                    refused.message()
                )
            });
            assert_eq!(
                recovered,
                set.allowed
                    .iter()
                    .map(|allowed| (*allowed).to_string())
                    .collect::<Vec<_>>(),
                "case {case:?}: the refusal must list the documented set, got {recovered:?}"
            );

            // The refusal's `fieldList` entry repeats the same sentence against the
            // same member.
            let body = refused.json();
            assert_eq!(
                body.get("fieldList")
                    .and_then(Value::as_array)
                    .map(|entries| entries
                        .iter()
                        .map(|entry| (
                            entry.get("name").and_then(Value::as_str).unwrap_or_default(),
                            entry
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                        ))
                        .collect::<Vec<_>>()),
                Some(vec![(set.member, sentence.as_str())]),
                "case {case:?}: fieldList must name the member and repeat the sentence, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn every_value_in_a_documented_set_is_accepted_as_published() {
    // The complement of the refusal table: no published value is refused by the
    // check whose sentence lists it, so the set the message advertises is the set
    // the service really accepts.
    for set in value_sets() {
        for (index, value) in set.allowed.iter().enumerate() {
            let router = emulator();
            let name = format!("accepted-{index}");
            let created = call(
                &router,
                "POST",
                set.path,
                Some((set.request)(&name, value)),
            )
            .await;
            assert_eq!(
                created.status,
                200,
                "case {} {:?}={value:?}: a published value must be accepted, got {} {:?}",
                set.operation,
                set.member,
                created.status,
                created.body
            );
        }
    }
}
