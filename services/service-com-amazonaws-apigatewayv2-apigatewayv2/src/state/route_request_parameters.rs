// SPDX-License-Identifier: CC0-1.0

//! Where a ROUTE may declare request parameters: the one route of a WEBSOCKET api that
//! accepts a `Route$RequestParameters` map.
//!
//! "The request parameters for the route. Supported only for WebSocket APIs"
//! (`Route$RequestParameters`) says which PROTOCOL may carry the member — the restriction
//! every protocol-restricted member of a route shares
//! ([`crate::state::validate_protocol_restricted_members`]) — and leaves open which route of
//! such an api may. API Gateway narrows it to one: the request parameters a route declares
//! are the parameters of the CONNECTION REQUEST, the only call a WebSocket api serves that
//! carries request parameters of its own (a message carries a payload and no request line),
//! so they are accepted only on the route that serves it. Every other route of the api is
//! refused `BadRequestException`.
//!
//! The one sentence that refusal is worded with is stated here, so the create and the update
//! of a route cannot word it differently; it has no variable parts.

/// The `RouteKey` of the route that serves the CONNECTION REQUEST: AWS documents `$connect`
/// as the route "API Gateway invokes when a client connects to your API", which is the call
/// a route's request parameters are the parameters of.
pub const CONNECT_ROUTE_KEY: &str = "$connect";

/// How API Gateway words the refusal of request parameters declared on any other route.
pub const CONNECT_ONLY_MESSAGE: &str =
    "Request parameters are only supported for the $connect route in WEBSOCKET APIs";

/// Whether a route of an api of `protocol_type` whose key is `route_key` may carry the
/// request parameters it declares.
///
/// `declares_parameters` is whether the route ends up carrying ANY: a map the request sent
/// empty declares nothing, so it is accepted on any route — a route carries no request
/// parameters either way, and refusing it would refuse a request that configured nothing.
/// An api of another protocol is left to the protocol restriction that owns the member.
pub fn validate_route_request_parameters(
    protocol_type: &str,
    route_key: &str,
    declares_parameters: bool,
) -> Result<(), &'static str> {
    if protocol_type != crate::state::WEBSOCKET_PROTOCOL_TYPE {
        return Ok(());
    }
    if !declares_parameters || route_key == CONNECT_ROUTE_KEY {
        return Ok(());
    }
    Err(CONNECT_ONLY_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_parameters_are_accepted_only_on_the_connection_requests_route() {
        // The rows span what the rule generalises over: the parent api's protocol, the
        // route key (the connection request's, each other reserved key, a key of the api's
        // own naming, and the empty key), and a route that declares none.
        let cases: &[(&str, &str, bool, bool)] = &[
            ("WEBSOCKET", "$connect", true, true),
            ("WEBSOCKET", "$disconnect", true, false),
            ("WEBSOCKET", "$default", true, false),
            ("WEBSOCKET", "sendmessage", true, false),
            ("WEBSOCKET", "", true, false),
            // A declaration of nothing configures nothing, on any key.
            ("WEBSOCKET", "$default", false, true),
            ("WEBSOCKET", "$connect", false, true),
            // Another protocol is the protocol restriction's to refuse, not this rule's.
            ("HTTP", "GET /orders", true, true),
            ("HTTP", "$connect", true, true),
        ];
        for (protocol_type, route_key, declares, expected) in cases {
            let validated =
                validate_route_request_parameters(protocol_type, route_key, *declares);
            assert_eq!(
                validated.is_ok(),
                *expected,
                "case ({protocol_type:?}, {route_key:?}, declares {declares}): expected \
                 accepted {expected}, got {validated:?}"
            );
            if !*expected {
                assert_eq!(
                    validated,
                    Err(CONNECT_ONLY_MESSAGE),
                    "case ({protocol_type:?}, {route_key:?}): expected the one refusal \
                     sentence, got {validated:?}"
                );
            }
        }
    }
}
