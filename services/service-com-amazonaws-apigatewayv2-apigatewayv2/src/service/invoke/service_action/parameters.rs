// SPDX-License-Identifier: CC0-1.0

//! The PARAMETERS a first-class service-action integration hands the action it names:
//! `Integration$RequestParameters` read as the mapping it is.
//!
//! "For HTTP API integrations with a specified `integrationSubtype`, request parameters are
//! a key-value map specifying parameters that are passed to `AWS_PROXY` integrations. You
//! can provide static values, or map request data, stage variables, or context variables
//! that are evaluated at runtime" (`Integration$RequestParameters`). So each entry's KEY is
//! a parameter of the service action and its VALUE is either one mapping expression or a
//! static value, and this module is the single reading of that value.
//!
//! The expressions, from the mapping-expression catalog API Gateway documents for parameter
//! mapping (`$request.*`, `$context.*`, `$stageVariables.*`, and a static value for anything
//! else):
//!
//! | value | what it resolves to |
//! |---|---|
//! | `$request.header.<name>` | that request header, case-insensitively named |
//! | `$request.querystring.<name>` | that query-string parameter |
//! | `$request.path` | the request path within the stage, which the route key matched |
//! | `$request.path.<name>` | that path variable of the matched route key |
//! | `$request.body` | the request body, as the text it arrived as |
//! | `$request.body.<member>[.<member>]` | that member selected out of the JSON body |
//! | `$context.<variable>` | that context variable of the invocation |
//! | `$stageVariables.<name>` | that variable of the serving stage |
//! | anything not starting with `$` | itself, as a static value |
//!
//! A value that names something the request does not carry — an absent header, a member no
//! body holds, a variable the stage does not state — resolves to NOTHING, and the parameter
//! is then not supplied to the action at all rather than supplied empty: a mapping is a
//! reference to a value, and a reference to an absent value is not the empty string. Such a
//! parameter is REPORTED as one this request did not satisfy ([`Mapped::unresolved_required`])
//! as well as left unsupplied, because a mapping the integration wrote for a parameter the
//! action requires is a value the request was supposed to carry. What a MISSING parameter
//! means is the action's own rule, decided by [`super::Action`].
//!
//! Nothing here is parsed by hand: a JSON body is read by `serde_json`, and the context
//! variables are the ones the gateway already states for this invocation
//! ([`super::super::proxy_event`]), looked up by the member path a `$context` expression
//! names — so the two cannot disagree about what the request was. Each of those readings is
//! made AT MOST ONCE per invoke ([`Sources`]), so a mapping of many entries costs one pass
//! over the request and not one per entry.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::super::proxy_event::{event_2_0, Invocation, ProxyRequest};

/// The prefix every mapping expression carries; a value without it is a static value.
const EXPRESSION_SIGIL: char = '$';

/// How a mapping expression's parts are separated.
const PART_SEPARATOR: char = '.';

/// The three roots a mapping expression selects from.
const REQUEST_ROOT: &str = "request";
const CONTEXT_ROOT: &str = "context";
const STAGE_VARIABLES_ROOT: &str = "stageVariables";

/// The `$request` sources: the request's own parts a parameter can be mapped from.
const HEADER_SOURCE: &str = "header";
const QUERY_STRING_SOURCE: &str = "querystring";
const PATH_SOURCE: &str = "path";
const BODY_SOURCE: &str = "body";

/// The member of a `2.0` event that holds the `$context` variables of the invocation.
const REQUEST_CONTEXT_MEMBER: &str = "requestContext";

/// The `$context` variables whose own spelling differs from where the invocation states
/// them, with the member path each reads: the HTTP request's own facts, which a `2.0`
/// request context keeps under `http`, and the two spellings of the request's instant.
///
/// Source: the `$context` variables API Gateway documents for an HTTP api
/// (`$context.httpMethod`, `$context.path`, `$context.protocol`,
/// `$context.identity.sourceIp`, `$context.identity.userAgent`, `$context.requestTime`,
/// `$context.requestTimeEpoch`). Every other variable is stated under its own name, so it
/// is looked up as written.
const CONTEXT_ALIASES: &[(&str, &[&str])] = &[
    ("httpMethod", &["http", "method"]),
    ("path", &["http", "path"]),
    ("protocol", &["http", "protocol"]),
    ("identity.sourceIp", &["http", "sourceIp"]),
    ("identity.userAgent", &["http", "userAgent"]),
    ("requestTime", &["time"]),
    ("requestTimeEpoch", &["timeEpoch"]),
];

/// The parameters one invoke hands the action, by the action's own parameter name.
///
/// Only the entries that RESOLVED are held as values: a mapping naming a value the request
/// does not carry supplies no parameter (see the module docs). Which of them failed to
/// resolve is kept beside them, because a mapping the request did not satisfy is a different
/// thing from a mapping the integration never wrote.
#[derive(Debug, Clone, Default)]
pub struct Mapped {
    resolved: BTreeMap<String, String>,
    /// The parameters the integration maps a mapping EXPRESSION for that this request
    /// carries no value for. A parameter mapped a static value is never among them, and
    /// neither is one the integration maps nothing at all for.
    unresolved: BTreeSet<String>,
}

impl Mapped {
    /// The value mapped for the action's parameter `name`, or `None` when the integration
    /// mapped none or its mapping resolved to nothing.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.resolved.get(name).map(String::as_str)
    }

    /// The first mapped parameter that is NOT one of `carried`, or `None` when every mapped
    /// parameter is one the action carries.
    ///
    /// A parameter this emulator cannot hand the action is not silently dropped: a request
    /// whose mapping it would lose is answered as an integration that produced no response,
    /// because a publication missing what the integration mapped onto it is not the
    /// publication the api was configured to make.
    pub fn beyond(&self, carried: &[&str]) -> Option<&str> {
        self.resolved
            .keys()
            .map(String::as_str)
            .find(|name| !carried.contains(name))
    }

    /// The first parameter of `required` whose mapping expression this request carries no
    /// value for, or `None` when the request satisfied every one of them.
    ///
    /// A parameter the action requires is what the mapping exists to supply, so a request
    /// carrying nothing for the expression that selects it is a request the action cannot be
    /// performed for — and the fault is the REQUEST's, which is why it is told apart here
    /// from a parameter the integration never mapped at all.
    pub fn unresolved_required(&self, required: &[&str]) -> Option<&str> {
        self.unresolved
            .iter()
            .find(|name| required.contains(&name.as_str()))
            .map(String::as_str)
    }
}

/// Resolve every entry of `request_parameters` against this invocation.
///
/// Each SOURCE a mapping reads is computed at most once per invoke ([`Sources`]), so the work
/// here is one pass over the request's own parts plus one lookup per entry — a mapping with
/// many `$request.body.…` entries does not re-read the body once per entry.
pub fn resolve(
    request_parameters: &BTreeMap<String, String>,
    request: &ProxyRequest,
    invocation: &Invocation,
) -> Mapped {
    let mut sources = Sources {
        request,
        invocation,
        body_text: None,
        body_document: None,
        path_parameters: None,
        context: None,
    };
    let mut resolved = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    for (parameter, value) in request_parameters {
        match value.strip_prefix(EXPRESSION_SIGIL) {
            None => {
                resolved.insert(parameter.clone(), value.clone());
            }
            Some(expression) => match sources.value_of(expression) {
                Some(mapped) => {
                    resolved.insert(parameter.clone(), mapped);
                }
                None => {
                    unresolved.insert(parameter.clone());
                }
            },
        }
    }
    Mapped {
        resolved,
        unresolved,
    }
}

/// What the mappings of one invoke read, each computed AT MOST ONCE however many entries name
/// it: the request's body as text and as the document it is, the path variables the matched
/// route key names, and the invocation's context variables.
///
/// Held per invoke rather than per entry because the cost of each is the cost of the REQUEST
/// — decoding the body, parsing it, stating the context — while the number of entries is the
/// integration's: computing them per entry would multiply one by the other.
struct Sources<'i> {
    request: &'i ProxyRequest,
    invocation: &'i Invocation,
    /// The body as the text it is, or `None` for a body that is not text; the outer `Option`
    /// is "not read yet".
    body_text: Option<Option<&'i str>>,
    /// The body as the JSON document it is, or `None` for a body that is not one.
    body_document: Option<Option<Value>>,
    /// The path variables of the matched route key.
    path_parameters: Option<BTreeMap<String, String>>,
    /// The `$context` variables of this invocation.
    context: Option<Value>,
}

impl Sources<'_> {
    /// The value the expression `$<expression>` selects, or `None` when it names nothing this
    /// request carries (and for an expression naming no source at all).
    fn value_of(&mut self, expression: &str) -> Option<String> {
        let (root, rest) = split_part(expression);
        match root {
            REQUEST_ROOT => self.request_value(rest?),
            CONTEXT_ROOT => {
                let variable = rest?;
                context_value(variable, self.context())
            }
            STAGE_VARIABLES_ROOT => self.request.stage_variables.get(rest?).cloned(),
            _ => None,
        }
    }

    /// The value a `$request.…` expression selects.
    fn request_value(&mut self, expression: &str) -> Option<String> {
        let (source, rest) = split_part(expression);
        match source {
            // A header is named case-insensitively (RFC 9110 §5.1), and the request's headers
            // are held under their lowercased names.
            HEADER_SOURCE => self
                .request
                .headers
                .get(&rest?.to_ascii_lowercase())
                .cloned(),
            QUERY_STRING_SOURCE => self.request.query_parameters.get(rest?).cloned(),
            PATH_SOURCE => match rest {
                None => Some(self.request.route_path.clone()),
                Some(name) => self.path_parameters().get(name).cloned(),
            },
            // The whole body for the bare source, and otherwise the member the remaining
            // parts name, selected out of the JSON the body is.
            BODY_SOURCE => match rest {
                None => self.body_text().map(str::to_string),
                Some(member_path) => self.body_member(member_path),
            },
            _ => None,
        }
    }

    /// The request's body as the text it is, read once.
    ///
    /// A body that is not text names no value at all — a mapped parameter is a string, and a
    /// JSON string carries text.
    fn body_text(&mut self) -> Option<&str> {
        let request = self.request;
        *self
            .body_text
            .get_or_insert_with(|| std::str::from_utf8(&request.body).ok())
    }

    /// The member `member_path` names, selected out of the document the body is.
    ///
    /// A member no body holds names no value. A selected value that is not itself a string is
    /// carried as the JSON it is, which is what makes `$request.body.detail` usable for a
    /// parameter whose own content is a document.
    fn body_member(&mut self, member_path: &str) -> Option<String> {
        let document = self.body_document()?;
        let mut selected = document;
        for member in member_path.split(PART_SEPARATOR) {
            selected = match selected {
                // An array is indexed by position, which is how a member path reaches into a
                // list the body carries.
                Value::Array(entries) => entries.get(member.parse::<usize>().ok()?)?,
                _ => selected.get(member)?,
            };
        }
        text_of(selected)
    }

    /// The request's body as the JSON document it is, parsed once.
    fn body_document(&mut self) -> Option<&Value> {
        if self.body_document.is_none() {
            let parsed = self
                .body_text()
                .and_then(|text| serde_json::from_str::<Value>(text).ok());
            self.body_document = Some(parsed);
        }
        self.body_document.as_ref()?.as_ref()
    }

    /// The path variables the matched route key names, read off the request's own path once.
    fn path_parameters(&mut self) -> &BTreeMap<String, String> {
        let request = self.request;
        self.path_parameters
            .get_or_insert_with(|| request.path_parameters())
    }

    /// The `$context` variables of this invocation, as the gateway states them — stated once,
    /// and only when something reads one: a mapping of static values and request parts needs
    /// none.
    fn context(&mut self) -> &Value {
        let (request, invocation) = (self.request, self.invocation);
        self.context.get_or_insert_with(|| {
            event_2_0(request, invocation)
                .get(REQUEST_CONTEXT_MEMBER)
                .cloned()
                .unwrap_or(Value::Null)
        })
    }
}

/// The value a `$context.…` expression selects out of the stated context.
fn context_value(variable: &str, context: &Value) -> Option<String> {
    let aliased = CONTEXT_ALIASES
        .iter()
        .find(|(spelling, _)| *spelling == variable)
        .map(|(_, path)| (*path).to_vec());
    let path = aliased.unwrap_or_else(|| variable.split(PART_SEPARATOR).collect());
    let mut selected = context;
    for member in path {
        selected = selected.get(member)?;
    }
    text_of(selected)
}

/// One selected JSON value as the text a mapped parameter carries: a string as itself, and
/// anything else as the JSON it is, so nothing of what the request held is lost. A `null`
/// carries no value at all.
fn text_of(selected: &Value) -> Option<String> {
    match selected {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// `expression` split at its first separator: the leading part, and the remainder when it
/// has one.
fn split_part(expression: &str) -> (&str, Option<&str>) {
    match expression.split_once(PART_SEPARATOR) {
        Some((part, rest)) => (part, Some(rest).filter(|rest| !rest.is_empty())),
        None => (expression, None),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    /// The invoke every row below maps from: a POST of a JSON body to a route carrying a
    /// path variable, a query string, headers and a stage variable.
    fn invoke() -> (ProxyRequest, Invocation) {
        let request = ProxyRequest {
            method: "POST".to_string(),
            raw_path: "/live/orders/7".to_string(),
            route_path: "/orders/7".to_string(),
            raw_query_string: "kind=retail&tag=a&tag=b".to_string(),
            headers: [
                ("content-type".to_string(), "application/json".to_string()),
                ("x-source".to_string(), "app.orders".to_string()),
            ]
            .into_iter()
            .collect(),
            query_parameters: [
                ("kind".to_string(), "retail".to_string()),
                ("tag".to_string(), "a,b".to_string()),
            ]
            .into_iter()
            .collect(),
            body: br#"{"note":"placed","item":{"sku":"A-1","count":2},"tags":["x","y"]}"#
                .to_vec(),
            route_key: "POST /orders/{id}".to_string(),
            api_id: "abcd1234".to_string(),
            stage_name: "live".to_string(),
            stage_variables: [("tenant".to_string(), "blue".to_string())]
                .into_iter()
                .collect(),
            region: "us-east-1".to_string(),
            domain_name: "abcd1234.execute-api.us-east-1.amazonaws.com".to_string(),
        };
        let invocation = Invocation {
            account_id: "000000000000".to_string(),
            request_id: "request-1".to_string(),
            received: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            authorizer_context: None,
            authorizer_principal: None,
            admitted_jwt: None,
        };
        (request, invocation)
    }

    /// The value one entry maps to for that invoke.
    fn mapped(value: &str) -> Option<String> {
        let (request, invocation) = invoke();
        let parameters = [("P".to_string(), value.to_string())].into_iter().collect();
        resolve(&parameters, &request, &invocation)
            .get("P")
            .map(str::to_string)
    }

    /// RULE: an entry's value is one mapping expression over the request, the invocation's
    /// context or the stage's variables, or a static value — and one that names a value the
    /// request does not carry supplies no parameter at all.
    #[test]
    fn every_mapping_expression_resolves_to_what_it_names() {
        // Rows span the catalog in the module docs — one per source, plus the forms that
        // name nothing.
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("a static value", "app.orders", Some("app.orders")),
            ("a header", "$request.header.x-source", Some("app.orders")),
            (
                "a header named in another case",
                "$request.header.X-Source",
                Some("app.orders"),
            ),
            ("an absent header", "$request.header.x-absent", None),
            ("a query parameter", "$request.querystring.kind", Some("retail")),
            (
                "a repeated query parameter, which is one value",
                "$request.querystring.tag",
                Some("a,b"),
            ),
            ("an absent query parameter", "$request.querystring.absent", None),
            ("the path within the stage", "$request.path", Some("/orders/7")),
            ("a path variable", "$request.path.id", Some("7")),
            ("a path variable the key does not name", "$request.path.other", None),
            (
                "the whole body",
                "$request.body",
                Some(r#"{"note":"placed","item":{"sku":"A-1","count":2},"tags":["x","y"]}"#),
            ),
            ("a member of the body", "$request.body.note", Some("placed")),
            (
                "a nested member of the body",
                "$request.body.item.sku",
                Some("A-1"),
            ),
            ("a numeric member of the body", "$request.body.item.count", Some("2")),
            (
                "a member that is a document, carried as the JSON it is",
                "$request.body.tags",
                Some(r#"["x","y"]"#),
            ),
            ("an entry of a list the body carries", "$request.body.tags.1", Some("y")),
            ("a member no body holds", "$request.body.absent", None),
            ("a stage variable", "$stageVariables.tenant", Some("blue")),
            ("a variable the stage does not state", "$stageVariables.absent", None),
            ("a context variable", "$context.requestId", Some("request-1")),
            ("the api's own id", "$context.apiId", Some("abcd1234")),
            ("the stage", "$context.stage", Some("live")),
            ("the method, which the context keeps under http", "$context.httpMethod", Some("POST")),
            (
                "the caller's address, likewise",
                "$context.identity.sourceIp",
                Some("127.0.0.1"),
            ),
            ("a context variable nothing states", "$context.nothing", None),
            ("an expression naming no source", "$nothing.at.all", None),
            ("a bare sigil", "$", None),
        ];
        for (case, value, expected) in rows {
            let actual = mapped(value);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?} for {value:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a parameter the action does not carry is reported, so nothing an integration
    /// mapped is silently dropped.
    #[test]
    fn a_parameter_the_action_does_not_carry_is_reported() {
        let (request, invocation) = invoke();
        let parameters: BTreeMap<String, String> = [
            ("Source".to_string(), "app.orders".to_string()),
            ("Nonsense".to_string(), "x".to_string()),
        ]
        .into_iter()
        .collect();
        let resolved = resolve(&parameters, &request, &invocation);
        let beyond = resolved.beyond(&["Source", "Detail"]);
        assert_eq!(
            beyond,
            Some("Nonsense"),
            "case \"a parameter outside the action\": expected it reported, got {beyond:?}"
        );
        let carried = resolved.beyond(&["Source", "Nonsense"]);
        assert_eq!(
            carried, None,
            "case \"every parameter carried\": expected nothing reported, got {carried:?}"
        );
    }

    /// RULE: a parameter the action REQUIRES whose mapping expression this request carries no
    /// value for is reported as one the request did not satisfy — while a static value, a
    /// resolved expression and a parameter the integration maps nothing for are not.
    #[test]
    fn a_required_parameter_the_request_does_not_satisfy_is_reported() {
        /// One case: the entries the integration maps, and the required parameter the
        /// request must then be reported as not satisfying.
        struct Row {
            case: &'static str,
            entries: &'static [(&'static str, &'static str)],
            expected: Option<&'static str>,
        }
        // Rows differ along what the entry maps and whether the request carries it: an
        // expression it does carry, one it does not, a static value, a body that is not JSON
        // at all, and a parameter the integration maps nothing for.
        let rows: &[Row] = &[
            Row {
                case: "an expression the request carries",
                entries: &[("MessageBody", "$request.body.note")],
                expected: None,
            },
            Row {
                case: "an expression naming a member no body holds",
                entries: &[("MessageBody", "$request.body.absent")],
                expected: Some("MessageBody"),
            },
            Row {
                case: "an expression naming an absent header",
                entries: &[("MessageBody", "$request.header.x-absent")],
                expected: Some("MessageBody"),
            },
            Row {
                case: "a static value",
                entries: &[("MessageBody", "placed")],
                expected: None,
            },
            Row {
                case: "a required parameter the integration maps nothing for",
                entries: &[("DelaySeconds", "$request.body.absent")],
                expected: None,
            },
            Row {
                case: "the other required parameter, unresolved",
                entries: &[
                    ("MessageBody", "placed"),
                    ("QueueUrl", "$request.body.queue"),
                ],
                expected: Some("QueueUrl"),
            },
        ];
        let required = &["QueueUrl", "MessageBody"];
        for row in rows {
            let case = row.case;
            let (request, invocation) = invoke();
            let parameters: BTreeMap<String, String> = row
                .entries
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
            let resolved = resolve(&parameters, &request, &invocation);
            let actual = resolved.unresolved_required(required);
            assert_eq!(
                actual, row.expected,
                "case {case:?}: expected {:?}, got {actual:?}",
                row.expected
            );
        }
    }

    /// RULE: a body that is not text names no value, because a mapped parameter is text.
    #[test]
    fn a_body_that_is_not_text_names_no_value() {
        let rows: &[(&str, Option<&str>)] = &[("$request.body", None), ("$request.body.note", None)];
        let (mut request, invocation) = invoke();
        request.body = vec![0xff, 0xfe, 0x00];
        for (value, expected) in rows {
            let parameters = [("P".to_string(), (*value).to_string())]
                .into_iter()
                .collect();
            let actual = resolve(&parameters, &request, &invocation)
                .get("P")
                .map(str::to_string);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {value:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
