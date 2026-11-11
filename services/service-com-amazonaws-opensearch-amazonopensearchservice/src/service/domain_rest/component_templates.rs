// SPDX-License-Identifier: CC0-1.0

//! The data plane's COMPONENT TEMPLATE API: `PUT`, `GET` and `DELETE` on
//! `/_component_template[/{name}]`.
//!
//! A component template is "a building block for constructing index templates that
//! specifies index mappings, settings, and aliases"; it configures no index by itself. An
//! index template REFERENCES one through `composed_of`, and the index that template
//! creates carries what each component declares with the template's own section merged on
//! top — the one composition [`crate::state::Domain::templated_schema`] performs where an
//! index is minted, so a component written after the template still applies to an index
//! created after it.

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{json, Map, Value};

use crate::state::{self, fgac, ingest, ComponentTemplate, Domain};

use super::{
    error, guarded, ok, wrong_method, Request, PARSE_EXCEPTION, RESOURCE_NOT_FOUND_EXCEPTION,
    TEMPLATE_QUOTA,
};

/// The member a component template carries its index configuration under.
const TEMPLATE: &str = "template";

/// `PUT`, `GET` or `DELETE` on ONE component template, and the `GET` that names none.
pub(super) fn component_template_request(
    domain: &mut Domain,
    request: &Request<'_>,
    name: Option<&str>,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => guarded(
            domain,
            request,
            fgac::ACTION_PUT_COMPONENT_TEMPLATE,
            |domain| match name {
                Some(name) => write(domain, request, name),
                None => missing(None),
            },
        ),
        "GET" | "HEAD" => guarded(
            domain,
            request,
            fgac::ACTION_GET_COMPONENT_TEMPLATE,
            |domain| {
                let reported: Vec<Value> = domain
                    .component_templates
                    .values()
                    .filter(|held| name.is_none_or(|named| named == held.name))
                    .map(|held| {
                        json!({ "name": held.name, "component_template": held.definition })
                    })
                    .collect();
                if reported.is_empty() {
                    if let Some(name) = name {
                        return missing(Some(name));
                    }
                }
                ok(&json!({ "component_templates": reported }))
            },
        ),
        "DELETE" => guarded(
            domain,
            request,
            fgac::ACTION_PUT_COMPONENT_TEMPLATE,
            |domain| {
                let Some(name) = name else {
                    return missing(None);
                };
                match domain.component_templates.remove(name) {
                    Some(_) => ok(&json!({ "acknowledged": true })),
                    None => missing(Some(name)),
                }
            },
        ),
        _ => wrong_method(request, "PUT, POST, GET, HEAD, DELETE"),
    }
}

/// Store the component the request carries: the index configuration it declares, kept in
/// the same form an index's own schema is stored in so the composition merges one shape.
fn write(domain: &mut Domain, request: &Request<'_>, name: &str) -> RtResponse {
    let definition = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object",
            )
        }
        Err(refusal) => return *refusal,
    };
    // "template: the template to be applied" — a component that declares none carries
    // nothing for an index template to be composed of, which the engine refuses rather
    // than storing an empty building block.
    let Some(template) = definition.get(TEMPLATE).filter(|value| value.is_object()) else {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("Required [{TEMPLATE}]"),
        );
    };
    if !domain.component_templates.contains_key(name)
        && domain.component_templates.len() >= TEMPLATE_QUOTA
    {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "domain [{}] already holds the maximum of {TEMPLATE_QUOTA} component templates",
                domain.domain_name
            ),
        );
    }
    let mut schema = Map::new();
    for section in [state::SCHEMA_MAPPINGS, "settings"] {
        if let Some(value) = template.get(section) {
            schema.insert(section.to_string(), value.clone());
        }
    }
    let schema = state::document_from_json(&Value::Object(schema));
    let values = state::index_schema_values(&schema);
    if values > state::INDEX_SCHEMA_QUOTA {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("index schema holds {values} values, past the maximum this cluster takes"),
        );
    }
    domain.component_templates.insert(
        name.to_string(),
        ComponentTemplate {
            name: name.to_string(),
            schema,
            definition,
        },
    );
    ok(&json!({ "acknowledged": true }))
}

/// The refusal for a component template the cluster does not hold, and for a request that
/// named none where one is required.
fn missing(name: Option<&str>) -> RtResponse {
    match name {
        Some(name) => error(
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
            &format!("component template matching [{name}] not found"),
        ),
        None => error(400, ingest::ILLEGAL_ARGUMENT_EXCEPTION, "name is missing"),
    }
}
