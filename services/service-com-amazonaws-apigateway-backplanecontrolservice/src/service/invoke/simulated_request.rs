// SPDX-License-Identifier: CC0-1.0

//! What a SIMULATED request carries, read the one way.
//!
//! The two `TestInvoke*` operations describe a request with members rather than sending one:
//! "a key-value map of headers to simulate an incoming invocation request" (`headers`), "the
//! headers as a map from string to list of values" (`multiValueHeaders`) and "the URI path,
//! including query string, of the simulated invocation request" (`pathWithQueryString`). Both
//! operations carry the SAME members and both hand what they read to the same machinery — the
//! mapping-template inputs, a proxy or authorizer event, an authorizer's `identitySource` —
//! so WHICH value a simulated request carries for a name is read here, once, and neither
//! operation can read the members it shares with the other differently.
//!
//! Every reader takes the maps themselves rather than an operation's input struct: the two
//! inputs are different shapes carrying the same two maps, and this module is about the maps.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The headers one simulated request carries, across both members a caller may carry one in.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SimulatedHeaders<'i> {
    /// `headers`, the single-valued member.
    single: Option<&'i HashMap<String, String>>,
    /// `multiValueHeaders`, the multi-valued one.
    multi: Option<&'i HashMap<String, Vec<String>>>,
}

impl<'i> SimulatedHeaders<'i> {
    /// The headers a request simulated with these two members carries.
    pub(crate) fn of(
        single: Option<&'i HashMap<String, String>>,
        multi: Option<&'i HashMap<String, Vec<String>>>,
    ) -> Self {
        Self { single, multi }
    }

    /// The value the request carries for `name`, looked up case-insensitively (a header name
    /// is case-insensitive on the wire, and these are supplied as map keys) across both
    /// members: `headers` first, then the FIRST value of the `multiValueHeaders` entry, which
    /// is the value a single-valued read of a multi-valued header sees.
    pub(crate) fn value(&self, name: &str) -> Option<String> {
        let single = self
            .single
            .iter()
            .flat_map(|carried| carried.iter())
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then(|| value.clone()));
        single.or_else(|| {
            self.multi
                .iter()
                .flat_map(|carried| carried.iter())
                .find_map(|(key, values)| {
                    key.eq_ignore_ascii_case(name)
                        .then(|| values.first().cloned())
                        .flatten()
                })
        })
    }

    /// The names the request carries, in name order.
    ///
    /// The NAMES, not the values: a simulated header is where an authorization token or an
    /// API key would travel, and an execution log is not where a caller's credential belongs.
    pub(crate) fn names(&self) -> Vec<String> {
        let mut names: BTreeSet<&str> = self
            .single
            .iter()
            .flat_map(|carried| carried.keys())
            .map(String::as_str)
            .collect();
        names.extend(
            self.multi
                .iter()
                .flat_map(|carried| carried.keys())
                .map(String::as_str),
        );
        names.into_iter().map(str::to_owned).collect()
    }

    /// The headers by name, single-valued, as [`Self::value`] reads one — `headers`, then the
    /// FIRST value of a `multiValueHeaders` entry. A name in both keeps the single-valued one.
    ///
    /// Read for the reason the template-selecting header is: a header the caller simulated
    /// must be the one a template renders against and the one an authorizer's identity source
    /// is read from, whichever member carried it.
    pub(crate) fn single_valued(&self) -> BTreeMap<String, String> {
        let mut carried: BTreeMap<String, String> = BTreeMap::new();
        for (name, values) in self.multi.iter().flat_map(|carried| carried.iter()) {
            if let Some(first) = values.first() {
                carried.insert(name.clone(), first.clone());
            }
        }
        for (name, value) in self.single.iter().flat_map(|carried| carried.iter()) {
            carried.insert(name.clone(), value.clone());
        }
        carried
    }

    /// EVERY value the request carries under each name: the `multiValueHeaders` entries the
    /// caller supplied, and the single-valued `headers` for a name only that member carries.
    ///
    /// The multi-valued view of [`Self::single_valued`], which is what a proxy integration's
    /// or a `REQUEST` authorizer's event carries beside the single-valued one.
    pub(crate) fn every_value(&self) -> BTreeMap<String, Vec<String>> {
        let mut carried: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, values) in self.multi.iter().flat_map(|carried| carried.iter()) {
            carried.insert(name.clone(), values.clone());
        }
        for (name, value) in self.single.iter().flat_map(|carried| carried.iter()) {
            carried
                .entry(name.clone())
                .or_insert_with(|| vec![value.clone()]);
        }
        carried
    }
}

/// The path and the query string of `pathWithQueryString` — "the URI path, including query
/// string, of the simulated invocation request" — split at the first `?`. Each half is absent
/// when the member carries none of it.
pub(crate) fn split_query(path_with_query: Option<&str>) -> (Option<&str>, Option<&str>) {
    let Some(value) = path_with_query.filter(|value| !value.is_empty()) else {
        return (None, None);
    };
    match value.split_once('?') {
        Some((path, query)) => (
            (!path.is_empty()).then_some(path),
            (!query.is_empty()).then_some(query),
        ),
        None => (Some(value), None),
    }
}

/// EVERY value the simulated query string carries under each name, read by the one reader
/// this service reads a query string with ([`super::sent_query_parameters`]).
pub(crate) fn every_query_parameter(query: Option<&str>) -> BTreeMap<String, Vec<String>> {
    let mut carried: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in super::sent_query_parameters(query.unwrap_or_default()) {
        carried.insert(name, vec![value]);
    }
    carried
}

/// The names of the entries of one map a caller simulates a request with, in name order:
/// the stage variables, and the additional context variables a simulated authorizer
/// execution carries.
///
/// The NAMES, not the values, for the reason [`SimulatedHeaders::names`] gives.
pub(crate) fn map_names(carried: Option<&HashMap<String, String>>) -> Vec<String> {
    carried
        .into_iter()
        .flatten()
        .map(|(name, _)| name.as_str())
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns: which value a simulated request carries for a name
    //! when both members carry one, and how `pathWithQueryString` splits.

    use super::*;

    /// RULE: `headers` wins over `multiValueHeaders` for a single-valued read, a multi-valued
    /// entry reads as its FIRST value, the name is matched case-insensitively, and every value
    /// of every name is reported in the multi-valued view.
    #[test]
    fn a_simulated_request_carries_both_members_of_a_header() {
        let single = HashMap::from([
            ("Accept".to_string(), "text/plain".to_string()),
            ("X-Only-Single".to_string(), "one".to_string()),
        ]);
        let multi = HashMap::from([
            (
                "Accept".to_string(),
                vec!["application/json".to_string(), "text/csv".to_string()],
            ),
            (
                "X-Only-Multi".to_string(),
                vec!["first".to_string(), "second".to_string()],
            ),
        ]);
        let headers = SimulatedHeaders::of(Some(&single), Some(&multi));
        // Rows differ along which member carries the name, and along the case it is read in.
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("both members", "Accept", Some("text/plain")),
            ("the single-valued member only", "X-Only-Single", Some("one")),
            ("the multi-valued member only", "X-Only-Multi", Some("first")),
            ("another case of a carried name", "x-only-multi", Some("first")),
            ("a name neither member carries", "X-Absent", None),
        ];
        for (case, name, expected) in rows {
            let read = headers.value(name);
            assert_eq!(
                read.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?} for {name:?}, got {read:?}"
            );
        }
        assert_eq!(
            headers.names(),
            vec![
                "Accept".to_string(),
                "X-Only-Multi".to_string(),
                "X-Only-Single".to_string(),
            ],
            "case \"the names carried\": got {:?}",
            headers.names()
        );
        assert_eq!(
            headers.every_value(),
            BTreeMap::from([
                (
                    "Accept".to_string(),
                    vec!["application/json".to_string(), "text/csv".to_string()]
                ),
                ("X-Only-Single".to_string(), vec!["one".to_string()]),
                (
                    "X-Only-Multi".to_string(),
                    vec!["first".to_string(), "second".to_string()]
                ),
            ]),
            "case \"every value carried\": got {:?}",
            headers.every_value()
        );
        let absent = SimulatedHeaders::of(None, None);
        assert!(
            absent.value("Accept").is_none() && absent.names().is_empty(),
            "case \"a request carrying neither member\": expected nothing carried, got {:?}",
            absent.single_valued()
        );
    }

    /// One row: what `pathWithQueryString` carries, and the two halves it splits into.
    struct Split {
        case: &'static str,
        carried: Option<&'static str>,
        path: Option<&'static str>,
        query: Option<&'static str>,
    }

    /// RULE: `pathWithQueryString` is the path and the query string, split at the first `?`,
    /// each half absent where the member carries none of it.
    #[test]
    fn the_simulated_path_and_query_string_split_at_the_first_question_mark() {
        // Rows differ along which halves the member carries.
        let rows: &[Split] = &[
            Split {
                case: "both halves",
                carried: Some("/orders?a=1&b=2"),
                path: Some("/orders"),
                query: Some("a=1&b=2"),
            },
            Split {
                case: "a path only",
                carried: Some("/orders"),
                path: Some("/orders"),
                query: None,
            },
            Split {
                case: "a path and an empty query",
                carried: Some("/orders?"),
                path: Some("/orders"),
                query: None,
            },
            Split {
                case: "a query only",
                carried: Some("?a=1"),
                path: None,
                query: Some("a=1"),
            },
            Split {
                case: "a query carrying a question mark",
                carried: Some("/o?a=1?2"),
                path: Some("/o"),
                query: Some("a=1?2"),
            },
            Split {
                case: "an empty member",
                carried: Some(""),
                path: None,
                query: None,
            },
            Split {
                case: "no member at all",
                carried: None,
                path: None,
                query: None,
            },
        ];
        for row in rows {
            let split = split_query(row.carried);
            assert_eq!(
                split,
                (row.path, row.query),
                "case {case:?}: expected {expected:?}, got {split:?}",
                case = row.case,
                expected = (row.path, row.query)
            );
        }
    }
}
