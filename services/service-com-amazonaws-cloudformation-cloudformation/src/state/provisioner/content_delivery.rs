// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon CloudFront's small identity resources: origin
//! access identities, public keys and key groups. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`. `AWS::CloudFront::Distribution` is not here yet: its
//! `DistributionConfig` needs a renderer from CloudFormation's list spelling to CloudFront's
//! `Quantity`/`Items` XML, which is its own piece of work.
//!
//! CloudFront MINTS each of these resources' ids, so every create answers the id the service
//! reported as the resource's physical id ([`Created::physical_resource_id`]), as the
//! GuardDuty detector arm does. CloudFront versions what it stores by an `ETag` it answers in a
//! response header and demands back as `If-Match` on every update and delete, so each update
//! and delete reads the current `ETag` first ([`call_rest_xml_versioned`]).

use super::*;

/// The signing name and Region-independent URI prefix of CloudFront's 2020-05-31 API.
pub(super) const CLOUDFRONT_SIGNING_NAME: &str = "cloudfront";
pub(super) const CLOUDFRONT_URI_PREFIX: &str = "/2020-05-31";
/// The namespace every CloudFront 2020-05-31 request document is written under.
pub(super) const CLOUDFRONT_XML_NAMESPACE: &str = "http://cloudfront.amazonaws.com/doc/2020-05-31/";

/// The salts each CloudFront type's `CallerReference` is derived under, so a retried create of
/// one resource of one stack sends the same reference and CloudFront recognises the repeat.
pub(super) const ORIGIN_ACCESS_IDENTITY_CALLER_REFERENCE_SALT: &str = "cloudfront-origin-access-identity";
pub(super) const PUBLIC_KEY_CALLER_REFERENCE_SALT: &str = "cloudfront-public-key";

/// The codes CloudFront answers for an id it does not hold, per resource kind; a delete of
/// something already gone is what the delete asks for.
pub(super) const NO_SUCH_ORIGIN_ACCESS_IDENTITY: &str = "NoSuchCloudFrontOriginAccessIdentity";
pub(super) const NO_SUCH_PUBLIC_KEY: &str = "NoSuchPublicKey";
pub(super) const NO_SUCH_KEY_GROUP: &str = "NoSuchResource";

/// The three resource kinds this file owns, with the URI each one's record lives under and the
/// element CloudFront's answers wrap it in.
#[derive(Clone, Copy)]
pub(super) enum CloudFrontKind {
    OriginAccessIdentity,
    PublicKey,
    KeyGroup,
}

impl CloudFrontKind {
    /// The collection URI a create POSTs to and the record URI is under.
    fn collection(self) -> &'static str {
        match self {
            Self::OriginAccessIdentity => "/origin-access-identity/cloudfront",
            Self::PublicKey => "/public-key",
            Self::KeyGroup => "/key-group",
        }
    }

    /// The URI an update PUTs the configuration to: CloudFront binds the identity and the
    /// public key's to `…/{Id}/config` and the key group's to the record itself.
    fn config_suffix(self) -> &'static str {
        match self {
            Self::OriginAccessIdentity | Self::PublicKey => "/config",
            Self::KeyGroup => "",
        }
    }

    /// The element a create or get answer carries the record under.
    fn answer_element(self) -> &'static str {
        match self {
            Self::OriginAccessIdentity => "CloudFrontOriginAccessIdentity",
            Self::PublicKey => "PublicKey",
            Self::KeyGroup => "KeyGroup",
        }
    }

    /// The configuration property the CloudFormation type declares, which is also the
    /// request document's root element.
    fn config_property(self) -> &'static str {
        match self {
            Self::OriginAccessIdentity => "CloudFrontOriginAccessIdentityConfig",
            Self::PublicKey => "PublicKeyConfig",
            Self::KeyGroup => "KeyGroupConfig",
        }
    }

    fn not_found(self) -> &'static str {
        match self {
            Self::OriginAccessIdentity => NO_SUCH_ORIGIN_ACCESS_IDENTITY,
            Self::PublicKey => NO_SUCH_PUBLIC_KEY,
            Self::KeyGroup => NO_SUCH_KEY_GROUP,
        }
    }

    fn record_uri(self, id: &str) -> String {
        format!("{}/{}", self.collection(), percent_encoded(id))
    }
}

/// Call one CloudFront operation in process, answering the document and the `ETag` header.
pub(super) async fn call_cloudfront(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    method: &'static str,
    uri: &str,
    body: Option<&str>,
    if_match: Option<&str>,
) -> Result<(String, Option<String>), Refusal> {
    call_rest_xml_versioned(
        ctx,
        CLOUDFRONT_SIGNING_NAME,
        region,
        method,
        &format!("{CLOUDFRONT_URI_PREFIX}{uri}"),
        body,
        if_match,
    )
    .await
}

/// The configuration a declaration states, rendered as CloudFront's request document.
///
/// `CallerReference` is the one member CloudFormation decides when a declaration leaves it
/// out (the identity's config does not publish one at all; a public key's may): a digest of
/// the stack and the logical id, so a retried create sends the reference the first one did.
/// A key group's `Items` is CloudFormation's plain list of public key ids, which CloudFront
/// spells `<Items><PublicKey>id</PublicKey>…</Items>`.
fn config_document(
    kind: CloudFrontKind,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
    held_reference: Option<&str>,
) -> Result<String, Refusal> {
    let config = declared.get(kind.config_property()).ok_or_else(|| Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "Property {} cannot be empty for resource of type {}",
            kind.config_property(),
            resource.resource_type
        ),
    })?;
    let text = |name: &str| config.get(name).and_then(scalar_text);
    let mut members = String::new();
    match kind {
        CloudFrontKind::OriginAccessIdentity => {
            let reference = held_reference.map(ToString::to_string).unwrap_or_else(|| {
                route53_caller_reference(
                    ORIGIN_ACCESS_IDENTITY_CALLER_REFERENCE_SALT,
                    resource,
                    propagated,
                )
            });
            members.push_str(&xml_leaf("CallerReference", &reference));
            members.push_str(&xml_leaf("Comment", &text("Comment").unwrap_or_default()));
        }
        CloudFrontKind::PublicKey => {
            let reference = held_reference
                .map(ToString::to_string)
                .or_else(|| text("CallerReference"))
                .unwrap_or_else(|| {
                route53_caller_reference(PUBLIC_KEY_CALLER_REFERENCE_SALT, resource, propagated)
            });
            members.push_str(&xml_leaf("CallerReference", &reference));
            members.push_str(&xml_leaf("Name", &text("Name").unwrap_or_default()));
            members.push_str(&xml_leaf("EncodedKey", &text("EncodedKey").unwrap_or_default()));
            if let Some(comment) = text("Comment") {
                members.push_str(&xml_leaf("Comment", &comment));
            }
        }
        CloudFrontKind::KeyGroup => {
            members.push_str(&xml_leaf("Name", &text("Name").unwrap_or_default()));
            let items: String = config
                .get("Items")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(scalar_text)
                        .map(|id| xml_leaf("PublicKey", &id))
                        .collect()
                })
                .unwrap_or_default();
            members.push_str(&format!("<Items>{items}</Items>"));
            if let Some(comment) = text("Comment") {
                members.push_str(&xml_leaf("Comment", &comment));
            }
        }
    }
    let root = kind.config_property();
    Ok(format!(
        "<{root} xmlns=\"{CLOUDFRONT_XML_NAMESPACE}\">{members}</{root}>"
    ))
}

/// Create the resource a CloudFront identity declaration describes, answering the id
/// CloudFront minted as the physical id, with the attributes `Fn::GetAtt` reads.
pub(super) async fn create_cloudfront_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: CloudFrontKind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let body = config_document(kind, resource, &resource.properties, propagated, None)?;
    let (answered, _etag) =
        call_cloudfront(ctx, region, "POST", kind.collection(), Some(&body), None).await?;
    let reported = xml_scalar_children(&answered, kind.answer_element());
    let id = reported
        .get("Id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!(
                "CloudFront answered no {}.Id for {}",
                kind.answer_element(),
                resource.logical_id
            ),
        })?
        .to_string();
    let mut attributes = BTreeMap::new();
    attributes.insert("Id".to_string(), id.clone());
    // "Fn::GetAtt returns … S3CanonicalUserId: The Amazon S3 canonical user ID for the origin
    // access identity"; a public key's `CreatedTime` is the instant CloudFront recorded.
    for member in ["S3CanonicalUserId", "CreatedTime"] {
        if let Some(value) = reported.get(member).and_then(Value::as_str) {
            attributes.insert(member.to_string(), value.to_string());
        }
    }
    Ok(Created {
        physical_resource_id: Some(id),
        attributes,
    })
}

/// Bring a CloudFront identity resource to what `declared` states: read its `ETag` and the
/// caller reference it holds, then PUT the whole configuration back under that `ETag`
/// (CloudFront replaces the configuration it holds).
pub(super) async fn update_cloudfront_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: CloudFrontKind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<(), Refusal> {
    let id = &resource.physical_resource_id;
    let (record, etag) =
        call_cloudfront(ctx, region, "GET", &kind.record_uri(id), None, None).await?;
    // A caller reference cannot change on an update, so the one CloudFront holds is sent back.
    let held = xml_element(&record, "CallerReference");
    let body = config_document(kind, resource, declared, &[], held.as_deref())?;
    call_cloudfront(
        ctx,
        region,
        "PUT",
        &format!("{}{}", kind.record_uri(id), kind.config_suffix()),
        Some(&body),
        etag.as_deref(),
    )
    .await?;
    Ok(())
}

/// Delete a CloudFront identity resource a stack created: read its current `ETag`, then
/// DELETE under it. A resource CloudFront no longer holds is what a delete asks for.
pub(super) async fn delete_cloudfront_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: CloudFrontKind,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let uri = kind.record_uri(physical_resource_id);
    let etag = match call_cloudfront(ctx, region, "GET", &uri, None, None).await {
        Ok((_record, etag)) => etag,
        Err(refusal) if refusal.code == kind.not_found() => return Ok(()),
        Err(refusal) => return Err(refusal),
    };
    match call_cloudfront(ctx, region, "DELETE", &uri, None, etag.as_deref()).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == kind.not_found() => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// The resource's record as CloudFront reports it, for drift detection: the scalar members of
/// its answer element, or `None` when CloudFront no longer holds it.
pub(super) async fn describe_cloudfront_resource(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: CloudFrontKind,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    match call_cloudfront(
        ctx,
        region,
        "GET",
        &kind.record_uri(physical_resource_id),
        None,
        None,
    )
    .await
    {
        Ok((record, _etag)) => Ok(Some(Value::Object(xml_scalar_children(
            &record,
            kind.answer_element(),
        )))),
        Err(refusal) if refusal.code == kind.not_found() => Ok(None),
        Err(refusal) => Err(refusal),
    }
}
