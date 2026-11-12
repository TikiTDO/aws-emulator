// SPDX-License-Identifier: CC0-1.0

//! The partner-AI-app slice of this service's state: the [`PartnerApp`] record and its
//! settings, the id and base URL it is reached through, its bound and clock-owed span,
//! and the sentences a missing or unsubscribed app is refused with.

use super::*;

/// `PartnerAppStatus`'s members this emulator moves a partner AI app through (both are
/// members of `crate::ops::types::PARTNER_APP_STATUS_VALUES`; the modelled enum also
/// carries the update, failure and delete states, which this slice does not reach).
/// A separate pair from [`STATUS_CREATING`], which names `EndpointStatus`'s members.
pub const PARTNER_APP_STATUS_CREATING: &str = "Creating";
/// See [`PARTNER_APP_STATUS_CREATING`].
pub const PARTNER_APP_STATUS_AVAILABLE: &str = "Available";

/// `ListPartnerApps`' documented `MaxResults` default, applied when the caller names
/// none: "The default value for MaxResults is 10."
/// (`smithy_query(query="doc",shape="ListPartnerAppsRequest",member="MaxResults")`).
/// [`MAX_PAGE_SIZE`] still caps the page above it, as it does for every listing here.
pub const PARTNER_APP_DEFAULT_PAGE_SIZE: i32 = 10;

/// See [`MODEL_QUOTA`]. Bounds the partner-app store, and is what `CreatePartnerApp`
/// answers `ResourceLimitExceeded` past.
pub const PARTNER_APP_QUOTA: usize = 1_000;

/// How long after `CreatePartnerApp` the clock owes the `Creating` -> `Available`
/// transition of `PartnerAppStatus`.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that lists or describes the
/// app straight after creating it still observes `Creating` (and the state refusals that
/// go with it), one that waits observes `Available`. Real SageMaker takes minutes to
/// provision a partner application; the span is compressed, never zero, and the due
/// instant is stored rather than slept on.
pub const PARTNER_APP_AVAILABLE_AFTER: Duration = ENDPOINT_IN_SERVICE_AFTER;

/// Where the partner app `app_id` is served in `region`: the host SageMaker gives one
/// partner AI app, which `DescribePartnerApp` reports as `BaseUrl` and
/// `CreatePartnerAppPresignedUrl` builds its URL on. The app's own id is what makes it
/// unique, so the host carries it.
pub fn partner_app_base_url(region: &str, app_id: &str) -> String {
    format!("https://{app_id}.{region}.partner-app.sagemaker.aws")
}

/// A fresh partner-app id: `app-` and the twelve upper-case characters the modelled
/// `PartnerAppArn` pattern admits after it (`partner-app\/app-[A-Z0-9]{12}`).
///
/// The caller never supplies it — a partner app is addressed by its ARN, so the id is the
/// server's to generate, and it is derived from nothing in the request. Same scheme as
/// every other id in this crate ([`mint_short_id`]), upper-cased to the alphabet this
/// pattern admits.
pub fn mint_partner_app_id() -> String {
    format!("app-{}", mint_short_id().to_ascii_uppercase())
}

/// How the partner-app operations state an app this account holds none of.
///
/// A FIXED sentence carrying NOTHING from the request: a partner AI app is addressed by
/// its ARN, and the service names no identifier here, so the refusal does not echo back
/// the ARN a caller probed with. Stated once here so the reader (`DescribePartnerApp`) and
/// the URL mint (`CreatePartnerAppPresignedUrl`) cannot word the same fact differently.
pub const PARTNER_APP_NOT_FOUND: &str = "App entity does not exist.";

/// How `CreatePartnerApp` refuses a `Type` this account holds no partner-application
/// subscription for.
///
/// A partner AI app runs a third party's application, which is licensed to the account
/// through AWS Marketplace rather than through this API — so a type the account has not
/// subscribed to cannot be provisioned. The sentence is FIXED text carrying nothing from
/// the request: the service states that the caller holds no entitlement, not which
/// offering was asked for, so the refusal does not echo the `Type` back. See
/// [`ComAmazonawsSagemakerSagemakerState::subscribed_to_partner_app`].
pub const PARTNER_APP_NOT_SUBSCRIBED: &str = "Caller is not subscribed to the marketplace offering.";

/// The configuration one partner AI app carries: everything `CreatePartnerApp` is given
/// about HOW the app runs, as opposed to the identity SageMaker mints for it.
///
/// Kept as one record so `DescribePartnerApp` reports exactly what the create stored, and
/// so `UpdatePartnerApp` — which takes the same members again — replaces the
/// configuration without touching the app's identity, status or grants.
#[derive(Debug, Clone, Default)]
pub struct PartnerAppSettings {
    /// `ExecutionRoleArn` — the IAM role the partner application uses.
    pub execution_role_arn: String,
    /// `Tier` — the instance type and size of the cluster attached to the app.
    pub tier: String,
    /// `AuthType` — the authorization type users access the app with.
    pub auth_type: String,
    /// `KmsKeyId`, when the request named a customer managed key.
    pub kms_key_id: Option<String>,
    pub maintenance_config: Option<crate::ops::types::PartnerAppMaintenanceConfig>,
    pub application_config: Option<crate::ops::types::PartnerAppConfig>,
    /// `IdcConfig` — the Identity Center instance an `IDC` app federates with.
    pub idc_config: Option<crate::ops::types::IdcConfigInput>,
    pub enable_iam_session_based_identity: Option<bool>,
    pub enable_auto_minor_version_upgrade: Option<bool>,
}

/// A SageMaker Partner AI App, as `ListPartnerApps` summarises it and
/// `DescribePartnerApp` reports it.
///
/// Keyed in state by [`PartnerApp::app_id`] (the ARN's `app-...` resource id), which AWS
/// mints and every later call addresses the app by. `CreatePartnerApp` fills the store and
/// `ListPartnerApps` pages whatever it holds; the updating and deleting handlers extend
/// [`ComAmazonawsSagemakerSagemakerState::partner_apps`] rather than adding a second map.
///
/// `available_at` is the instant the clock owes the `Creating` -> `Available` transition;
/// it is stored, never slept on, and materialised by whichever reporting call first runs
/// at or after it.
#[derive(Debug, Clone)]
pub struct PartnerApp {
    pub app_id: String,
    pub arn: String,
    pub name: String,
    pub app_type: String,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub available_at: SystemTime,
    /// Where the app itself is served; the presigned URL
    /// `CreatePartnerAppPresignedUrl` answers is this plus the grant it minted.
    pub base_url: String,
    /// How the app runs, as the create was given it. See [`PartnerAppSettings`].
    pub settings: PartnerAppSettings,
    /// The `ClientToken` the create that minted this app presented, if any, so a retried
    /// create answers the app it already made instead of a second one.
    pub client_token: Option<String>,
    /// The `Tags` the create carried, which the tag operations read and write.
    pub tags: Vec<Tag>,
    /// Every presigned-URL grant minted for this app that has not expired, oldest
    /// first. See [`PresignedUrlGrant`].
    pub grants: Vec<PresignedUrlGrant>,
}

impl PartnerApp {
    /// Materialise the `Creating` -> `Available` transition the clock already owes, the
    /// same way [`Endpoint::settle_status`] does for an endpoint: it was due at
    /// `available_at` whether or not anybody listed the apps, so the observer makes no
    /// difference to the result, and `LastModifiedTime` moves to the instant the
    /// transition was due rather than to "now".
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == PARTNER_APP_STATUS_CREATING && now >= self.available_at {
            self.status = PARTNER_APP_STATUS_AVAILABLE.to_string();
            self.last_modified_time = self.available_at;
            return true;
        }
        false
    }
}
