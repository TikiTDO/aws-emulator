// SPDX-License-Identifier: CC0-1.0

//! The layout this service REPORTS an attached resource policy document in.
//!
//! A resource policy travels as an opaque JSON DOCUMENT (`GetResourcePolicy`
//! "retrieves the JSON text of the resource-based policy document attached to the
//! secret", this service's Smithy model), and a service that stores one does not
//! echo the caller's bytes: it parses the document and re-serializes it, so what a
//! read answers is the one canonical rendering of the policy, whatever layout the
//! attach was written in.
//!
//! That rendering is not this service's own — it is the layout every emulated
//! service that attaches an IAM policy document reports, so it lives in the shared
//! crate ([`::service_common::iam_policy::canonical_document`], which documents the
//! layout and pins it) and is only reached from here.

/// A resource policy document in the layout this service reports it in, which is
/// the layout a secret holds it in.
///
/// Applied where a policy is attached, so no path stores a document in any other
/// layout and every reader of [`super::Secret::resource_policy`] reports the one
/// rendering: `GetResourcePolicy` answers the same text whatever layout the
/// document was submitted in, and a caller that re-attaches what it read reads the
/// same text again.
///
/// Text that is not a document this service would accept is rendered as it came;
/// which documents are acceptable is [`super::parse_resource_policy`]'s reading, and
/// it runs before this.
pub fn attached_resource_policy(document: &str) -> String {
    ::service_common::iam_policy::canonical_document(document)
}
