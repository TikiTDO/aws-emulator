// SPDX-License-Identifier: CC0-1.0

//! The three Amazon Route 53 resource types that are neither a hosted zone, a record set nor
//! a health check: the CIDR collection an IP-based record set routes a client's address
//! through, the reusable delegation set a zone can be created on, and the traffic policy a
//! routing configuration is held as.
//!
//! Each is created in Route 53 itself, through the operation that service publishes for it,
//! and each one's identity is the identifier that service MINTED — so the value a stack's
//! `Ref` and its outputs report is what `ListCidrCollections`,
//! `GetReusableDelegationSet` and `GetTrafficPolicy` resolve
//! ([`super::super::template::identity_is_service_assigned`]).
//!
//! The Route 53 wire itself — the signing name, the URI prefix, the request namespace and the
//! caller reference CloudFormation supplies where a create requires one — is
//! [`super::platform_services`]'s, so a zone's create and a delegation set's are made the
//! same way.

use super::*;

/// The salt each of these types' caller reference is derived under
/// ([`route53_caller_reference`]), so one stack's CIDR collection, its delegation set and its
/// hosted zone do not derive the same reference: the salt is per TYPE, as the two salts
/// [`super::platform_services`] states are.
pub(super) const CIDR_COLLECTION_CALLER_REFERENCE_SALT: &str = "route53-cidr-collection";
pub(super) const DELEGATION_SET_CALLER_REFERENCE_SALT: &str = "route53-delegation-set";

/// The prefix Route 53 wraps a reusable delegation set id in where it reports one
/// (`DelegationSet$Id` comes back as `/delegationset/N…`), and which every operation that
/// TAKES a set id accepts beside the bare form.
const DELEGATION_SET_ID_PATH_PREFIX: &str = "/delegationset/";

/// The ARNs of a reusable delegation set and a traffic policy, and the id inside one: that
/// ARN is what each type's `Ref` returns and therefore the resource's physical id, while
/// every Route 53 call on the entity takes the bare id
/// ([`super::template::delegation_set_arn`]).
use super::template::{delegation_set_arn, route53_id_in, traffic_policy_arn};

/// The error code Route 53 answers for a reusable delegation set it does not hold: "a
/// reusable delegation set with the specified ID does not exist" (`NoSuchDelegationSet`).
const NO_SUCH_DELEGATION_SET: &str = "NoSuchDelegationSet";

/// The error code Route 53 answers for a traffic policy or policy version it does not hold:
/// "no traffic policy exists with the specified ID" (`NoSuchTrafficPolicy`).
const NO_SUCH_TRAFFIC_POLICY: &str = "NoSuchTrafficPolicy";

/// The error code Route 53 answers for a CIDR collection it does not hold: "the CIDR
/// collection you specified, doesn't exist" (`NoSuchCidrCollectionException`).
const NO_SUCH_CIDR_COLLECTION: &str = "NoSuchCidrCollectionException";

/// The `Version` the one traffic policy this type creates carries: "for a new traffic
/// policy, the value of `Version` is always 1" (`TrafficPolicy$Version`).
///
/// `AWS::Route53::TrafficPolicy` declares a name, a document and a comment and creates ONE
/// policy with one version — a further version is `CreateTrafficPolicyVersion`, which no
/// property of this type asks for — so this is the version every call that takes both halves
/// of a policy's `(Id, Version)` address names. Read from the catalogue of return values, so
/// the version the type PUBLISHES and the one its calls name cannot drift apart.
const FIRST_TRAFFIC_POLICY_VERSION: u32 = super::template::FIRST_TRAFFIC_POLICY_VERSION;

/// The `Action` that puts the blocks of one location in a collection: "PUT: Create a CIDR
/// block within the specified collection" (`CidrCollectionChange$Action`).
const CIDR_BLOCKS_PUT: &str = "PUT";

/// The `Action` that takes them out again: "DELETE_IF_EXISTS: Delete an existing CIDR block
/// from the collection".
const CIDR_BLOCKS_DELETE: &str = "DELETE_IF_EXISTS";

/// How many pages of a collection's blocks the delete reads before it stops
/// ([`cidr_blocks_of`]): Route 53 bounds a collection at 1,000 CIDR blocks and serves at
/// least one block per page, so a walk longer than this is a cursor that is not advancing
/// rather than a collection that holds more.
const MAX_CIDR_BLOCK_PAGES: usize = 1_000;

/// A reusable delegation set id in the BARE form every `ResourceId` member is bounded for:
/// the `/delegationset/` prefix a Route 53 answer wraps one in, stripped.
fn bare_delegation_set_id(id: &str) -> &str {
    id.strip_prefix(DELEGATION_SET_ID_PATH_PREFIX).unwrap_or(id)
}

// ---------------------------------------------------------------------------
// AWS::Route53::CidrCollection
// ---------------------------------------------------------------------------

/// The properties of `AWS::Route53::CidrCollection` this arm maps onto
/// `CreateCidrCollection` and the change beside it.
///
/// Source: AWS's CloudFormation resource reference for the type. `Name` is the collection's
/// own name, and `Locations` are the locations the collection associates CIDR blocks with —
/// which `CreateCidrCollection` takes none of, so they are a `ChangeCidrCollection` of its
/// own on the collection just created, the same second call a client makes.
const CIDR_COLLECTION_PROPERTIES: &[&str] = &["Name", "Locations"];

/// Create the CIDR collection an `AWS::Route53::CidrCollection` declares, so a record set
/// whose `CidrRoutingConfig` names that collection and a client reading its blocks reach the
/// collection the template asked for.
///
/// `Ref` on this resource returns the collection id, which `CreateCidrCollection` MINTS and
/// reports, so that id replaces the computed physical id and the type's `Id` and `Arn`
/// attributes are read off the same answer.
pub(super) async fn create_cidr_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, CIDR_COLLECTION_PROPERTIES)?;
    // Required by the type and by the create alike: a declaration with no name describes no
    // collection that could be created.
    let name = property(resource, "Name").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "Property Name cannot be empty for resource of type {}",
            resource.resource_type
        ),
    })?;
    // In the order `CreateCidrCollectionRequest` declares its members.
    let members = format!(
        "{}{}",
        xml_leaf("Name", &name),
        xml_leaf(
            "CallerReference",
            &route53_caller_reference(
                CIDR_COLLECTION_CALLER_REFERENCE_SALT,
                resource,
                propagated
            )
        )
    );
    let answered = call_route53(
        ctx,
        region,
        "POST",
        "/cidrcollection",
        Some(&route53_request("CreateCidrCollectionRequest", &members)),
    )
    .await?;
    let reported = Value::Object(xml_scalar_children(&answered, "Collection"));
    let id = reported
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateCidrCollection answered no Collection.Id".to_string(),
        })?
        .to_string();
    let changes = declared_cidr_changes(resource, CIDR_BLOCKS_PUT)?;
    if !changes.is_empty() {
        change_cidr_collection(ctx, region, &id, &changes).await?;
    }
    Ok(Created {
        physical_resource_id: Some(id),
        attributes: reported_attributes(&reported, &[("Arn", "Arn"), ("Id", "Id")]),
    })
}

/// One `Change` per location the declaration states, carrying `action` over that location's
/// declared blocks.
///
/// `Locations` is a list of `{LocationName, CidrList}` in a template and a list of
/// `CidrCollectionChange` on the wire, which is the same pair plus the action — so a
/// declaration that states neither member of one, or states a member no such change carries,
/// is REFUSED rather than sent as less than it asked for.
fn declared_cidr_changes(
    resource: &StoredResource,
    action: &'static str,
) -> Result<Vec<String>, Refusal> {
    let Some(declared) = resource.properties.get("Locations") else {
        return Ok(Vec::new());
    };
    let Value::Array(locations) = declared else {
        return Err(malformed_property("Locations"));
    };
    let mut changes = Vec::with_capacity(locations.len());
    for location in locations {
        let Value::Object(members) = location else {
            return Err(malformed_property("Locations"));
        };
        for member in members.keys() {
            if !["LocationName", "CidrList"].contains(&member.as_str()) {
                return Err(malformed_property(&format!("Locations.{member}")));
            }
        }
        let name = members
            .get("LocationName")
            .and_then(scalar_text)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| malformed_property("Locations.LocationName"))?;
        let Some(Value::Array(blocks)) = members.get("CidrList") else {
            return Err(malformed_property("Locations.CidrList"));
        };
        let mut rendered = String::new();
        for block in blocks {
            let block = scalar_text(block).ok_or_else(|| malformed_property("Locations.CidrList"))?;
            rendered.push_str(&xml_leaf("Cidr", &block));
        }
        // In the order `CidrCollectionChange` declares its members. The list member carries
        // no `xmlName`, so each change is a `<member>` element of `Changes`.
        changes.push(format!(
            "<member>{}{}<CidrList>{rendered}</CidrList></member>",
            xml_leaf("LocationName", &name),
            xml_leaf("Action", action)
        ));
    }
    Ok(changes)
}

/// Apply `changes` to the collection `id`, which is the one call that puts a CIDR block in a
/// collection or takes one out.
async fn change_cidr_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    id: &str,
    changes: &[String],
) -> Result<(), Refusal> {
    call_route53(
        ctx,
        region,
        "POST",
        &format!("/cidrcollection/{}", percent_encoded(id)),
        Some(&route53_request(
            "ChangeCidrCollectionRequest",
            &format!("<Changes>{}</Changes>", changes.concat()),
        )),
    )
    .await?;
    Ok(())
}

/// Delete the CIDR collection a stack created, by the id its create minted.
///
/// "This CIDR collection is in use, and isn't empty" (`CidrCollectionInUseException`): a
/// collection that still holds blocks is not one the delete takes away, so the blocks it
/// HOLDS come out first — read back from Route 53 rather than taken from the declaration, so
/// a collection changed since the deployment is still emptied. A collection that service no
/// longer holds is already in the state the delete asks for.
pub(super) async fn delete_cidr_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let held = match cidr_blocks_of(ctx, region, physical_resource_id).await {
        Ok(held) => held,
        Err(refusal) if refusal.code == NO_SUCH_CIDR_COLLECTION => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    if !held.is_empty() {
        let changes: Vec<String> = held
            .iter()
            .map(|(location, blocks)| {
                let rendered: String = blocks.iter().map(|block| xml_leaf("Cidr", block)).collect();
                format!(
                    "<member>{}{}<CidrList>{rendered}</CidrList></member>",
                    xml_leaf("LocationName", location),
                    xml_leaf("Action", CIDR_BLOCKS_DELETE)
                )
            })
            .collect();
        match change_cidr_collection(ctx, region, physical_resource_id, &changes).await {
            Ok(()) => {}
            Err(refusal) if refusal.code == NO_SUCH_CIDR_COLLECTION => return Ok(()),
            Err(refusal) => return Err(refusal),
        }
    }
    match call_route53(
        ctx,
        region,
        "DELETE",
        &format!("/cidrcollection/{}", percent_encoded(physical_resource_id)),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_CIDR_COLLECTION => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Every CIDR block the collection holds, keyed by the location it belongs to, as
/// `ListCidrBlocks` reports them.
///
/// PAGED to the end: that listing serves a page at a time and hands back a token for the
/// rest, so a collection holding more blocks than one page carries is read through as many
/// requests as it takes — which is what makes the delete empty ALL of it. The walk is bounded
/// by [`MAX_CIDR_BLOCK_PAGES`], since a token that did not advance would otherwise be read
/// for ever.
async fn cidr_blocks_of(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, Vec<String>>, Refusal> {
    let mut held: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut next: Option<String> = None;
    for _ in 0..MAX_CIDR_BLOCK_PAGES {
        let mut uri = format!(
            "/cidrcollection/{}/cidrblocks",
            percent_encoded(physical_resource_id)
        );
        if let Some(token) = &next {
            uri.push_str(&format!("?nexttoken={}", percent_encoded(token)));
        }
        let answered = call_route53(ctx, region, "GET", &uri, None).await?;
        // Each `CidrBlockSummary` reports both members, so the two element runs are parallel
        // lists and zip entry by entry.
        for (location, block) in xml_elements(&answered, "LocationName")
            .into_iter()
            .zip(xml_elements(&answered, "CidrBlock"))
        {
            held.entry(location).or_default().push(block);
        }
        next = xml_element(&answered, "NextToken").filter(|token| !token.is_empty());
        if next.is_none() {
            break;
        }
    }
    Ok(held)
}

/// The CIDR collection's configuration as Route 53 reports it, for drift detection, and
/// `None` for an id it no longer holds a collection for.
///
/// `ListCidrCollections` is the read that reports a collection's metadata, so the walk looks
/// for the one carrying this id. The one property of this type that is also a scalar member
/// of the reported collection is its `Name`; the declared `Locations` are a list of
/// structures and no member of it at all.
pub(super) async fn describe_cidr_collection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = call_route53(ctx, region, "GET", "/cidrcollection", None).await?;
    let found = xml_elements(&answered, "Id")
        .into_iter()
        .position(|id| id == physical_resource_id);
    let Some(position) = found else {
        return Ok(None);
    };
    let Some(name) = xml_elements(&answered, "Name").into_iter().nth(position) else {
        return Ok(None);
    };
    let mut reported = Map::new();
    reported.insert("Name".to_string(), Value::String(name));
    Ok(Some(Value::Object(reported)))
}

// ---------------------------------------------------------------------------
// AWS::Route53::DelegationSet
// ---------------------------------------------------------------------------

/// The properties of `AWS::Route53::DelegationSet` this arm maps onto
/// `CreateReusableDelegationSet`.
///
/// Source: AWS's CloudFormation resource reference for the type. `CallerReference` is the
/// token the create is retried under, and `HostedZoneId` is "the ID of the hosted zone whose
/// delegation set you want to mark as reusable" — both members of that call under the same
/// names.
const DELEGATION_SET_PROPERTIES: &[&str] = &["CallerReference", "HostedZoneId"];

/// Create the reusable delegation set an `AWS::Route53::DelegationSet` declares, so a hosted
/// zone whose `DelegationSetId` names it is created on the name servers the set holds.
///
/// `Ref` on this resource returns the delegation set id, which
/// `CreateReusableDelegationSet` MINTS and reports in its `/delegationset/`-prefixed form, so
/// the bare id that answer carries replaces the computed physical id. The set's
/// `NameServers` are Route 53's to assign — "a complex type that contains a list of the
/// authoritative name servers" — and are reported as this type's list-valued attribute.
///
/// A declaration that states no `CallerReference` still needs one, because the call declares
/// it `@required`: CloudFormation supplies the same per-stack, per-resource token it supplies
/// for a hosted zone's ([`route53_caller_reference`]).
pub(super) async fn create_delegation_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, DELEGATION_SET_PROPERTIES)?;
    let caller_reference = property(resource, "CallerReference").unwrap_or_else(|| {
        route53_caller_reference(DELEGATION_SET_CALLER_REFERENCE_SALT, resource, propagated)
    });
    // In the order `CreateReusableDelegationSetRequest` declares its members.
    let mut members = xml_leaf("CallerReference", &caller_reference);
    if let Some(zone) = property(resource, "HostedZoneId") {
        members.push_str(&xml_leaf("HostedZoneId", bare_hosted_zone_id(&zone)));
    }
    let answered = call_route53(
        ctx,
        region,
        "POST",
        "/delegationset",
        Some(&route53_request(
            "CreateReusableDelegationSetRequest",
            &members,
        )),
    )
    .await?;
    let reported = xml_scalar_children(&answered, "DelegationSet");
    let id = reported
        .get("Id")
        .and_then(Value::as_str)
        .map(bare_delegation_set_id)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateReusableDelegationSet answered no DelegationSet.Id".to_string(),
        })?
        .to_string();
    Ok(Created {
        physical_resource_id: Some(delegation_set_arn(&id)),
        attributes: delegation_set_attributes(&id, &answered),
    })
}

/// The attributes a created reusable delegation set publishes: the bare `Id` Route 53 minted,
/// and the `NameServers` it delegated to.
///
/// The id is the one every Route 53 call on the set takes, while the resource's own identity
/// is the ARN that id is nested in — which is what this type's `Ref` returns.
fn delegation_set_attributes(id: &str, answered: &str) -> BTreeMap<String, String> {
    let mut attributes = delegation_set_name_servers(answered);
    attributes.insert("Id".to_string(), id.to_string());
    attributes
}

/// The `NameServers` the delegation set reports, as this type's list-valued `Fn::GetAtt`
/// attribute.
///
/// Route 53 ASSIGNS them, so they are read off its answer rather than computed, and carried
/// element by element because the attribute map is string-valued
/// ([`list_attribute_element`]).
fn delegation_set_name_servers(answered: &str) -> BTreeMap<String, String> {
    xml_elements(answered, "NameServer")
        .into_iter()
        .enumerate()
        .map(|(index, server)| (list_attribute_element("NameServers", index), server))
        .collect()
}

/// Delete the reusable delegation set a stack created, by the id its create minted — which is
/// the id nested in the ARN the resource is identified by. A set Route 53 no longer holds
/// (`NoSuchDelegationSet`) is already in the state the delete asks for.
pub(super) async fn delete_delegation_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_route53(
        ctx,
        region,
        "DELETE",
        &format!(
            "/delegationset/{}",
            percent_encoded(route53_id_in(physical_resource_id))
        ),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_DELEGATION_SET => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The delegation set's configuration as Route 53 reports it, for drift detection. An id it
/// no longer holds a set for (`NoSuchDelegationSet`) was deleted.
///
/// The one property of this type that is also a scalar member of the reported set is its
/// `CallerReference`; the zone a set was made reusable FROM is no member of it at all.
pub(super) async fn describe_delegation_set(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_route53(
        ctx,
        region,
        "GET",
        &format!(
            "/delegationset/{}",
            percent_encoded(route53_id_in(physical_resource_id))
        ),
        None,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == NO_SUCH_DELEGATION_SET => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let reported = Value::Object(xml_scalar_children(&answered, "DelegationSet"));
    Ok(Some(reported_as_properties(
        &reported,
        &[("CallerReference", "CallerReference")],
    )))
}

// ---------------------------------------------------------------------------
// AWS::Route53::TrafficPolicy
// ---------------------------------------------------------------------------

/// The properties of `AWS::Route53::TrafficPolicy` this arm maps onto
/// `CreateTrafficPolicy`.
///
/// Source: AWS's CloudFormation resource reference for the type — the policy's `Name`, the
/// `Document` that defines it and the `Comment` the create carries — each a member of that
/// call under the same name. The DNS type of the record sets the policy creates is no
/// property of the type: Route 53 reads it off the document itself
/// (`TrafficPolicy$Type`).
const TRAFFIC_POLICY_PROPERTIES: &[&str] = &["Comment", "Document", "Name"];

/// Create the traffic policy an `AWS::Route53::TrafficPolicy` declares, so a traffic policy
/// instance built on it — and a client reading the policy back — reaches the configuration
/// the template declared.
///
/// `Ref` on this resource returns the traffic policy id, which `CreateTrafficPolicy` MINTS
/// and reports, so that id replaces the computed physical id and the type's `Id` attribute is
/// read off the same answer. A document Route 53 will not accept
/// (`InvalidTrafficPolicyDocument`) fails the resource with that service's own refusal rather
/// than reporting a policy nothing created.
pub(super) async fn create_traffic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Created, Refusal> {
    refuse_unmapped_properties(resource, TRAFFIC_POLICY_PROPERTIES)?;
    let name = property(resource, "Name").ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "Property Name cannot be empty for resource of type {}",
            resource.resource_type
        ),
    })?;
    let document = traffic_policy_document(resource)?;
    // In the order `CreateTrafficPolicyRequest` declares its members.
    let mut members = format!(
        "{}{}",
        xml_leaf("Name", &name),
        xml_leaf("Document", &document)
    );
    if let Some(comment) = property(resource, "Comment") {
        members.push_str(&xml_leaf("Comment", &comment));
    }
    let answered = call_route53(
        ctx,
        region,
        "POST",
        "/trafficpolicy",
        Some(&route53_request("CreateTrafficPolicyRequest", &members)),
    )
    .await?;
    let reported = Value::Object(xml_scalar_children(&answered, "TrafficPolicy"));
    let id = reported
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: "CreateTrafficPolicy answered no TrafficPolicy.Id".to_string(),
        })?
        .to_string();
    Ok(Created {
        physical_resource_id: Some(traffic_policy_arn(&id)),
        attributes: reported_attributes(
            &reported,
            &[("Id", "Id"), ("Type", "Type"), ("Version", "Version")],
        ),
    })
}

/// The `Document` the create carries: "the definition of a traffic policy in JSON format".
///
/// The property is a JSON DOCUMENT, which a template may state as the document itself or as
/// the text of one — so an object or an array is serialised to the text the call takes, and a
/// string is already that text and goes through as written. A declaration carrying no
/// document describes no policy that could be created.
fn traffic_policy_document(resource: &StoredResource) -> Result<String, Refusal> {
    let declared = resource
        .properties
        .get("Document")
        .filter(|declared| !declared.is_null())
        .ok_or_else(|| Refusal {
            code: MISSING_PROPERTY_CODE.to_string(),
            message: format!(
                "Property Document cannot be empty for resource of type {}",
                resource.resource_type
            ),
        })?;
    match declared {
        Value::String(text) => Ok(text.clone()),
        document => ::serde_json::to_string(document).map_err(|error| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: error.to_string(),
        }),
    }
}

/// Delete the traffic policy a stack created, by the id its create minted — the id nested in
/// the ARN the resource is identified by — and the version that create answered
/// ([`FIRST_TRAFFIC_POLICY_VERSION`]). A policy Route 53 no longer holds
/// (`NoSuchTrafficPolicy`) is already in the state the delete asks for.
pub(super) async fn delete_traffic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    match call_route53(
        ctx,
        region,
        "DELETE",
        &format!(
            "/trafficpolicy/{}/{FIRST_TRAFFIC_POLICY_VERSION}",
            percent_encoded(route53_id_in(physical_resource_id))
        ),
        None,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NO_SUCH_TRAFFIC_POLICY => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The traffic policy's configuration as Route 53 reports it, for drift detection. An id it
/// no longer holds a policy for (`NoSuchTrafficPolicy`) was deleted.
///
/// The properties of this type that are also scalar members of the reported policy are its
/// `Name` and its `Comment`; the `Document` is compared by neither side, because the text
/// Route 53 holds is the serialisation of the declaration rather than the declaration itself.
pub(super) async fn describe_traffic_policy(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let answered = match call_route53(
        ctx,
        region,
        "GET",
        &format!(
            "/trafficpolicy/{}/{FIRST_TRAFFIC_POLICY_VERSION}",
            percent_encoded(route53_id_in(physical_resource_id))
        ),
        None,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == NO_SUCH_TRAFFIC_POLICY => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let reported = Value::Object(xml_scalar_children(&answered, "TrafficPolicy"));
    Ok(Some(reported_as_properties(
        &reported,
        &[("Name", "Name"), ("Comment", "Comment")],
    )))
}
