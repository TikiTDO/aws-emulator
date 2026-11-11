// SPDX-License-Identifier: CC0-1.0

//! What an `Authorizer` EXTRACTS from the request it is asked about: the values its
//! `identitySource` names.
//!
//! `Authorizer$identitySource` is "the identity source for which authorization is
//! requested" — a comma-separated list of mapping expressions, "the request header mapping
//! expression for the custom header holding the authorization token … For example,
//! `method.request.header.Auth`" — so WHICH parts of a request an authorizer is asked about
//! is the authorizer's own declaration rather than anything the caller chooses. Every source
//! it declares must be PRESENT: a request missing one carried no identity for the authorizer
//! to evaluate, so the authorizer is not invoked at all.
//!
//! Read in ONE place because the same declaration is read from both sides of this service —
//! a request a deployed stage answers ([`super::authorization_refusal`]) and a request a
//! caller SIMULATES ([`super::simulated_authorizer`]) — so the two cannot differ on which
//! sources an authorizer declares, on which value is the one it evaluates, or on what the
//! absence of one means. WHERE a value is read from is the caller's own business: the wire
//! for one, the members of a simulated request for the other, which is what the two lookups
//! passed in are for.

/// What the request carries for the sources `identitySource` declares.
#[derive(Debug, Default)]
pub(super) struct Presented {
    /// Every declared source's value, in the order the authorizer declares them — what a
    /// held result is keyed by, since two callers presenting two tokens are two identities
    /// and the same token twice is one.
    pub values: Vec<String>,
    /// The FIRST of them, which is the value an authorizer evaluating ONE credential is
    /// asked about: the bearer token a `TOKEN` authorizer receives, the identity token a
    /// user-pool authorizer verifies.
    pub first: Option<String>,
}

/// One source the authorizer declares that the request does not carry.
#[derive(Debug)]
pub(super) struct Absent {
    /// The mapping expression, exactly as the authorizer declares it, so a log can name
    /// which source was missing.
    pub source: String,
}

/// The values `identity_source` names, read through `header` and `query` — the two
/// request-bound `location`s a mapping expression takes — or the first source the request
/// does not carry.
///
/// A source bound to something else (a stage variable, say) is one this emulator cannot
/// resolve: it is neither satisfied nor missing, so it is left out rather than treated as
/// either.
pub(super) fn presented(
    identity_source: Option<&str>,
    header: impl Fn(&str) -> Option<String>,
    query: impl Fn(&str) -> Option<String>,
) -> Result<Presented, Absent> {
    let mut carried = Presented::default();
    for source in identity_source
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        let read = if let Some(name) = source.strip_prefix(super::METHOD_REQUEST_HEADER_PREFIX) {
            header(name)
        } else if let Some(name) = source.strip_prefix(super::METHOD_REQUEST_QUERY_PREFIX) {
            query(name)
        } else {
            continue;
        };
        // A source present but EMPTY carried no identity either: there is nothing for the
        // authorizer to evaluate, so it is the same absence as a source the request left out.
        let Some(value) = read.filter(|value| !value.is_empty()) else {
            return Err(Absent {
                source: source.to_string(),
            });
        };
        carried.first.get_or_insert_with(|| value.clone());
        carried.values.push(value);
    }
    Ok(carried)
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns: which values an authorizer's declaration extracts
    //! from a request, and which declaration a request fails to satisfy.

    use super::*;

    /// A lookup over a fixed set of `(name, value)` pairs, compared case-insensitively for a
    /// header and exactly for a query-string parameter.
    fn lookup(pairs: &[(&str, &str)], name: &str, fold_case: bool) -> Option<String> {
        pairs.iter().find_map(|(key, value)| {
            let matched = if fold_case {
                key.eq_ignore_ascii_case(name)
            } else {
                *key == name
            };
            matched.then(|| (*value).to_string())
        })
    }

    /// One row: the declaration, and the values it extracts from the request below —
    /// `None` for a declaration the request does not satisfy.
    struct Row {
        case: &'static str,
        declared: Option<&'static str>,
        extracted: Option<&'static [&'static str]>,
    }

    /// RULE: every source the authorizer declares must be present; the values are taken in
    /// declaration order, the first of them is the one credential; a source bound to neither
    /// location is left out.
    #[test]
    fn every_declared_source_must_be_present_and_the_first_is_the_credential() {
        let headers: &[(&str, &str)] = &[("Authorization", "tok"), ("X-Blank", "")];
        let query: &[(&str, &str)] = &[("access_token", "qtok")];
        // Rows differ along the declaration: one source, several, a missing one, an empty
        // one, a source bound to neither location, and no declaration at all.
        let rows: &[Row] = &[
            Row {
                case: "one header",
                declared: Some("method.request.header.Authorization"),
                extracted: Some(&["tok"]),
            },
            Row {
                case: "the same header in another case",
                declared: Some("method.request.header.authorization"),
                extracted: Some(&["tok"]),
            },
            Row {
                case: "a header and a query parameter",
                declared: Some(
                    "method.request.header.Authorization, \
                     method.request.querystring.access_token",
                ),
                extracted: Some(&["tok", "qtok"]),
            },
            Row {
                case: "a query parameter first",
                declared: Some(
                    "method.request.querystring.access_token,\
                     method.request.header.Authorization",
                ),
                extracted: Some(&["qtok", "tok"]),
            },
            Row {
                case: "a header the request does not carry",
                declared: Some("method.request.header.Missing"),
                extracted: None,
            },
            Row {
                case: "a header the request carries empty",
                declared: Some("method.request.header.X-Blank"),
                extracted: None,
            },
            Row {
                case: "a query parameter the request does not carry",
                declared: Some("method.request.querystring.absent"),
                extracted: None,
            },
            Row {
                case: "one present source beside one absent",
                declared: Some(
                    "method.request.header.Authorization,method.request.header.Missing",
                ),
                extracted: None,
            },
            Row {
                case: "a source bound to neither location",
                declared: Some("stageVariables.token"),
                extracted: Some(&[]),
            },
            Row {
                case: "no declaration at all",
                declared: None,
                extracted: Some(&[]),
            },
            Row {
                case: "an empty declaration",
                declared: Some(""),
                extracted: Some(&[]),
            },
        ];
        for Row {
            case,
            declared,
            extracted: expected,
        } in rows
        {
            let read = presented(
                *declared,
                |name| lookup(headers, name, true),
                |name| lookup(query, name, false),
            );
            match (&read, expected) {
                (Ok(carried), Some(values)) => {
                    assert_eq!(
                        carried.values,
                        values.iter().map(|value| (*value).to_string()).collect::<Vec<String>>(),
                        "case {case:?}: expected {values:?}, got {carried:?}"
                    );
                    assert_eq!(
                        carried.first.as_deref(),
                        values.first().copied(),
                        "case {case:?}: expected the first value {:?}, got {carried:?}",
                        values.first()
                    );
                }
                (Err(_absent), None) => {}
                (read, _) => panic!("case {case:?}: expected {expected:?}, got {read:?}"),
            }
        }
    }
}
