// SPDX-License-Identifier: CC0-1.0

//! Which managed permissions govern one resource share, and how a read of the share's
//! permissions projects them.
//!
//! RAM governs a resource share by MANAGED PERMISSIONS, and which ones a share is
//! governed by is state the share carries rather than something derivable from the
//! create request alone: the permissions that request named, plus the ones
//! `AssociateResourceSharePermission` attached afterwards ("Adds or replaces the RAM
//! permission for a resource type included in a resource share"), plus — for every
//! resource type the share shares that none of those applies to — the one permission
//! Amazon Web Services publishes as that resource type's default.
//!
//! An attachment is stored per `(share, permission ARN)` pair with the version of the
//! permission that was attached, so a read reports the version the share is actually
//! governed by.

use super::{
    default_aws_managed_permission, resource_type_matches,
    ComAmazonawsRamAmazonresourcesharingState, PermissionRecord, PermissionVersionRecord,
    ShareRecord, ASSOCIATION_STATUS_ASSOCIATED, FIRST_PERMISSION_VERSION,
};
use crate::ops::types::ResourceSharePermissionSummary;

/// The `status` a managed permission reports when it is read as one of the permissions
/// ATTACHED TO A RESOURCE SHARE.
///
/// `ResourceSharePermissionSummary$status` is "The current status of the permission"
/// and is modelled as a plain string rather than as the `PermissionStatus` enum
/// (`smithy_query(query="shape",shape="ResourceSharePermissionSummary")`): read through
/// a share, what is current about the permission is where its attachment to THAT share
/// has got to, so a permission currently attached reports that it is associated with
/// it. A read of a permission in its own right (`GetPermission`, `CreatePermission`)
/// reports the version's own `PermissionStatus`
/// ([`super::PERMISSION_STATUS_ATTACHABLE`]) instead, which is a different question.
///
/// The word is the one the association statuses already carry, so it is taken from
/// there rather than spelled a second time.
pub const SHARE_PERMISSION_STATUS_ASSOCIATED: &str = ASSOCIATION_STATUS_ASSOCIATED;

/// One managed permission attached to one resource share, at the version that was
/// attached.
#[derive(Debug, Clone, Default)]
pub struct SharePermissionRecord {
    /// The permission's ARN, which is also the key this record is held under on the
    /// share.
    pub permission_arn: String,
    /// The version of the permission the share is governed by. A permission's default
    /// version can move after the attachment was made, and the share keeps the version
    /// it attached until an attach replaces it — which is what lets
    /// `ResourceSharePermissionSummary$defaultVersion` report false for an outdated
    /// attachment.
    pub version: i32,
}

impl ShareRecord {
    /// Whether this share has the permission `permission_arn` attached to it by name.
    ///
    /// A resource type's default managed permission is NOT attached by name: it follows
    /// from what the share shares, so
    /// [`ComAmazonawsRamAmazonresourcesharingState::share_permissions`] derives it and
    /// this answers false for one.
    pub fn attaches_permission(&self, permission_arn: &str) -> bool {
        self.attached_permissions.contains_key(permission_arn)
    }

    /// Attach `attachment` to this share, replacing whatever was attached under the
    /// same permission ARN — an attach of a permission the share already holds moves
    /// the share onto the version this one names and adds nothing.
    pub fn attach_permission(&mut self, attachment: SharePermissionRecord) {
        self.attached_permissions
            .insert(attachment.permission_arn.clone(), attachment);
    }

    /// Detach the permission `permission_arn` from this share, and answer whether it
    /// was attached (and so whether the attached set changed).
    pub fn detach_permission(&mut self, permission_arn: &str) -> bool {
        self.attached_permissions.remove(permission_arn).is_some()
    }
}

/// One managed permission as one resource share holds it: the permission record and the
/// version of it that share is governed by.
#[derive(Debug, Clone, Copy)]
pub struct AttachedPermission<'a> {
    /// The permission the share is governed by.
    pub permission: &'a PermissionRecord,
    /// The version of it the share attached.
    pub version: &'a PermissionVersionRecord,
}

impl AttachedPermission<'_> {
    /// The attached permission's ARN, which a paginated read pages over.
    pub fn arn(&self) -> &str {
        &self.permission.arn
    }

    /// This attachment as the modelled `ResourceSharePermissionSummary` wire shape: the
    /// permission at the version the share attached, whose `status` reports the
    /// attachment ([`SHARE_PERMISSION_STATUS_ASSOCIATED`]) rather than the version's own
    /// attachability.
    pub fn summary(&self) -> ResourceSharePermissionSummary {
        ResourceSharePermissionSummary {
            status: Some(SHARE_PERMISSION_STATUS_ASSOCIATED.to_string()),
            ..self.permission.summary(self.version)
        }
    }
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// The attachment to record for `permission_arn`: the permission's CURRENT default
    /// version, because "the default behavior is to use the version that is currently
    /// set as the default version for the permission" and a request may name "only the
    /// version that is currently set as the default version"
    /// (`AssociateResourceSharePermission$permissionVersion`). The number is resolved
    /// when the attachment is made and kept, so an attachment made before the
    /// permission's default version moved still reports the version it attached.
    ///
    /// An ARN this store holds no permission for still yields a record — the share
    /// carries what the request named, and a read reports only the permissions that
    /// resolve.
    pub fn permission_attachment(&self, permission_arn: &str) -> SharePermissionRecord {
        let version = self
            .permission(permission_arn)
            .map_or(FIRST_PERMISSION_VERSION, |permission| {
                permission.default_version
            });
        SharePermissionRecord {
            permission_arn: permission_arn.to_string(),
            version,
        }
    }

    /// The permissions attached to the share `share_arn` by name that apply to the same
    /// resource type as `permission_arn` and are not `permission_arn` itself, in
    /// permission ARN order.
    ///
    /// "A resource share can have only one permission per resource type"
    /// (`AssociateResourceSharePermission$replace`), so these are the attachments an
    /// attach of `permission_arn` collides with: an attach that replaces detaches them,
    /// and one that does not is refused. Resource types are matched the way
    /// [`resource_type_matches`] matches them, so the two spellings of one type collide.
    pub fn colliding_permissions(&self, share_arn: &str, permission_arn: &str) -> Vec<String> {
        let (Some(resource_type), Some(share)) = (
            self.permission(permission_arn)
                .map(|permission| permission.resource_type.clone()),
            self.share(share_arn),
        ) else {
            return Vec::new();
        };
        share
            .attached_permissions
            .keys()
            .filter(|attached| attached.as_str() != permission_arn)
            .filter(|attached| {
                self.permission(attached)
                    .is_some_and(|other| resource_type_matches(&other.resource_type, &resource_type))
            })
            .cloned()
            .collect()
    }

    /// Attach the managed permission `permission_arn` to the live share `share_arn` at
    /// the permission's default version, detaching every permission named in `replaced`
    /// first, and answer whether the share took the attachment.
    ///
    /// `false` means no live share has that ARN, so nothing was attached.
    pub fn attach_share_permission(
        &mut self,
        share_arn: &str,
        permission_arn: &str,
        replaced: &[String],
    ) -> bool {
        let attachment = self.permission_attachment(permission_arn);
        let Some(share) = self.live_share_mut(share_arn) else {
            return false;
        };
        for detached in replaced {
            share.detach_permission(detached);
        }
        share.attach_permission(attachment);
        true
    }

    /// Every managed permission attached to the share `arn` names, in permission ARN
    /// order — the order a paginated read of them pages in — and each of them once.
    /// Empty for a share this store does not hold.
    ///
    /// A share's permissions are the ones attached to it by name — those the create
    /// request named plus those `AssociateResourceSharePermission` attached, each at the
    /// version that attachment carries — plus the DEFAULT managed permission of every
    /// resource type it shares that none of those applies to: a managed permission
    /// flagged `isResourceTypeDefault` is "the default managed permission for all
    /// resources of this resource type"
    /// (`ResourceSharePermissionSummary$isResourceTypeDefault`), the one RAM attaches
    /// when the share names none for the type, so a share created with a resource and no
    /// permission at all is still governed by one. A permission ARN no permission in this
    /// store answers to names nothing to report, and neither does an attachment whose
    /// permission carries no version at all.
    pub fn share_permissions(&self, arn: &str) -> Vec<AttachedPermission<'_>> {
        let Some(share) = self.share(arn) else {
            return Vec::new();
        };
        let mut attached: Vec<AttachedPermission<'_>> = share
            .attached_permissions
            .values()
            .filter_map(|attachment| {
                let permission = self.permissions.get(&attachment.permission_arn)?;
                // The version the share attached; its default version when the store no
                // longer holds that one, so the share still reports the permission that
                // governs it.
                let version = permission
                    .version(Some(attachment.version))
                    .or_else(|| permission.version(None))?;
                Some(AttachedPermission {
                    permission,
                    version,
                })
            })
            .collect();
        for resource_type in self.shared_resource_types(arn) {
            if attached.iter().any(|holder| {
                resource_type_matches(&holder.permission.resource_type, &resource_type)
            }) {
                continue;
            }
            let default = default_aws_managed_permission(&resource_type)
                .and_then(|entry| self.permissions.get(&entry.arn()));
            if let Some(permission) = default {
                if let Some(version) = permission.version(None) {
                    attached.push(AttachedPermission {
                        permission,
                        version,
                    });
                }
            }
        }
        attached.sort_unstable_by(|left, right| left.arn().cmp(right.arn()));
        attached.dedup_by(|left, right| left.arn() == right.arn());
        attached
    }
}

/// The message of the `InvalidParameterException` an attach renders when the share
/// already has a DIFFERENT permission attached for the same resource type and the
/// request does not replace it.
///
/// "A resource share can have only one permission per resource type. If a resource
/// share already has a permission for the specified resource type and you don't set
/// replace to true then the operation returns an error"
/// (`AssociateResourceSharePermission$replace`), so the message names the resource type
/// that is already governed, the permission governing it, and the parameter that would
/// replace it.
pub fn permission_already_attached_message(
    share_arn: &str,
    resource_type: &str,
    attached: &str,
) -> String {
    format!(
        "Resource share {share_arn} already has permission {attached} associated for \
         resource type {resource_type}. Set replace to true to replace it."
    )
}

/// The message of the `InvalidParameterException` an attach renders when the request
/// names a version of the permission that is not its default one.
///
/// "You can specify only the version that is currently set as the default version for
/// the permission. If you also set the replace pararameter to true, then this operation
/// updates an outdated version of the permission to the current default version"
/// (`AssociateResourceSharePermission$permissionVersion`), so the message names the
/// version that was asked for, the default version it is not, and the parameter that
/// would update the share to that default.
pub fn not_default_permission_version_message(
    permission_arn: &str,
    requested: i32,
    default_version: i32,
) -> String {
    format!(
        "permissionVersion {requested} is not the default version of permission \
         {permission_arn}, which is version {default_version}. Specify the default \
         version, or set replace to true to update the resource share to it."
    )
}
