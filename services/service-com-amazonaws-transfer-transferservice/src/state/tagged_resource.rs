// SPDX-License-Identifier: CC0-1.0

//! WHICH kind of resource an ARN names, for the operations of the tag plane — the only
//! ones that address a resource by its ARN rather than by its own identifier.
//!
//! `ResourceNotFoundException` models `Message`, `Resource` and `ResourceType` as all
//! required precisely so a caller can tell which kind of object was missing, so a
//! refusal that names one kind for every ARN tells a caller tagging several kinds
//! nothing. The kind is decoded from the ARN itself: its resource field states the
//! resource type and the identifier of the resource
//! (`server/<ServerId>`, `user/<ServerId>/<UserName>`), and each kind already has its
//! `ResourceType` and its fixed `Message` in this service's not-found table
//! ([`super::NotFoundResource`]) — the same entry the kind's OWN operations answer
//! with, so `TagResource` on a missing server and `DescribeServer` on the same id
//! report the same kind in the same words.
//!
//! An ARN of a kind this table does not list — another service's ARN, or a Transfer
//! resource kind nothing here holds — names no kind this service can report, so it
//! keeps the generic entry ([`super::NOT_FOUND_TAGGED_RESOURCE`]) and the whole ARN in
//! `Resource`: there is no identifier to decode out of a resource field this service
//! does not write.

use super::*;

/// Each resource kind of this service as its ARN names it: the resource-type segment
/// the ARN builder writes ([`super::server_arn`], [`super::user_arn`],
/// [`super::host_key_arn`], [`super::web_app_arn`], [`super::profile_arn`],
/// [`super::connector_arn`], [`super::workflow_arn`], [`super::agreement_arn`],
/// [`super::certificate_arn`]), with that kind's entry in the not-found table.
///
/// It is the same list of kinds
/// [`ComAmazonawsTransferTransferserviceState::taggable`] resolves an ARN against — a
/// kind becomes taggable by storing an `arn` and a `tags` and being added to both — so
/// every ARN the tag plane can fail to resolve has an entry here.
const ARN_RESOURCE_KINDS: &[(&str, &NotFoundResource)] = &[
    ("server", &NOT_FOUND_SERVER),
    ("user", &NOT_FOUND_USER),
    ("host-key", &NOT_FOUND_HOST_KEY),
    ("webapp", &NOT_FOUND_WEB_APP),
    ("profile", &NOT_FOUND_PROFILE),
    ("connector", &NOT_FOUND_CONNECTOR),
    ("workflow", &NOT_FOUND_WORKFLOW),
    ("agreement", &NOT_FOUND_AGREEMENT),
    ("certificate", &NOT_FOUND_CERTIFICATE),
];

/// The kind `arn` names and the identifier it addresses that resource by, or `None`
/// when the value is not an ARN of a resource kind this service holds.
///
/// The identifier is the LAST segment of the ARN's resource field, which is how every
/// one of this service's ARN shapes addresses its resource: the id alone for an
/// account-level resource (`profile/<ProfileId>`) and the name or id under its parent
/// for a resource a server owns (`user/<ServerId>/<UserName>`,
/// `host-key/<ServerId>/<HostKeyId>`). That is the SAME value the kind's own
/// operations put in `Resource`, so one resource is reported under one identifier
/// whichever operation failed to find it.
pub fn arn_resource_kind(arn: &str) -> Option<(&'static NotFoundResource, &str)> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != ARN_SERVICE {
        return None;
    }
    let (_, kind) = ARN_RESOURCE_KINDS
        .iter()
        .find(|(segment, _)| *segment == parsed.resource_type())?;
    let identifier = parsed.resource.rsplit('/').next()?;
    (!identifier.is_empty()).then_some((kind, identifier))
}

/// The not-found payload for a tag-plane request whose `Arn` names no stored resource:
/// the kind the ARN names, reported with that kind's own `ResourceType` and fixed
/// `Message` and with the resource's own IDENTIFIER in `Resource`.
///
/// Built through [`super::resource_not_found`] like every other not-found in this
/// service, so the identifier travels in `Resource` alone and the `Message` is the
/// fixed text for the kind. An ARN whose kind this service does not hold keeps the
/// generic entry and the whole ARN, since no identifier can be read out of it.
pub fn unknown_tagged_resource(arn: &str) -> ResourceNotFoundException {
    match arn_resource_kind(arn) {
        Some((kind, identifier)) => resource_not_found(kind, identifier),
        None => resource_not_found(&NOT_FOUND_TAGGED_RESOURCE, arn),
    }
}
