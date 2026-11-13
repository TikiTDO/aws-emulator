// SPDX-License-Identifier: CC0-1.0

//! What a WEBSOCKET api's stage ANSWERS: which route serves a message, and what an
//! integration that reaches no backend replies with.
//!
//! A WebSocket api serves two kinds of call over one socket, and the route each is served by
//! is decided here from the configuration the stage's deployment froze:
//!
//! * the CONNECTION REQUEST, the opening handshake itself, served by the `$connect` route
//!   when the api has one ([`connection_request`]);
//! * each MESSAGE the client then sends, served by the route the api's
//!   `RouteSelectionExpression` selects ([`message_route`]).
//!
//! Everything in this module is a PURE function of the api's records and the bytes the
//! client sent: nothing here touches the socket, the store or the clock, so the rule each
//! answer follows can be read — and tested — without a connection. The BACKEND a route
//! fronts is performed by [`super::integration`], which holds the in-process transport; what
//! is decided here is what an integration answers without one, which is the `MOCK` type's
//! whole rule ([`mock_reply`]) and is also what a reading of the records can say about a
//! connection request before any socket exists ([`super::serve`]).
//!
//! # The rule a `MOCK` integration's reply follows
//!
//! A `MOCK` integration has no backend: "a MOCK integration lets API Gateway return a
//! response without sending the request further to the backend". Its reply is therefore
//! computed from the integration's own records, in the order API Gateway computes it:
//!
//! 1. the integration's `RequestTemplates` entry that keys the call is RENDERED, and the
//!    `statusCode` member of what it renders to is the status the call produced — that is
//!    what a `MOCK` request template is for ("`{"statusCode": 200}`");
//! 2. that status SELECTS one of the integration's `IntegrationResponses` by its
//!    `IntegrationResponseKey`: a key written as a regular expression between slashes
//!    (`/2\d\d/`) is selected when it matches the status, and `$default` is the fallback for
//!    a call no patterned key matches;
//! 3. the selected response's own `ResponseTemplates` entry is RENDERED, and the text it
//!    renders to is the payload the caller reads back.
//!
//! A call that selects NO integration response has no reply at all — API Gateway has no
//! mapping to answer from — which is a fault of the api's configuration rather than of the
//! caller: the connection request fails with `500`, and a message is answered with the
//! gateway's own `Internal server error` frame.

use std::collections::BTreeMap;

use crate::state::integration_response_key::{key_matches_status, DEFAULT_SELECTION_KEY};
use crate::state::{Api, DeployedConfiguration, Integration, IntegrationResponse, Route};

/// The `RouteKey` of the route that serves the CONNECTION REQUEST: AWS documents `$connect`
/// as the route "API Gateway invokes when a client connects to your API".
pub const CONNECT_ROUTE_KEY: &str = "$connect";

/// The `RouteKey` of the route that serves the close: "API Gateway invokes [it] when a
/// client disconnects from your API".
pub const DISCONNECT_ROUTE_KEY: &str = "$disconnect";

/// The `RouteKey` that serves every message no other route's key matches: "`$default`, which
/// API Gateway invokes if the route selection expression cannot be evaluated or if no
/// matching route is found".
pub const DEFAULT_ROUTE_KEY: &str = "$default";

/// The media type a mapping template is keyed by when it is keyed by one at all — the key
/// API Gateway's own console writes for a WebSocket api's templates, and the one a message
/// carrying JSON selects.
pub const JSON_TEMPLATE_KEY: &str = "application/json";

/// The `IntegrationType` whose reply is computed from the integration's own records; see the
/// module docs.
pub const MOCK_INTEGRATION_TYPE: &str = "MOCK";

/// What a route's integration replied with: the status the call produced and the payload the
/// selected integration response rendered, if it rendered one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The status the call produced — for a `MOCK`, the `statusCode` its rendered request
    /// template named — or `None` when the call produced none.
    pub status_code: Option<String>,
    /// The `IntegrationResponseKey` of the response the status selected.
    pub selected_key: String,
    /// The payload the selected response's `ResponseTemplates` rendered, or `None` when it
    /// carries no template that keys the call (a reply with no payload).
    pub payload: Option<String>,
}

/// Why a route's integration produced no reply at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unserved {
    /// The route names no integration of this api, or names none at all, so there is
    /// nothing to call.
    NoIntegration,
    /// No `IntegrationResponse` of the integration is selected by the status the call
    /// produced, so API Gateway has no mapping to answer from.
    NoSelectableResponse,
    /// The integration is of a type this emulator does not perform here, or carries a
    /// template it does not render; the string says which, for the log and for the
    /// developer.
    Unperformable(String),
}

/// The route of the deployed configuration whose `RouteKey` is `route_key`, or `None` when
/// the deployment holds none.
pub fn deployed_route<'d>(
    deployed: &'d DeployedConfiguration,
    route_key: &str,
) -> Option<&'d Route> {
    deployed
        .routes
        .values()
        .find(|route| route.route_key == route_key)
}

/// The integration `route` targets, read from the SAME deployment the route came from, so a
/// stage pinned to an older deployment calls the integration as it stood then.
pub fn targeted_integration<'d>(
    deployed: &'d DeployedConfiguration,
    route: &Route,
) -> Option<&'d Integration> {
    let integration_id = route
        .target
        .as_deref()
        .and_then(crate::state::target_integration_id)?;
    deployed.integrations.get(integration_id)
}

/// What the CONNECTION REQUEST is answered with, from the configuration the stage deployed,
/// WITHOUT reaching a backend.
///
/// The handshake completes only when the route that serves the connection request produced a
/// SUCCESSFUL reply: the `$connect` route's integration is called before the socket exists,
/// and its reply is what admits the connection. So:
///
/// * an api with NO `$connect` route admits the connection — there is no route to refuse it;
/// * a `$connect` route whose integration replies with a `2xx` status admits it;
/// * a `$connect` route whose integration produces no reply, or a reply outside `2xx`, fails
///   the handshake.
///
/// A `MOCK` integration produces NO connection decision at all: it answers from the
/// integration's own records without reaching a backend, and a connection request is
/// precisely a call whose answer the backend has to give (which is why `$connect` is the
/// route an authorizer and a handler admit or refuse a client at). A `$connect` route bound
/// to one therefore fails the handshake, which is the recorded behaviour of API Gateway for
/// that configuration — the client reads `500` and no connection is established.
///
/// A `$connect` route fronting a BACKEND is decided by that backend's own reply, which this
/// reading cannot obtain: a caller with no in-process transport reads `Unserved` for it,
/// while the api's own socket performs the integration ([`super::integration`]) and is
/// admitted by the status the function answered.
pub fn connection_request(deployed: &DeployedConfiguration) -> Result<Option<Reply>, Unserved> {
    let Some(route) = deployed_route(deployed, CONNECT_ROUTE_KEY) else {
        return Ok(None);
    };
    if targeted_integration(deployed, route)
        .is_some_and(|integration| integration.integration_type == MOCK_INTEGRATION_TYPE)
    {
        return Err(no_connection_decision());
    }
    let reply = integration_reply(deployed, route, None)?;
    Ok(Some(reply))
}

/// Why a `MOCK` integration gives the CONNECTION REQUEST no decision at all.
///
/// A connection request is precisely a call whose answer the BACKEND has to give — which is
/// why `$connect` is the route an authorizer and a handler admit or refuse a client at — and
/// a `MOCK` integration "lets API Gateway return a response without sending the request
/// further to the backend". A `$connect` route bound to one therefore fails the handshake,
/// whatever status its templates render, which is the recorded behaviour of API Gateway for
/// that configuration: the client reads `500` and no connection is established.
///
/// Stated here, as the one owner of the rule, because both the reading of the records and
/// the performing of the integration apply it ([`super::integration::perform`]).
pub fn no_connection_decision() -> Unserved {
    Unserved::Unperformable(format!(
        "a {MOCK_INTEGRATION_TYPE} integration answers from the integration's own records \
         and reaches no backend, so it gives the connection request no decision"
    ))
}

/// Whether a status a reply carries is a SUCCESS — the `2xx` class, which is what admits a
/// connection request.
pub fn is_success_status(status: Option<&str>) -> bool {
    status
        .and_then(|status| status.trim().parse::<u16>().ok())
        .is_some_and(|status| (200..300).contains(&status))
}

/// The route key the api's `RouteSelectionExpression` selects for `body`, or `None` when the
/// expression selects nothing in this message.
///
/// A WEBSOCKET api's expression names a FIELD OF THE MESSAGE BODY
/// (`crate::state::WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX`, the rule every writer of the
/// member is held to), so the key is that field's value: `$request.body.action` selects the
/// value of `action`. A message that is not a JSON object, or that carries no such field,
/// selects nothing — which is the "route selection expression cannot be evaluated" case
/// `$default` exists for.
pub fn selected_route_key(api: &Api, body: &str) -> Option<String> {
    let reference = crate::state::selection_expression_references(&api.route_selection_expression)
        .into_iter()
        .next()?;
    // The NAMESPACE is read case-insensitively, the way the writer of the member reads it,
    // and the FIELD is read exactly as written, because a JSON member name is
    // case-sensitive. Both halves are taken fallibly, so an expression whose bytes do not
    // split there names no field rather than panicking on a caller's value.
    let prefix = crate::state::WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX;
    let namespace = reference.get(..prefix.len())?;
    let field = reference.get(prefix.len()..).filter(|field| !field.is_empty())?;
    if !namespace.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let document: ::serde_json::Value = ::serde_json::from_str(body).ok()?;
    let value = document.get(field)?;
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// The route of this deployment that serves the message `body`: the one whose `RouteKey` the
/// api's `RouteSelectionExpression` selected, then the `$default` route, and `None` when the
/// api holds neither — a message it routes nowhere.
///
/// The ONE selection both callers make, so what a reading of the records says a message is
/// routed to and what the socket actually invokes cannot disagree.
pub fn message_route<'d>(
    api: &Api,
    deployed: &'d DeployedConfiguration,
    body: &str,
) -> Option<&'d Route> {
    selected_route_key(api, body)
        .as_deref()
        .and_then(|key| deployed_route(deployed, key))
        .or_else(|| deployed_route(deployed, DEFAULT_ROUTE_KEY))
}

/// Whether `route` is TWO-WAY: "To define a two-way route, create a route response" — a
/// route with none gets no reply sent back to it however its integration replied.
pub fn is_two_way(route: &Route) -> bool {
    !route.route_responses.is_empty()
}

/// The call a mapping template is rendered against, when there is one to render against: a
/// reading of the api's records that has no connection in hand renders none.
///
/// It carries the WHOLE context of the call rather than one member of it: a template reads
/// `$context.<member>` for any member the gateway reports about the call it is rendered for
/// ([`super::event::template_context`] is the one owner of that set), `$stageVariables.<name>`
/// for the serving stage's own variables, and `$input` over the frame's payload — which only a
/// call that CARRIES a frame has, so the body is optional the way a close's is.
#[derive(Debug, Clone, Copy)]
pub struct TemplateCall<'c> {
    context: &'c BTreeMap<String, String>,
    stage_variables: &'c BTreeMap<String, String>,
    body: Option<&'c str>,
}

impl<'c> TemplateCall<'c> {
    /// One call of a connection, as a template reads it: its `$context` values, the serving
    /// stage's variables, and the frame it carries when it carries one.
    pub fn new(
        context: &'c BTreeMap<String, String>,
        stage_variables: &'c BTreeMap<String, String>,
        body: Option<&'c str>,
    ) -> Self {
        Self {
            context,
            stage_variables,
            body,
        }
    }
}

/// The reply `route`'s integration produces for this call WITHOUT reaching a backend; see
/// the module docs for the rule a `MOCK` integration's follows.
///
/// Every other integration type is answered by a backend, which this reading does not call:
/// it is reported as unperformable, and the caller that HAS the in-process transport
/// performs it instead ([`super::integration`]).
fn integration_reply(
    deployed: &DeployedConfiguration,
    route: &Route,
    call: Option<TemplateCall<'_>>,
) -> Result<Reply, Unserved> {
    let integration = targeted_integration(deployed, route).ok_or(Unserved::NoIntegration)?;
    if integration.integration_type != MOCK_INTEGRATION_TYPE {
        // Every other integration type REACHES a backend — a function to invoke, an HTTP
        // endpoint to call, an AWS service action to perform — which is not this reading's
        // to decide: it says so rather than answering as if the backend had replied.
        return Err(Unserved::Unperformable(format!(
            "an integration of type {} is performed by its backend, which this reading of \
             the api's records does not call",
            integration.integration_type
        )));
    }
    mock_reply(integration, call)
}

/// The reply a `MOCK` integration computes for this call, from the integration's own
/// records: the rule the module docs state, in the order API Gateway computes it.
pub fn mock_reply(
    integration: &Integration,
    call: Option<TemplateCall<'_>>,
) -> Result<Reply, Unserved> {
    let status = match keyed_template(&integration.request_templates) {
        Some(template) => {
            let rendered = rendered_template(template, call)?;
            status_code_of(&rendered)
        }
        None => None,
    };
    let selected = selected_integration_response(integration, status.as_deref())
        .ok_or(Unserved::NoSelectableResponse)?;
    let payload = match keyed_template(&selected.response_templates) {
        Some(template) => Some(rendered_template(template, call)?),
        None => None,
    };
    Ok(Reply {
        status_code: status,
        selected_key: selected.integration_response_key.clone(),
        payload,
    })
}

/// The template of `templates` that keys this call: the `$default` entry, then the JSON one,
/// then the single entry a map that holds exactly one carries.
///
/// A WebSocket message carries no `Content-Type` of its own, so what keys the call is the
/// map's own fallback key rather than a media type the client named; a map holding one
/// template under some other key is still that integration's one template, so it is the one
/// rendered.
fn keyed_template(templates: &BTreeMap<String, String>) -> Option<&str> {
    if let Some(keyed) = templates
        .get(DEFAULT_SELECTION_KEY)
        .or_else(|| templates.get(JSON_TEMPLATE_KEY))
    {
        return Some(keyed.as_str());
    }
    if templates.len() == 1 {
        return templates.values().next().map(String::as_str);
    }
    None
}

/// `template` rendered against this call, or why this emulator does not render it.
///
/// A mapping template is "a map of Velocity templates" (`Integration$RequestTemplates`), and
/// this emulator renders one in ONE place: the renderer the REST data plane of API Gateway
/// uses, which resolves the closed set of references API Gateway documents a template
/// against and refuses a template carrying a DIRECTIVE — a statement that would have to be
/// EXECUTED — rather than passing it through as if it were a literal body
/// (`service_com_amazonaws_apigateway_backplanecontrolservice::service::mapping_template`).
/// What this function owns is the BINDING of that renderer's inputs to a WebSocket call: the
/// frame is the payload `$input.body`, `$input.json(…)` and `$input.path(…)` read, and the
/// `$context` and `$stageVariables` are the call's own ([`TemplateCall`]).
///
/// A template carrying no reference IS the body and renders to itself, which is what a
/// `MOCK` integration's `{"statusCode": 200}` is.
fn rendered_template(template: &str, call: Option<TemplateCall<'_>>) -> Result<String, Unserved> {
    let inputs = ::service_com_amazonaws_apigateway_backplanecontrolservice::service::mapping_template::TemplateInputs {
        payload: call.and_then(|call| call.body).map(|body| body.as_bytes().to_vec()),
        context: call.map(|call| call.context.clone()).unwrap_or_default(),
        stage_variables: call.map(|call| call.stage_variables.clone()).unwrap_or_default(),
        ..Default::default()
    };
    ::service_com_amazonaws_apigateway_backplanecontrolservice::service::mapping_template::rendered_without_directives(template, &inputs)
        .map_err(|unrendered| Unserved::Unperformable(unrendered.reason))
}

/// The status a rendered `MOCK` request template names, or `None` when it names none.
///
/// The documented form is a literal object carrying the status the call produces
/// (`{"statusCode": 200}`), so this reads that member off what the template RENDERED TO: a
/// rendering that is not such an object names no status, and the call then selects the
/// integration response that is the fallback rather than one guessed at.
fn status_code_of(rendered: &str) -> Option<String> {
    let value: ::serde_json::Value = ::serde_json::from_str(rendered).ok()?;
    match value.get("statusCode")? {
        ::serde_json::Value::Number(number) => number.as_u64().map(|code| code.to_string()),
        ::serde_json::Value::String(code) => Some(code.clone()),
        _ => None,
    }
}

/// The integration response `status` selects, or `None` when the integration holds none that
/// this call selects.
///
/// A patterned key is tried first, in key order so the choice is stable whatever order the
/// map yields, and `$default` is the fallback — which is also the only key a call that
/// produced NO status can select.
fn selected_integration_response<'i>(
    integration: &'i Integration,
    status: Option<&str>,
) -> Option<&'i IntegrationResponse> {
    if let Some(status) = status {
        let matched = integration
            .integration_responses
            .values()
            .filter(|response| response.integration_response_key != DEFAULT_SELECTION_KEY)
            .find(|response| key_matches_status(&response.integration_response_key, status));
        if matched.is_some() {
            return matched;
        }
    }
    integration
        .integration_responses
        .values()
        .find(|response| response.integration_response_key == DEFAULT_SELECTION_KEY)
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns, over the inputs each generalises over. The behaviour
    //! end to end — a client's handshake, its frames and the replies it reads — is driven
    //! over a real socket in `tests/all/websocket_transport_data_plane.rs`.

    use super::*;

    #[test]
    fn an_integration_response_key_selects_the_statuses_its_pattern_matches() {
        // The rows span what the rule generalises over: each of the patterned forms a key
        // takes, a literal key, the whole-match requirement, and a key no engine builds.
        let cases: &[(&str, &str, bool)] = &[
            (r"/2\d\d/", "200", true),
            (r"/2\d\d/", "299", true),
            (r"/2\d\d/", "404", false),
            (r"/4\d{2}/", "404", true),
            (r"/4\d{2}/", "200", false),
            // The whole status, not a part of it.
            (r"/2\d\d/", "1200", false),
            ("200", "200", true),
            ("200", "201", false),
            ("/(/", "200", false),
            ("//", "200", false),
        ];
        for (key, status, expected) in cases {
            let selected = key_matches_status(key, status);
            assert_eq!(
                selected, *expected,
                "case ({key:?}, {status:?}): expected {expected}, got {selected}"
            );
        }
    }

    #[test]
    fn a_rendered_template_names_the_status_it_carries() {
        // The rows span what the reading generalises over: the documented literal form, the
        // quoted spelling, an object carrying other members too, a body naming no status,
        // and text that is no JSON object at all.
        let cases: &[(&str, Option<&str>)] = &[
            (r#"{"statusCode": 200}"#, Some("200")),
            (r#"{"statusCode": "404"}"#, Some("404")),
            (r#"{"statusCode":201,"body":"x"}"#, Some("201")),
            (r#"{"body":"x"}"#, None),
            ("not json", None),
        ];
        for (rendered, expected) in cases {
            let named = status_code_of(rendered);
            assert_eq!(
                named.as_deref(),
                *expected,
                "case {rendered:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    #[test]
    fn a_template_renders_its_literal_text_and_the_references_a_call_has_values_for() {
        // The `$context` of the call is the one the gateway reports about it, keyed by the
        // member path a template writes (`super::super::event::template_context`).
        let context = BTreeMap::from([
            ("connectionId".to_string(), "L0SM9cOFyjUCIhA=".to_string()),
            ("routeKey".to_string(), "ping".to_string()),
            ("identity.sourceIp".to_string(), "127.0.0.1".to_string()),
        ]);
        let stage_variables = BTreeMap::from([("tenant".to_string(), "acme".to_string())]);
        let call = TemplateCall::new(&context, &stage_variables, Some(r#"{"action":"ping"}"#));
        // The rows span what the binding generalises over: a template with no reference at
        // all, each kind of reference a frame has a value for (a member of the context, a
        // nested one, a stage variable), both spellings of a reference, a query of the
        // message, and a `$` that opens none.
        let cases: &[(&str, Option<&str>)] = &[
            (r#"{"statusCode": 200}"#, Some(r#"{"statusCode": 200}"#)),
            ("$input.body", Some(r#"{"action":"ping"}"#)),
            ("${input.body}", Some(r#"{"action":"ping"}"#)),
            ("id=$context.connectionId", Some("id=L0SM9cOFyjUCIhA=")),
            ("$context.routeKey", Some("ping")),
            ("$context.identity.sourceIp", Some("127.0.0.1")),
            ("$stageVariables.tenant", Some("acme")),
            ("$input.json('$.action')", Some("\"ping\"")),
            ("price $9", Some("price $9")),
            // Not rendered: a reference this call has no value for, and a directive.
            ("$stageVariables.where", None),
            ("$context.nothing", None),
            ("#set($x = 1)", None),
        ];
        for (template, expected) in cases {
            let rendered = rendered_template(template, Some(call)).ok();
            assert_eq!(
                rendered.as_deref(),
                *expected,
                "case {template:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
