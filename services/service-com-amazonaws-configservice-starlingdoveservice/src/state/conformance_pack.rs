// SPDX-License-Identifier: CC0-1.0

//! One stored conformance pack: the pack as it was put and the projections the
//! conformance-pack read paths report it through.

use super::*;

/// One conformance pack deployed in this account and Region.
///
/// A conformance pack is deployed as a CloudFormation stack, which is why the pack
/// carries both its own ARN and the stack's ([`ConformancePackStatusDetail$StackArn`]).
/// Its deployment is NOT finished when `PutConformancePack` answers: the pack is
/// `CREATE_IN_PROGRESS` until the deployment is due (see
/// [`CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER`]) and `CREATE_COMPLETE` from then on. The
/// entry therefore stores the two INSTANTS and derives the state from the clock, so no
/// read has to move it and two reads at the same instant agree.
///
/// What the deployment CREATED is part of the pack: the Config rules its template
/// declares are written into
/// [`ComAmazonawsConfigserviceStarlingdoveserviceState::config_rules`] (and their
/// remediation configurations beside them) and named here in
/// [`Self::deployed_rule_names`], so a pack reported complete is a pack whose resources
/// exist; a deployment that could not create them carries [`Self::failure_reason`] and
/// is reported `CREATE_FAILED` instead.
///
/// [`ConformancePackStatusDetail$StackArn`]: ConformancePackStatusDetail
#[derive(Debug, Clone)]
pub struct ConformancePackEntry {
    /// `ConformancePackName`, which is unique per account per Region and is what every
    /// conformance pack operation names the pack by.
    pub name: String,
    /// The `ConformancePackId` Config assigned the pack when it was created; it is the
    /// last segment of [`Self::arn`] and never changes once minted.
    pub id: String,
    pub arn: String,
    /// The ARN of the CloudFormation stack the pack is deployed as.
    pub stack_arn: String,
    /// The template the pack was last deployed from, byte for byte as it was sent, and
    /// the Amazon S3 location Config stores conformance pack templates at. Both are
    /// replaced by a later `PutConformancePack` for the same name, and
    /// `DescribeConformancePacks` is what reports the pack's stored configuration back.
    pub template_body: Option<String>,
    pub delivery_s3_bucket: Option<String>,
    pub delivery_s3_key_prefix: Option<String>,
    /// The `ConformancePackInputParameter` objects the pack was deployed with.
    pub input_parameters: Vec<ConformancePackInputParameter>,
    /// The Config rules this pack's deployment CREATED, by the names Config gave them
    /// ([`conformance_pack_rule_name`]).
    ///
    /// They are the pack's own resources, so a redeploy of this pack replaces exactly
    /// these and nothing else in
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::config_rules`]. A pack whose
    /// deployment failed ([`Self::failure_reason`]) deployed none, so this is empty.
    pub deployed_rule_names: Vec<String>,
    /// Why the pack's deployment failed, when it did
    /// (`ConformancePackStatusDetail$ConformancePackStatusReason`: "The reason of
    /// conformance pack creation failure"), which is what puts the pack in
    /// [`CONFORMANCE_PACK_STATE_CREATE_FAILED`] rather than reporting a deployment that
    /// created nothing as complete.
    pub failure_reason: Option<String>,
    /// "Last time when conformation pack creation and update was requested" — the
    /// instant of the call that last deployed this pack.
    pub last_update_requested_at: SystemTime,
    /// When that deployment is due to be complete; see
    /// [`CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER`].
    pub deployment_completes_at: SystemTime,
    /// Tags are taken at creation and not updated by `PutConformancePack` ("Tags are
    /// added at creation and cannot be updated with this operation"); they are this
    /// resource's one tag set, reached through
    /// [`ComAmazonawsConfigserviceStarlingdoveserviceState::tags_of`].
    pub tags: TagSet,
}

impl ConformancePackEntry {
    /// The `ConformancePackState` the pack is in at `now`: a deployment that could not
    /// create the resources the template declares FAILED, and otherwise it is in
    /// progress until the instant it is due to complete, and complete from then on.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.failure_reason.is_some() {
            CONFORMANCE_PACK_STATE_CREATE_FAILED
        } else if now >= self.deployment_completes_at {
            CONFORMANCE_PACK_STATE_CREATE_COMPLETE
        } else {
            CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS
        }
    }

    /// The modelled `ConformancePackStatusDetail`, read back out of state at `now`.
    ///
    /// `ConformancePackStatusReason` is "the reason of conformance pack creation
    /// failure", which a deployment that is in progress or complete has none of, and
    /// `LastUpdateCompletedTime` is "last time when conformation pack creation and
    /// update was successful", which neither a deployment still in progress nor a failed
    /// one has reached.
    pub fn to_status_detail(&self, now: SystemTime) -> ConformancePackStatusDetail {
        let complete = self.state_at(now) == CONFORMANCE_PACK_STATE_CREATE_COMPLETE;
        ConformancePackStatusDetail {
            conformance_pack_name: self.name.clone(),
            conformance_pack_id: self.id.clone(),
            conformance_pack_arn: self.arn.clone(),
            conformance_pack_state: self.state_at(now).to_string(),
            stack_arn: self.stack_arn.clone(),
            conformance_pack_status_reason: self.failure_reason.clone(),
            last_update_requested_time: timestamp(self.last_update_requested_at),
            last_update_completed_time: complete
                .then(|| timestamp(self.deployment_completes_at)),
        }
    }

    /// The modelled `ConformancePackDetail` — the pack's stored CONFIGURATION, as
    /// `DescribeConformancePacks` reports it, which is what the deployment was last
    /// requested with rather than how far that deployment has got
    /// ([`Self::to_status_detail`] is what reports the latter).
    ///
    /// `CreatedBy` is "the Amazon Web Services service that created the conformance
    /// pack", which a pack the account deployed itself through `PutConformancePack` has
    /// none of, and `TemplateSSMDocumentDetails` is carried only by a pack deployed from
    /// a Systems Manager document, which this store holds none of: `PutConformancePack`
    /// refuses that leg rather than deploying from a template it never read.
    pub fn to_detail(&self) -> ConformancePackDetail {
        ConformancePackDetail {
            conformance_pack_name: self.name.clone(),
            conformance_pack_arn: self.arn.clone(),
            conformance_pack_id: self.id.clone(),
            delivery_s3_bucket: self.delivery_s3_bucket.clone(),
            delivery_s3_key_prefix: self.delivery_s3_key_prefix.clone(),
            conformance_pack_input_parameters: Some(self.input_parameters.clone()),
            last_update_requested_time: Some(timestamp(self.last_update_requested_at)),
            created_by: None,
            template_ssm_document_details: None,
        }
    }
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Drop the resources ONE conformance pack's deployment created, named by
    /// [`ConformancePackEntry::deployed_rule_names`].
    ///
    /// A pack's resources are the Config rules its template declared and the remediation
    /// actions declared for them, so this is the ONE statement of what goes when a pack's
    /// deployment is undone: `DeleteConformancePack` removes them with the pack ("all the
    /// Config rules, remediation actions, and all evaluation results within that
    /// conformance pack" — a rule's evaluation results live on the rule, so removing the
    /// rule removes them), and a REDEPLOY of the same pack removes exactly these before
    /// creating what the new template declares. Nothing else in the account's rules is
    /// touched.
    pub fn remove_conformance_pack_resources(&mut self, rule_names: &[String]) {
        for rule_name in rule_names {
            self.config_rules.remove(rule_name);
            self.remediation_configurations.remove(rule_name);
        }
    }
}
