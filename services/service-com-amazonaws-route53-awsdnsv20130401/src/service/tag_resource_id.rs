// SPDX-License-Identifier: CC0-1.0

//! The field a refusal of a tag operation's RESOURCE ID is stated at.
//!
//! Route 53's tag surface is addressed by a resource TYPE plus the id of a resource of
//! that type: `ListTagsForResource` and `ChangeTagsForResource` take one `ResourceId`
//! and `ListTagsForResources` a list of them, every one typed `TagResourceId`
//! (`@length 0..64`), beside the `ResourceType` (`healthcheck` | `hostedzone`) their
//! URI names
//! (`smithy_query(query="operation",operation="ListTagsForResource,ListTagsForResources,ChangeTagsForResource")`).
//! A value outside that bound is refused by the framework's constraint walk, which
//! states its clause at the member path it walked — `resourceId` for the single read's
//! label, and `resourceIds.<position>.member` for one element of the batch's list.
//!
//! Route 53 states it at the id field of the resource TYPE the request names instead:
//! what the request got wrong is a health check's id or a hosted zone's id, so the field
//! named is the one the model spells for that resource's own id in the operations that
//! take it directly — `GetHealthCheckRequest$HealthCheckId` and
//! `GetDNSSECRequest$HostedZoneId`
//! (`smithy_query(query="shape",shape="GetHealthCheckRequest,GetDNSSECRequest")`) — in
//! the lowerCamel a member path is spelled in. A client reading the field name out of the
//! message then learns WHICH resource's id it got wrong, rather than a list position no
//! Route 53 input declares.
//!
//! The framework hands [`smithy_server_runtime::Service::override_refusal`] no request,
//! so that seam cannot see the resource type the refusal has to be stated against. The
//! resource type a tag request names is therefore put in scope around that request's
//! whole run ([`serving`], called from [`crate::service::envelope::ErrorEnvelope`]) and
//! read back from there ([`stated`]) — the same way the sibling services that need one
//! request fact inside that seam carry it (`service-com-amazonaws-cloudwatchlogs`'s
//! `SERVED_REFUSAL_CODE`).
//!
//! Nothing else about the refusal is this module's: which violations a request earns,
//! their order, how many there are and the requirement each clause states are the
//! framework's own, and so is the message of every refusal of every other member.

use std::future::Future;

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::Violation;

/// The id field each taggable resource type reports its OWN id at, one row per value of
/// `TagResourceType` (`smithy_query(query="shape",shape="TagResourceType")`).
///
/// The field is the member the model spells for that resource's id where an operation
/// takes it directly — `HealthCheckId` on `GetHealthCheckRequest`, `HostedZoneId` on
/// `GetDNSSECRequest` — in the lowerCamel every member path is spelled in
/// (`aws_smithy_schema_server::constraints::path::member`). It is not in the model as a
/// property of the tag operations, which type both ids as one `TagResourceId`; this is
/// the mapping from the type a tag request names to the id it is naming.
const ID_FIELDS: &[(&str, &str)] = &[
    ("healthcheck", "healthCheckId"),
    ("hostedzone", "hostedZoneId"),
];

/// The label whose value is the resource type a tag request names, as the modelled
/// `@http` URI of every tag operation spells it.
const RESOURCE_TYPE_LABEL: &str = "{ResourceType}";

/// The member path the framework states a clause about the single-resource operations'
/// `ResourceId` label at: the member name in lowerCamel.
const RESOURCE_ID_MEMBER: &str = "resourceId";

/// The member path of the batch read's id LIST, which the framework states a clause
/// about one element of as `<list>.<position>.member`.
const RESOURCE_ID_LIST_MEMBER: &str = "resourceIds";

/// Every operation of this service's tag surface, by the input schema its modelled
/// `@http` route is read off — so the route, and the position of the resource-type
/// label in it, come from the model rather than from a path literal written here.
fn tag_surface() -> [&'static Schema; 3] {
    [
        crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        crate::ops::types::LIST_TAGS_FOR_RESOURCES_INPUT_SCHEMA,
        crate::ops::types::CHANGE_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
    ]
}

/// The position of `label` among the segments of a modelled `@http` URI `pattern`.
fn label_position(pattern: &str, label: &str) -> Option<usize> {
    pattern
        .split('?')
        .next()
        .unwrap_or(pattern)
        .trim_matches('/')
        .split('/')
        .position(|segment| segment == label)
}

/// The resource type a tag request names, or `None` when `req` addresses no operation of
/// the tag surface.
///
/// The route is matched the way a service matches a modelled route
/// ([`service_common::addresses_modeled_http_route`]) and the value is the path segment
/// the pattern's `{ResourceType}` sits at, decoded as the codec that reads the request
/// decodes a path label. A segment outside `TagResourceType` is still carried: it names
/// no id field ([`id_field`]), which is exactly the case where the framework's own
/// wording stands.
fn resource_type(req: &RtRequest) -> Option<String> {
    let path = service_common::uri_path(req.uri());
    tag_surface().into_iter().find_map(|schema| {
        if !service_common::addresses_modeled_http_route(req, schema) {
            return None;
        }
        let position = label_position(schema.http()?.uri(), RESOURCE_TYPE_LABEL)?;
        let segment = service_common::nth_path_segment(path, position)?;
        Some(service_common::percent_decode_path_label(segment))
    })
}

tokio::task_local! {
    /// The resource type the tag request this task is serving names, put in scope by
    /// [`serving`] around that request's whole run and read back by [`stated`].
    ///
    /// Absent means this task is serving no tag request — every other operation of this
    /// service, and a refusal made outside an operation's run — and then every violation
    /// keeps the path the framework walked to it.
    static NAMED_RESOURCE_TYPE: String;
}

/// Serve `served` with the resource type `req` names in scope, when `req` addresses the
/// tag surface.
///
/// It adds nothing to the request walk: the future handed in is the modelled operation's
/// own run, awaited unchanged, so the response — including every refusal this module does
/// not restate — is the framework's.
pub async fn serving<T>(req: &RtRequest, served: impl Future<Output = T>) -> T {
    match resource_type(req) {
        Some(named) => NAMED_RESOURCE_TYPE.scope(named, served).await,
        None => served.await,
    }
}

/// The field the resource type this task's request names reports its own id at, or `None`
/// when there is no such type in scope.
fn id_field() -> Option<&'static str> {
    let named = NAMED_RESOURCE_TYPE.try_with(String::clone).ok()?;
    ID_FIELDS
        .iter()
        .find_map(|(resource_type, field)| (*resource_type == named).then_some(*field))
}

/// Whether `path` is where the framework states a clause about a tag operation's resource
/// id: the single-resource operations' label, or one ELEMENT of the batch read's list
/// ([`service_common::validation::list_of_element`] recognises that spelling).
///
/// A clause about the list ITSELF — how many ids a batch named — is not about any one
/// resource's id and is left where the framework states it.
fn is_resource_id(path: &str) -> bool {
    path == RESOURCE_ID_MEMBER
        || service_common::validation::list_of_element(path) == Some(RESOURCE_ID_LIST_MEMBER)
}

/// `violations` as a tag operation states them: one about a resource id restated at the
/// id field of the resource type the request names, with no list position, and every
/// other violation — and every violation of a request outside the tag surface — handed
/// back as the framework walked it.
pub fn stated(violations: &[Violation]) -> Vec<Violation> {
    let Some(field) = id_field() else {
        return violations.to_vec();
    };
    violations
        .iter()
        .map(|violation| {
            if is_resource_id(&violation.path) {
                Violation {
                    path: field.to_string(),
                    ..violation.clone()
                }
            } else {
                violation.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    //! The mapping read off paths and routes, with the whole refusal covered end to end
    //! by the crate's integration suite.

    use super::*;
    use aws_smithy_schema_server::ViolationKind;
    use aws_smithy_types::body::SdkBody;

    /// A request to `method` `uri` with no body.
    fn request(method: &str, uri: &str) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_method(method).expect("the method must be valid");
        req.set_uri(uri).expect("the uri must be valid");
        req
    }

    /// Every operation of the tag surface names its resource type, whichever spelling the
    /// request arrives in; a request to any other route names none.
    #[test]
    fn only_a_tag_request_names_a_resource_type() {
        for (case, method, uri, expected) in [
            (
                "the batch read",
                "POST",
                "/2013-04-01/tags/healthcheck",
                Some("healthcheck"),
            ),
            (
                "the single read",
                "GET",
                "/2013-04-01/tags/hostedzone/Z123",
                Some("hostedzone"),
            ),
            (
                "the tag change",
                "POST",
                "/2013-04-01/tags/healthcheck/abc",
                Some("healthcheck"),
            ),
            (
                "an absolute request target, as an SDK sends it",
                "POST",
                "https://route53.amazonaws.com/2013-04-01/tags/hostedzone",
                Some("hostedzone"),
            ),
            (
                "a percent-escaped resource type",
                "POST",
                "/2013-04-01/tags/health%63heck",
                Some("healthcheck"),
            ),
            (
                "a type outside the modelled value set",
                "POST",
                "/2013-04-01/tags/trafficpolicy",
                Some("trafficpolicy"),
            ),
            ("another operation", "GET", "/2013-04-01/hostedzone", None),
            (
                "the tag surface under another method",
                "DELETE",
                "/2013-04-01/tags/healthcheck",
                None,
            ),
        ] {
            assert_eq!(
                resource_type(&request(method, uri)).as_deref(),
                expected,
                "case {case:?}: expected the resource type {expected:?}"
            );
        }
    }

    /// The paths a resource-id clause is stated at, and the ones that are left alone.
    #[test]
    fn a_resource_id_path_is_the_label_or_one_list_element() {
        for path in ["resourceId", "resourceIds.1.member", "resourceIds.10.member"] {
            assert!(
                is_resource_id(path),
                "case {path:?}: expected a resource-id path"
            );
        }
        for path in [
            "resourceIds",
            "resourceType",
            "addTags.1.member.key",
            "resourceIds.member",
            "resourceIds.x.member",
            "otherIds.1.member",
        ] {
            assert!(
                !is_resource_id(path),
                "case {path:?}: expected the framework's own path to stand"
            );
        }
    }

    /// With a resource type in scope, a resource-id clause is restated at that type's id
    /// field and nothing else moves; with none, every violation is handed back as it was.
    #[tokio::test]
    async fn only_a_resource_id_clause_is_restated() {
        let violations = vec![
            Violation {
                path: "resourceIds.2.member".to_string(),
                kind: ViolationKind::MaxLength(64),
                value: Some("z".repeat(100)),
                sensitive: false,
            },
            Violation {
                path: "resourceIds".to_string(),
                kind: ViolationKind::MaxLength(10),
                value: Some("11 items".to_string()),
                sensitive: false,
            },
        ];

        let outside = stated(&violations);
        assert_eq!(
            outside, violations,
            "outside a tag request every violation must keep the framework's path"
        );

        for (resource_type, field) in [
            ("healthcheck", "healthCheckId"),
            ("hostedzone", "hostedZoneId"),
        ] {
            let restated = NAMED_RESOURCE_TYPE
                .scope(resource_type.to_string(), async { stated(&violations) })
                .await;
            assert_eq!(
                restated.iter().map(|v| v.path.clone()).collect::<Vec<_>>(),
                vec![field.to_string(), "resourceIds".to_string()],
                "case {resource_type:?}: expected the id clause at {field:?} and the list \
                 clause left alone"
            );
            assert_eq!(
                restated
                    .iter()
                    .map(|v| (v.kind.clone(), v.value.clone()))
                    .collect::<Vec<_>>(),
                violations
                    .iter()
                    .map(|v| (v.kind.clone(), v.value.clone()))
                    .collect::<Vec<_>>(),
                "case {resource_type:?}: only the path may change"
            );
        }

        let unknown = NAMED_RESOURCE_TYPE
            .scope("trafficpolicy".to_string(), async { stated(&violations) })
            .await;
        assert_eq!(
            unknown, violations,
            "a resource type with no id field must keep the framework's paths"
        );
    }
}
