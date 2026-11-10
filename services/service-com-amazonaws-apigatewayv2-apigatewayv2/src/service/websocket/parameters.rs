// SPDX-License-Identifier: CC0-1.0

//! The DATA MAPPING a WEBSOCKET route's integration applies: what
//! `Integration$RequestParameters` puts on the request the backend sees.
//!
//! "For WebSocket APIs, a key-value map specifying request parameters that are passed from
//! the method request to the backend. The key is an integration request parameter name and
//! the associated value is a method request parameter value or static value that must be
//! enclosed within single quotes and pre-encoded as required by the backend. The method
//! request parameter value must match the pattern of `method.request.{location}.{name}`,
//! where `{location}` is querystring, path, or header"
//! (`Integration$RequestParameters`). So one entry is a DESTINATION and a SOURCE:
//!
//! * the KEY names where the value travels on the integration request and under what name,
//!   written `integration.request.{location}.{name}` over the same three locations;
//! * the VALUE is either a request parameter of the CALL — read under the two spellings API
//!   Gateway accepts for one expression, the `method.request.…` the member documents and the
//!   `route.request.…` a WebSocket api's own configuration and `Authorizer$IdentitySource`
//!   write (`crate::service::data_plane::HEADER_IDENTITY_SOURCES` names the same pair) — or a
//!   STATIC value, "enclosed within single quotes".
//!
//! What it maps ONTO is the connection as the event states it ([`super::event::Connection`]):
//! a header destination is a header the backend reads, a query-string destination a query
//! parameter it reads. The destination's NAME travels as the integration wrote it, because
//! that is the name the backend is told; a SOURCE is looked up case-insensitively, the way
//! every reader of a field name in this data plane looks one up.
//!
//! An entry whose source the call carries NOTHING for maps nothing at all: the destination
//! is left off the integration request rather than carried as an empty value, which is what
//! lets a backend tell a parameter the client omitted from one it sent empty. An entry
//! written in neither form names no source this emulator reads, so it likewise maps nothing
//! and says so in the log.
//!
//! Nothing here touches the store, the clock or the socket: what one call's parameters map to
//! is a pure function of the entry and of what the client sent.

use std::collections::BTreeMap;

use super::event::{field, Connection};

/// Where one entry's value travels on the integration request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappedInto {
    /// A header of the integration request: `integration.request.header.{name}`.
    Header,
    /// A query-string parameter of it: `integration.request.querystring.{name}`.
    QueryString,
    /// A path parameter of it: `integration.request.path.{name}`. A WebSocket call binds no
    /// path parameter of its own, so such an entry reads a source and carries it nowhere the
    /// event reports.
    Path,
}

/// The `integration.request.{location}.{name}` prefixes a destination is written with, each
/// paired with the location it names.
const DESTINATION_PREFIXES: [(&str, MappedInto); 3] = [
    ("integration.request.header.", MappedInto::Header),
    ("integration.request.querystring.", MappedInto::QueryString),
    ("integration.request.path.", MappedInto::Path),
];

/// The two spellings of a HEADER source: the `method.request.header.` the member documents
/// and the `route.request.header.` a WebSocket api's own configuration writes.
const HEADER_SOURCE_PREFIXES: [&str; 2] = ["method.request.header.", "route.request.header."];

/// The same pair for a QUERY-STRING source.
const QUERY_SOURCE_PREFIXES: [&str; 2] =
    ["method.request.querystring.", "route.request.querystring."];

/// `connection` with everything `request_parameters` maps onto the integration request.
///
/// Entries are read in key order, so what a map of several puts on the request does not
/// depend on the order they were written in; an entry naming a destination this data plane
/// does not read, and one whose source the call carries nothing for, map nothing.
pub fn mapped(connection: &Connection, request_parameters: &BTreeMap<String, String>) -> Connection {
    if request_parameters.is_empty() {
        return connection.clone();
    }
    let mut mapped = connection.clone();
    for (key, value) in request_parameters {
        let Some((location, name)) = destination(key) else {
            ::tracing::debug!(
                parameter = %key,
                "this integration's request parameter names no integration request \
                 parameter, so it maps nothing"
            );
            continue;
        };
        let Some(resolved) = source_value(connection, value) else {
            ::tracing::debug!(
                parameter = %key,
                "this integration's request parameter maps from something this call carries \
                 no value for, so it maps nothing"
            );
            continue;
        };
        match location {
            MappedInto::Header => {
                // Under the name the integration wrote, replacing any value the client sent
                // under that same field name (a field name is case-insensitive).
                mapped
                    .headers
                    .retain(|field, _| !field.eq_ignore_ascii_case(name));
                mapped.headers.insert(name.to_string(), resolved);
            }
            MappedInto::QueryString => {
                mapped.query_parameters.insert(name.to_string(), resolved);
            }
            // A WebSocket call has no path template behind its stage, so there is no path
            // parameter of the integration request for the event to report.
            MappedInto::Path => {
                ::tracing::debug!(
                    parameter = %key,
                    "a WebSocket call binds no path parameter, so this entry maps nothing"
                );
            }
        }
    }
    mapped
}

/// Where `key` says the value travels and under what name, or `None` for a key that names no
/// integration request parameter at all.
pub fn destination(key: &str) -> Option<(MappedInto, &str)> {
    let key = key.trim();
    DESTINATION_PREFIXES
        .into_iter()
        .find_map(|(prefix, location)| {
            strip_prefix_ignoring_case(key, prefix)
                .filter(|name| !name.is_empty())
                .map(|name| (location, name))
        })
}

/// The value `value` maps for this call, or `None` when it maps nothing.
///
/// The static form is the literal between the single quotes; a header and a query-string
/// source are read off what the client sent, under either documented spelling.
fn source_value(connection: &Connection, value: &str) -> Option<String> {
    let value = value.trim();
    if let Some(literal) = static_value(value) {
        return Some(literal.to_string());
    }
    if let Some(name) = source_name(value, &HEADER_SOURCE_PREFIXES) {
        return field(&connection.headers, name).map(str::to_string);
    }
    if let Some(name) = source_name(value, &QUERY_SOURCE_PREFIXES) {
        return connection.query_parameters.get(name).cloned();
    }
    None
}

/// The literal a STATIC value states, or `None` for a value that is not one: "a static value
/// that must be enclosed within single quotes".
fn static_value(value: &str) -> Option<&str> {
    value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
}

/// The `{name}` of a source expression written with any of `prefixes`, or `None` when the
/// value is written with none of them.
///
/// The NAMESPACE is matched case-insensitively, the way every other reader of a mapping
/// expression in this service matches it, while the `{name}` is kept exactly as written: it
/// is a parameter name, and only a header's is case-insensitive.
fn source_name<'v>(value: &'v str, prefixes: &[&str]) -> Option<&'v str> {
    prefixes
        .iter()
        .find_map(|prefix| strip_prefix_ignoring_case(value, prefix))
        .filter(|name| !name.is_empty())
}

/// `text` without `prefix`, comparing the prefix case-insensitively. `None` when `text` does
/// not begin with it.
fn strip_prefix_ignoring_case<'t>(text: &'t str, prefix: &str) -> Option<&'t str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| text.get(prefix.len()..))
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    /// The connection every row below maps from: one header, and one query parameter.
    fn connection() -> Connection {
        Connection {
            api_id: "abc1230000".to_string(),
            stage_name: "prod".to_string(),
            connection_id: "L0SM9cOFyjUCIhA=".to_string(),
            region: "eu-west-1".to_string(),
            connected_at: SystemTime::UNIX_EPOCH,
            headers: BTreeMap::from([
                ("X-Client-Trace".to_string(), "trace-7".to_string()),
                ("User-Agent".to_string(), "suite".to_string()),
            ]),
            query_parameters: BTreeMap::from([("token".to_string(), "t-9".to_string())]),
            stage_variables: BTreeMap::new(),
            authorizer_context: None,
            authorizer_principal: None,
        }
    }

    /// The entries of one integration, as a map.
    fn parameters(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn a_destination_names_the_location_and_the_name_it_is_written_with() {
        // The rows span the three locations the member names, the case of the namespace, a
        // key naming a location but no name, and keys that name no destination at all.
        let cases: &[(&str, Option<(MappedInto, &str)>)] = &[
            (
                "integration.request.header.X-Trace",
                Some((MappedInto::Header, "X-Trace")),
            ),
            (
                "integration.request.querystring.token",
                Some((MappedInto::QueryString, "token")),
            ),
            (
                "integration.request.path.slot",
                Some((MappedInto::Path, "slot")),
            ),
            (
                "Integration.Request.Header.X-Trace",
                Some((MappedInto::Header, "X-Trace")),
            ),
            ("integration.request.header.", None),
            ("integration.request.body", None),
            ("route.request.header.X-Trace", None),
            ("", None),
        ];
        for (key, expected) in cases {
            let actual = destination(key);
            assert_eq!(
                actual, *expected,
                "case {key:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn an_entrys_value_maps_what_the_call_carries_for_the_source_it_names() {
        let connection = connection();
        // The rows span what the value's reading generalises over: both documented
        // spellings of a source, each location a WebSocket call carries, a header named in
        // another case than the client sent it in, the static form, a source the call
        // carries nothing for, and a value written in no form this reader knows.
        let cases: &[(&str, Option<&str>)] = &[
            ("route.request.header.x-client-trace", Some("trace-7")),
            ("method.request.header.x-client-trace", Some("trace-7")),
            ("route.request.header.X-Client-Trace", Some("trace-7")),
            ("Route.Request.Header.x-client-trace", Some("trace-7")),
            ("route.request.querystring.token", Some("t-9")),
            ("method.request.querystring.token", Some("t-9")),
            ("'fixed-value'", Some("fixed-value")),
            ("''", Some("")),
            ("route.request.header.x-absent", None),
            ("route.request.querystring.absent", None),
            ("$request.header.x-client-trace", None),
            ("plain", None),
        ];
        for (value, expected) in cases {
            let actual = source_value(&connection, value);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {value:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_map_of_entries_maps_each_into_the_location_its_key_names() {
        let connection = connection();
        let mapped = mapped(
            &connection,
            &parameters(&[
                (
                    "integration.request.header.X-Forwarded-Trace",
                    "route.request.header.x-client-trace",
                ),
                (
                    "integration.request.querystring.passed",
                    "route.request.querystring.token",
                ),
                ("integration.request.header.X-Fixed", "'shelf'"),
                // Maps nothing: the call carries no such header.
                (
                    "integration.request.header.X-Absent",
                    "route.request.header.x-never-sent",
                ),
                // Maps nothing: no destination this reader knows.
                ("x-not-a-destination", "'v'"),
            ]),
        );
        // The rows span what the mapping put on the request: each destination, the entries
        // that mapped nothing, and the handshake's own fields, which travel on unchanged.
        let cases: &[(&str, Option<&str>)] = &[
            ("X-Forwarded-Trace", Some("trace-7")),
            ("X-Fixed", Some("shelf")),
            ("X-Absent", None),
            ("x-not-a-destination", None),
            ("X-Client-Trace", Some("trace-7")),
            ("User-Agent", Some("suite")),
        ];
        for (name, expected) in cases {
            let actual = field(&mapped.headers, name);
            assert_eq!(
                actual, *expected,
                "case {name:?}: expected the header {expected:?}, got {actual:?}"
            );
        }
        let passed = mapped.query_parameters.get("passed").map(String::as_str);
        assert_eq!(
            passed,
            Some("t-9"),
            "case {:?}: expected the mapped query parameter, got {passed:?}",
            "integration.request.querystring.passed"
        );
        let sent = mapped.query_parameters.get("token").map(String::as_str);
        assert_eq!(
            sent,
            Some("t-9"),
            "case {:?}: the handshake's own query parameter travels on, got {sent:?}",
            "token"
        );
    }

    #[test]
    fn a_mapping_onto_a_name_the_client_sent_replaces_that_value() {
        let connection = connection();
        // The rows span the spellings a destination can collide with what the client sent
        // under: the same name, and the same field name in another case.
        let cases: [&str; 2] = ["X-Client-Trace", "x-client-trace"];
        for name in cases {
            let mapped = mapped(
                &connection,
                &parameters(&[(
                    &format!("integration.request.header.{name}"),
                    "'overwritten'",
                )]),
            );
            let actual = field(&mapped.headers, "x-client-trace");
            assert_eq!(
                actual,
                Some("overwritten"),
                "case {name:?}: expected the mapped value to replace what the client sent, \
                 got {actual:?}"
            );
            let values = mapped
                .headers
                .keys()
                .filter(|field| field.eq_ignore_ascii_case("x-client-trace"))
                .count();
            assert_eq!(
                values, 1,
                "case {name:?}: the field must be carried once, got {values} entries"
            );
        }
    }

    #[test]
    fn an_integration_that_maps_nothing_leaves_the_call_as_it_arrived() {
        let connection = connection();
        let mapped = mapped(&connection, &BTreeMap::new());
        assert_eq!(
            mapped.headers, connection.headers,
            "case {:?}: expected the handshake's own fields, got {:?}",
            "an integration with no request parameters", mapped.headers
        );
        assert_eq!(
            mapped.query_parameters, connection.query_parameters,
            "case {:?}: expected the handshake's own query string, got {:?}",
            "an integration with no request parameters", mapped.query_parameters
        );
    }
}
