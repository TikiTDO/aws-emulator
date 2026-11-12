// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudwatchomni-cloudwatchomnifrontend service.
//!
//! One store holds the service's resource hierarchy — a domain, the spaces that
//! belong to it, and the access profiles and dashboards those spaces hold — plus the
//! small amount of bookkeeping the modelled contract needs:
//! a name index for the duplicate-name conflict, the idempotency-token maps for the
//! `@idempotencyToken` creates, and a monotonic sequence so list pagination is
//! deterministic across pages.
//!
//! The derivation helpers below (ids, ARNs, endpoint URLs, timestamps, the opaque
//! page token) live here rather than in one operation's file because several
//! operations read them, and because every one of them is a property of the STORE
//! rather than of a single request.

use std::collections::HashMap;

use crate::ops::types::{
    AccessGrant, AccessGrantPrincipal, AccessProfile, Alert, AlertStateInfo, Domain,
    EncryptionConfiguration, IdentityProviderConfiguration, NotificationRule, OmniDashboard,
    OmniDashboardSummary, Rule, Space, SpaceSummary, ALERT_STATE_VALUES, COMPARATOR_VALUES,
    NOTIFICATION_TARGET_TYPE_VALUES, QUERY_LANGUAGE_VALUES, THRESHOLD_MODE_VALUES,
};

/// The region a resource is recorded in when the request carries no SigV4
/// credential scope to read one from (an unsigned, in-process call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace — `awsService.arnNamespace` in the Smithy model
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "cloudwatch";

/// This service's endpoint prefix — `awsService.endpointPrefix` in the Smithy model.
pub const ENDPOINT_PREFIX: &str = "cloudwatch-omni";

/// The only value `DomainStatus` declares, so a persisted domain always reports it.
pub const DOMAIN_STATUS_ACTIVE: &str = "ACTIVE";

/// `SpaceStatus` declares ACTIVE, SUSPENDED and MOVING — none of them a transient
/// "creating" state, so a created space is recorded ready.
pub const SPACE_STATUS_ACTIVE: &str = "ACTIVE";

/// `AssumeStatus`'s value for a principal that MAY assume an access profile — its
/// model doc: "The calling principal is authorized to assume the access profile."
pub const ASSUME_STATUS_ALLOWED: &str = "ALLOWED";

/// `AssumeStatus`'s value for a principal that may NOT — its model doc: "The calling
/// principal is not authorized to assume the access profile."
pub const ASSUME_STATUS_DENIED: &str = "DENIED";

/// `AccessProfileType`'s value for a profile a customer created, which is what every
/// profile created through this API is: the model doc for the value reads "Created and
/// owned by a customer", against SERVICE_MANAGED's "Provisioned and owned by the
/// service. Cannot be deleted by customers."
pub const ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// `EncryptionStrategy`'s value for a resource encrypted with an AWS owned key.
pub const ENCRYPTION_STRATEGY_AWS_OWNED: &str = "AWS_OWNED";

/// `EncryptionStrategy`'s value for a resource encrypted with a customer managed key.
pub const ENCRYPTION_STRATEGY_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// `IdentityProvider`'s value for an AWS Identity Center domain.
pub const IDENTITY_PROVIDER_IDC: &str = "IDC";

/// The page bound a list operation applies when the request names no `maxResults`.
///
/// `ListSpaces`'s `maxResults` model doc: "The maximum number of spaces to return per
/// page. Defaults to 100." That is also the maximum its `@range(min: 1, max: 100)` allows.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// How many domains the store holds before `CreateDomain` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// CloudWatchOmni publishes an adjustable quota on domains per account per Region, and
/// this emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store rather than a claim about any account's
/// limit. It is set far above any workflow an emulator serves, and it is what stops
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::domains`] growing without
/// end. It bounds `domain_name_index` and `create_domain_tokens` with it: both are
/// keyed one-to-one to a live domain and are forgotten by `DeleteDomain`, so neither
/// can outgrow this number.
pub const DOMAIN_QUOTA: usize = 1_000;

/// How many spaces the store holds before `CreateSpace` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::spaces`] and, with it,
/// `create_space_tokens` (one entry per live space, forgotten by `DeleteSpace`) — and
/// therefore also the per-request work [`spaces_after`] and [`domain_has_spaces`] do
/// under the store lock, which is linear in the number of stored spaces.
///
/// [`spaces_after`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::spaces_after
/// [`domain_has_spaces`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::domain_has_spaces
pub const SPACE_QUOTA: usize = 10_000;

/// How many access profiles the store holds before `CreateAccessProfile` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_profiles`] and, with
/// it, `create_access_profile_tokens` (one entry per live profile) — and therefore the
/// per-request work [`access_profile_named`] does under the store lock, which is linear
/// in the number of stored profiles.
///
/// [`access_profile_named`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_profile_named
pub const ACCESS_PROFILE_QUOTA: usize = 10_000;

/// `AccessGrantType`'s value for a grant a customer created, which is what every grant
/// created through this API is: the model doc for the value reads "Created and owned by
/// a customer", against SERVICE_MANAGED's "Provisioned and owned by the service. Cannot
/// be deleted by customers."
pub const ACCESS_GRANT_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// `AccessGrantPermission`'s value whose model doc reads "Access limited to an explicit
/// list of actions, specified in scopedActions" — the one permission that cannot stand
/// without those actions.
pub const ACCESS_GRANT_PERMISSION_CUSTOM: &str = "CUSTOM";

/// `AccessGrantPrincipalType`'s value for an Identity Center user — the only principal
/// type `AccessGrantPrincipal.principalAttributes` supports ("Supported only for
/// IDC_USER principals").
pub const ACCESS_GRANT_PRINCIPAL_TYPE_IDC_USER: &str = "IDC_USER";

/// `AccessGrantPrincipalType`'s value for an access profile — its model doc is "An
/// access profile." A grant held by this principal type authorizes the profile itself,
/// which is a different thing from authorizing an alert to assume that profile.
pub const ACCESS_GRANT_PRINCIPAL_TYPE_ACCESS_PROFILE: &str = "ACCESS_PROFILE";

/// `AccessGrantPrincipalType`'s value for an alert — its model doc is "An alert.",
/// enumerated separately from ACCESS_PROFILE ("An access profile."). A grant held by
/// this principal type is what authorizes an alert to assume the access profile the
/// grant is scoped to; a grant held by any other principal type, the profile itself
/// included, is not that grant.
pub const ACCESS_GRANT_PRINCIPAL_TYPE_ALERT: &str = "ALERT";

/// The action an alert's trust grant carries to assume an access profile, named in the
/// refusal a missing trust grant earns ([`alert_trust_rejection`]): this service's IAM
/// action prefix, which is also its [`ARN_NAMESPACE`], and the action's own name.
///
/// [`alert_trust_rejection`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::alert_trust_rejection
pub const ASSUME_ACCESS_PROFILE_ACTION: &str = "cloudwatch:AssumeAccessProfile";

/// The resource type name that names an access profile in a `ResourceScope`.
///
/// `ResourceScope.resourceType`'s model doc is "Resource type name (e.g., \"DataSet\",
/// \"OmniDashboard\")" — the modelled shape's own name, so an access profile is named by
/// `AccessProfile`. It is also the `resourceType` this service's `ResourceNotFound`
/// refusals report for a profile.
pub const ACCESS_PROFILE_RESOURCE_TYPE: &str = "AccessProfile";

/// How many access grants the store holds before `CreateAccessGrant` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_grants`] and, with
/// it, `create_access_grant_tokens` (one entry per live grant) — and therefore the
/// per-request work [`access_grant_named`] and [`access_profiles_scoped_by`] do under
/// the store lock, which is linear in the number of stored grants and profiles.
///
/// [`access_grant_named`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_grant_named
/// [`access_profiles_scoped_by`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::access_profiles_scoped_by
pub const ACCESS_GRANT_QUOTA: usize = 10_000;

/// The resource type an `OmniDashboard`'s ARN carries, as the model's own
/// `CreateOmniDashboard` example documents it
/// (`arn:aws:cloudwatch:<region>:<account>:omni-dashboard/<dashboardId>`).
pub const OMNI_DASHBOARD_ARN_RESOURCE_TYPE: &str = "omni-dashboard";

/// How many dashboards the store holds before `CreateOmniDashboard` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::omni_dashboards`] and, with
/// it, `create_omni_dashboard_tokens` (one entry per live dashboard) — and therefore the
/// per-request work [`omni_dashboard_named`] does under the store lock, which is linear
/// in the number of stored dashboards.
///
/// [`omni_dashboard_named`]: ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::omni_dashboard_named
pub const OMNI_DASHBOARD_QUOTA: usize = 10_000;

/// The resource type an alert's ARN carries, as the model's own `CreateAlert` examples
/// document it (`arn:aws:cloudwatch:<region>:<account>:alert/<alertId>`).
pub const ALERT_ARN_RESOURCE_TYPE: &str = "alert";

/// `NotificationStatus`'s value for an alert whose notifications (actions) are on.
pub const NOTIFICATION_STATUS_ENABLED: &str = "ENABLED";

/// `NotificationStatus`'s value for an alert whose notifications (actions) are off.
pub const NOTIFICATION_STATUS_DISABLED: &str = "DISABLED";

/// `AlertState`'s value for an evaluation that produced no data, which `NoData`'s model
/// doc names as the default resulting state: "`treatAs` carries the resulting state;
/// defaults to `NODATA` when omitted".
pub const ALERT_STATE_NODATA: &str = "NODATA";

/// The evaluation cadences an alert's rule may ask for.
///
/// `AlertEvaluation`'s model doc: "`intervalSeconds` must be one of {30, 60, 120, 300,
/// 600, 900, 1800, 3600}".
pub const ALERT_EVALUATION_INTERVALS: &[i32] = &[30, 60, 120, 300, 600, 900, 1800, 3600];

/// How many alerts the store holds before `CreateAlert` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]. It bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::alerts`] and, with it,
/// `create_alert_tokens` (one entry per live alert, forgotten by `DeleteAlert`), so
/// neither can grow without end.
pub const ALERT_QUOTA: usize = 10_000;

/// The resource type a view's ARN carries, in this service's ARN namespace.
pub const VIEW_ARN_RESOURCE_TYPE: &str = "view";

/// The resource type a refusal about a view names.
pub const VIEW_RESOURCE_TYPE: &str = "View";

/// `ViewType`'s value for a view created through this API.
///
/// `CreateView`'s model doc: "Only USER views can be created — MANAGED views are
/// provisioned by AWS", and `ViewType`'s USER doc reads "A view created and owned by the
/// customer."
pub const VIEW_TYPE_USER: &str = "USER";

/// How many views the store holds before `CreateView` refuses.
///
/// The emulator's OWN bound, for the same reason as [`DOMAIN_QUOTA`]: it bounds
/// [`ComAmazonawsCloudwatchomniCloudwatchomnifrontendState::views`] and, with it,
/// `create_view_tokens` (one entry per live view), so neither can grow without end.
/// `CreateView` declares no quota error, so the refusal past this bound is its modelled
/// `ValidationException`.
pub const VIEW_QUOTA: usize = 10_000;

/// A dashboard as persisted. The wire `OmniDashboard` shape carries no parent id, so
/// the space the dashboard lives in is kept beside it: a dashboard is addressed by its
/// space AND its id.
#[derive(Debug, Clone)]
pub struct StoredOmniDashboard {
    /// Creation order, so `ListOmniDashboards`-style enumeration is stable.
    pub seq: u64,
    /// The id of the space this dashboard belongs to.
    pub space_id: String,
    /// Exactly what `GetOmniDashboard` returns.
    pub dashboard: OmniDashboard,
}

/// A domain as persisted, with the bookkeeping the wire shape does not carry.
#[derive(Debug, Clone)]
pub struct StoredDomain {
    /// Creation order, so `ListDomains`-style enumeration is stable.
    pub seq: u64,
    /// Exactly what `GetDomain` returns.
    pub domain: Domain,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// A space as persisted. The wire `Space` shape carries only the parent's
/// `domainArn`, so the parent's id is kept beside it for `ListSpaces`'s
/// `domainId` filter and for the `DeleteDomain` conflict check.
#[derive(Debug, Clone)]
pub struct StoredSpace {
    /// Creation order, and the cursor `ListSpaces`'s page token resumes from.
    pub seq: u64,
    /// The id of the domain this space belongs to.
    pub domain_id: String,
    /// Exactly what `GetSpace` returns.
    pub space: Space,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// An access profile as persisted. The wire `AccessProfile` shape carries the parent
/// space's id, so the record needs no separate parent field.
#[derive(Debug, Clone)]
pub struct StoredAccessProfile {
    /// Creation order, so `ListAccessProfiles`-style enumeration is stable.
    pub seq: u64,
    /// The profile as persisted. `assumeStatus` is left unset here: it reports the
    /// CALLING principal's authorization rather than a property of the profile, so it
    /// is computed when the profile is reported ([`access_profile_view`]).
    pub profile: AccessProfile,
    /// The ids of the trust grants recorded against this profile — the grants that
    /// authorize a principal to assume it. A create records none, which is why a
    /// profile reports `assumeStatus` DENIED until a grant names it.
    pub assume_grant_ids: Vec<String>,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// An access grant as persisted. The wire `AccessGrant` shape carries its own
/// `domainId` and `spaceId`, so the record needs no separate parent field.
#[derive(Debug, Clone)]
pub struct StoredAccessGrant {
    /// Creation order, so `ListAccessGrants`-style enumeration is stable.
    pub seq: u64,
    /// Exactly what `GetAccessGrant` returns.
    pub grant: AccessGrant,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// An alert as persisted. The wire `Alert` shape carries its own `spaceId` and
/// `profileId`, so the record needs no separate parent field.
///
/// `state` is left unset here: it carries LIVE evaluation state rather than a property
/// of the record, so it is computed when the alert is reported ([`alert_view`]).
#[derive(Debug, Clone)]
pub struct StoredAlert {
    /// Creation order, so `ListAlerts`-style enumeration is stable.
    pub seq: u64,
    /// The alert as persisted, `state` apart.
    pub alert: Alert,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// A view as persisted: a named, reusable SQL query of this account and region.
///
/// The model declares no single `View` structure — `CreateView`, `GetView` and
/// `UpdateView` each answer with the same flat member set, and `ViewSummary` is that set
/// without the definition — so the record holds those members once and each operation
/// projects it.
#[derive(Debug, Clone)]
pub struct StoredView {
    /// Creation order, so `ListViews`-style enumeration is stable.
    pub seq: u64,
    /// The view's name, which is its identity within the account and region.
    pub name: String,
    /// The id the backend minted for this view. No modelled member reports it on its
    /// own; it is the resource id the view's ARN carries after the name
    /// ([`view_arn`]), so a caller that parses the ARN recovers it.
    pub view_id: String,
    /// The view's ARN ([`view_arn`]).
    pub arn: String,
    /// The view's ownership category, a `ViewType` value.
    pub r#type: String,
    /// The SQL query that defines the view.
    pub definition: String,
    /// The view's description, when the request carried one.
    pub description: Option<String>,
    /// When the view was created.
    pub created_at: ::aws_smithy_types::DateTime,
    /// When the view was last updated; the creation instant until an update moves it.
    pub updated_at: ::aws_smithy_types::DateTime,
    /// The request's `tags`; no modelled operation reads them back yet.
    pub tags: HashMap<String, String>,
}

/// Per-service state for com-amazonaws-cloudwatchomni-cloudwatchomnifrontend. A handler reaches it through [`service_state`].
///
/// Every map here is BOUNDED: `domains` (and the two maps keyed one-to-one to a live
/// domain) by [`DOMAIN_QUOTA`], `spaces` (and the token map keyed to a live space) by
/// [`SPACE_QUOTA`], `access_profiles` (and the token map keyed to a live profile) by
/// [`ACCESS_PROFILE_QUOTA`], `access_grants` (and the token map keyed to a live grant)
/// by [`ACCESS_GRANT_QUOTA`], `omni_dashboards` (and the token map keyed to a live
/// dashboard) by [`OMNI_DASHBOARD_QUOTA`], `alerts` (and the token map keyed to a live
/// alert) by [`ALERT_QUOTA`], `views` (and the token map keyed to a live view) by
/// [`VIEW_QUOTA`]. The creates refuse with the modelled
/// `ServiceQuotaExceededException` once the bound is reached, and the deletes release
/// records and their tokens together, so nothing in the store grows without end.
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudwatchomniCloudwatchomnifrontendState {
    /// Every persisted domain, keyed by its `domainId`.
    pub domains: HashMap<String, StoredDomain>,
    /// `name` -> `domainId`, the index behind the duplicate-name `ConflictException`.
    pub domain_name_index: HashMap<String, String>,
    /// Every persisted space, keyed by its `spaceId`.
    pub spaces: HashMap<String, StoredSpace>,
    /// `CreateDomain` `clientToken` -> the `domainId` that token created.
    pub create_domain_tokens: HashMap<String, String>,
    /// `CreateSpace` `clientToken` -> the `spaceId` that token created.
    pub create_space_tokens: HashMap<String, String>,
    /// Every persisted access profile, keyed by its `profileId`.
    pub access_profiles: HashMap<String, StoredAccessProfile>,
    /// `CreateAccessProfile` `clientToken` -> the `profileId` that token created.
    pub create_access_profile_tokens: HashMap<String, String>,
    /// Every persisted access grant, keyed by its `grantId`.
    pub access_grants: HashMap<String, StoredAccessGrant>,
    /// `CreateAccessGrant` `clientToken` -> the `grantId` that token created.
    pub create_access_grant_tokens: HashMap<String, String>,
    /// Every persisted dashboard, keyed by its `dashboardId`.
    pub omni_dashboards: HashMap<String, StoredOmniDashboard>,
    /// `CreateOmniDashboard` `clientToken` -> the `dashboardId` that token created.
    pub create_omni_dashboard_tokens: HashMap<String, String>,
    /// Every persisted alert, keyed by its `alertId`.
    pub alerts: HashMap<String, StoredAlert>,
    /// `CreateAlert` `clientToken` -> the `alertId` that token created.
    pub create_alert_tokens: HashMap<String, String>,
    /// Every persisted view, keyed by its name — which is a view's identity, since
    /// `CreateView`'s doc reads "View names must be unique within the account and
    /// region" and every view operation addresses one by name.
    pub views: HashMap<String, StoredView>,
    /// `CreateView` `clientToken` -> the view name that token created.
    pub create_view_tokens: HashMap<String, String>,
    /// Hands out [`StoredDomain::seq`] / [`StoredSpace::seq`] / [`StoredAccessProfile::seq`]
    /// / [`StoredAccessGrant::seq`] / [`StoredOmniDashboard::seq`] / [`StoredAlert::seq`].
    next_seq: u64,
}

impl ComAmazonawsCloudwatchomniCloudwatchomnifrontendState {
    /// The next creation-order sequence number. Strictly increasing and never
    /// reused, so a page token minted from one stays valid as the store grows.
    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// Every stored space in creation order, optionally restricted to one parent
    /// domain, skipping everything at or before `after_seq`.
    ///
    /// Linear in the number of stored spaces, which [`SPACE_QUOTA`] bounds.
    pub fn spaces_after(&self, domain_id: Option<&str>, after_seq: u64) -> Vec<&StoredSpace> {
        let mut selected: Vec<&StoredSpace> = self
            .spaces
            .values()
            .filter(|stored| stored.seq > after_seq)
            .filter(|stored| domain_id.is_none_or(|id| stored.domain_id == id))
            .collect();
        selected.sort_by_key(|stored| stored.seq);
        selected
    }

    /// Every stored dashboard of one space in creation order, restricted to the names
    /// starting with `name_prefix` when the request carries one, and skipping
    /// everything at or before `after_seq`.
    ///
    /// Linear in the number of stored dashboards, which [`OMNI_DASHBOARD_QUOTA`] bounds.
    pub fn omni_dashboards_after(
        &self,
        space_id: &str,
        name_prefix: Option<&str>,
        after_seq: u64,
    ) -> Vec<&StoredOmniDashboard> {
        let mut selected: Vec<&StoredOmniDashboard> = self
            .omni_dashboards
            .values()
            .filter(|stored| stored.space_id == space_id)
            .filter(|stored| stored.seq > after_seq)
            .filter(|stored| {
                name_prefix.is_none_or(|prefix| stored.dashboard.name.starts_with(prefix))
            })
            .collect();
        selected.sort_by_key(|stored| stored.seq);
        selected
    }

    /// Every stored view in creation order, restricted to one ownership category when
    /// the request names a `type`, and skipping everything at or before `after_seq`.
    ///
    /// Linear in the number of stored views, which [`VIEW_QUOTA`] bounds.
    pub fn views_after(&self, r#type: Option<&str>, after_seq: u64) -> Vec<&StoredView> {
        let mut selected: Vec<&StoredView> = self
            .views
            .values()
            .filter(|stored| stored.seq > after_seq)
            .filter(|stored| r#type.is_none_or(|category| stored.r#type == category))
            .collect();
        selected.sort_by_key(|stored| stored.seq);
        selected
    }

    /// The access profile `space_id` holds under `name`, if the store holds one.
    ///
    /// Linear in the number of stored profiles, which [`ACCESS_PROFILE_QUOTA`] bounds.
    pub fn access_profile_named(&self, space_id: &str, name: &str) -> Option<&StoredAccessProfile> {
        self.access_profiles
            .values()
            .find(|stored| stored.profile.space_id == space_id && stored.profile.name == name)
    }

    /// The access grant `space_id` holds under `name`, if the store holds one.
    ///
    /// Linear in the number of stored grants, which [`ACCESS_GRANT_QUOTA`] bounds.
    pub fn access_grant_named(&self, space_id: &str, name: &str) -> Option<&StoredAccessGrant> {
        self.access_grants.values().find(|stored| {
            stored.grant.space_id == space_id && stored.grant.name.as_deref() == Some(name)
        })
    }

    /// The ids of the access profiles `grant`'s scoped actions apply to.
    ///
    /// A `ScopedActions` group limits a grant to the resources its `ResourceScope`s
    /// name: `resourceArns` holds "specific resource ARNs" and, per its model doc,
    /// "when absent, defaults to `*`" — every resource of that `resourceType`. So a
    /// grant applies to an access profile when one of its resource scopes names that
    /// profile's ARN, or scopes the `AccessProfile` type without narrowing it to ARNs.
    /// Only profiles of the grant's own space are reached: a grant applies to one space.
    ///
    /// This is what records a grant against a profile, which is what
    /// [`access_profile_view`] reads `assumeStatus` from — a profile no grant applies to
    /// cannot be assumed by anyone.
    ///
    /// Linear in the number of stored profiles, which [`ACCESS_PROFILE_QUOTA`] bounds,
    /// times the grant's resource scopes, which the model bounds at 20 groups of 10.
    pub fn access_profiles_scoped_by(&self, grant: &AccessGrant) -> Vec<String> {
        let scopes = || {
            grant
                .scoped_actions
                .iter()
                .flatten()
                .filter_map(|group| group.resources.as_ref())
                .flatten()
        };
        let whole_profile_type = scopes().any(|scope| {
            scope.resource_type == ACCESS_PROFILE_RESOURCE_TYPE
                && scope
                    .resource_arns
                    .as_ref()
                    .is_none_or(|arns| arns.is_empty())
        });
        let mut named: Vec<String> = self
            .access_profiles
            .values()
            .filter(|stored| stored.profile.space_id == grant.space_id)
            .filter(|stored| {
                whole_profile_type
                    || scopes().any(|scope| {
                        scope
                            .resource_arns
                            .iter()
                            .flatten()
                            .any(|arn| *arn == stored.profile.arn)
                    })
            })
            .map(|stored| stored.profile.profile_id.clone())
            .collect();
        named.sort();
        named
    }

    /// Whether an ALERT-principal trust grant that names the assume action is recorded
    /// against the access profile `profile_id` — the grant an alert assumes that profile
    /// through.
    ///
    /// Three things have to hold, and each is what a modelled member says it is:
    ///
    /// * the grant's principal type is [`ACCESS_GRANT_PRINCIPAL_TYPE_ALERT`].
    ///   `AccessGrantPrincipalType` enumerates ALERT ("An alert.") separately from every
    ///   other principal type, ACCESS_PROFILE ("An access profile.") included, so the
    ///   principal a grant names decides WHO it authorizes: a grant whose principal is
    ///   the profile itself, or a user or a role, authorizes that principal, not an alert;
    /// * one group of its scoped actions names [`ASSUME_ACCESS_PROFILE_ACTION`], the
    ///   action assuming the profile is;
    /// * that SAME group's resource scopes reach this profile — `ScopedActions` is "a
    ///   group of actions ... together with the resource scopes ... that apply to exactly
    ///   those actions", so an action in one group is not scoped by another group's
    ///   resources. `ResourceScope.resourceArns` "when absent, defaults to \"*\"", i.e.
    ///   every resource of that `resourceType`.
    ///
    /// This is a different question from the `assumeStatus` [`access_profile_view`]
    /// reports: that member is "the calling principal's authorization to assume this
    /// access profile", so a profile may be assumable by the caller and still carry no
    /// trust an alert can use.
    ///
    /// Linear in the grants recorded against the profile ([`access_profiles_scoped_by`]
    /// records them), which [`ACCESS_GRANT_QUOTA`] bounds, times the scope groups the
    /// model bounds at 20 of 10.
    ///
    /// [`access_profiles_scoped_by`]: Self::access_profiles_scoped_by
    pub fn alert_trust_grant_exists(&self, profile_id: &str) -> bool {
        let Some(profile) = self.access_profiles.get(profile_id) else {
            return false;
        };
        let profile_arn = profile.profile.arn.as_str();
        profile
            .assume_grant_ids
            .iter()
            .filter_map(|grant_id| self.access_grants.get(grant_id))
            .filter(|stored| {
                stored.grant.principal.principal_type == ACCESS_GRANT_PRINCIPAL_TYPE_ALERT
            })
            .any(|stored| grant_assumes_profile(&stored.grant, profile_arn))
    }

    /// Why an alert may not assume the access profile `profile_id`, or `None` when it
    /// may ([`alert_trust_grant_exists`]).
    ///
    /// The refusal states the condition that failed and the step that clears it: the
    /// profile the request named, and the trust grant the caller has to create on it.
    /// It is not the `AssumeStatus` DENIED sentence, which reports a different
    /// condition — the CALLING principal's own authorization to assume the profile —
    /// and would send a caller to its credentials instead of to the missing grant.
    ///
    /// [`alert_trust_grant_exists`]: Self::alert_trust_grant_exists
    pub fn alert_trust_rejection(&self, profile_id: &str) -> Option<String> {
        if self.alert_trust_grant_exists(profile_id) {
            return None;
        }
        Some(format!(
            "The alert cannot assume access profile {profile_id}. Grant the profile a \
             trust grant with principalType={ACCESS_GRANT_PRINCIPAL_TYPE_ALERT} \
             (customAction {ASSUME_ACCESS_PROFILE_ACTION}) before creating the alert."
        ))
    }

    /// The ids of the access grants that reference the access profile `profile_id`.
    ///
    /// `DeleteAccessProfile`'s doc: "An access profile cannot be deleted while access
    /// grants reference it." A grant references a profile either way round: its resource
    /// scopes name the profile, which is what [`access_profiles_scoped_by`] records as
    /// the profile's `assume_grant_ids`, or the profile IS the principal the grant is
    /// held by ([`ACCESS_GRANT_PRINCIPAL_TYPE_ACCESS_PROFILE`] with that `principalId`).
    ///
    /// Linear in the number of stored grants, which [`ACCESS_GRANT_QUOTA`] bounds.
    ///
    /// [`access_profiles_scoped_by`]: Self::access_profiles_scoped_by
    pub fn grants_referencing_profile(&self, profile_id: &str) -> Vec<String> {
        let scoped = self
            .access_profiles
            .get(profile_id)
            .map(|stored| stored.assume_grant_ids.clone())
            .unwrap_or_default();
        let mut referencing: Vec<String> = self
            .access_grants
            .values()
            .filter(|stored| {
                let principal = &stored.grant.principal;
                scoped.contains(&stored.grant.grant_id)
                    || (principal.principal_type == ACCESS_GRANT_PRINCIPAL_TYPE_ACCESS_PROFILE
                        && principal.principal_id.as_deref() == Some(profile_id))
            })
            .map(|stored| stored.grant.grant_id.clone())
            .collect();
        referencing.sort();
        referencing
    }

    /// The dashboard `space_id` holds under `name`, if the store holds one.
    ///
    /// Linear in the number of stored dashboards, which [`OMNI_DASHBOARD_QUOTA`] bounds.
    pub fn omni_dashboard_named(&self, space_id: &str, name: &str) -> Option<&StoredOmniDashboard> {
        self.omni_dashboards
            .values()
            .find(|stored| stored.space_id == space_id && stored.dashboard.name == name)
    }

    /// Whether any stored space still belongs to `domain_id`.
    ///
    /// Linear in the number of stored spaces, which [`SPACE_QUOTA`] bounds.
    pub fn domain_has_spaces(&self, domain_id: &str) -> bool {
        self.spaces
            .values()
            .any(|stored| stored.domain_id == domain_id)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudwatchomniCloudwatchomnifrontendState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudwatchomniCloudwatchomnifrontendState>)
}

/// Now, as the modelled `Timestamp` type, at the resolution this service reports a
/// timestamp in: WHOLE epoch seconds.
///
/// Every `Timestamp` member this service answers with carries a whole-second epoch
/// value, so an instant is truncated to the second when it is TAKEN rather than when it
/// is serialized: the stored record then holds exactly the instant every response
/// reports, and a client that parses the member as an integer epoch — or compares,
/// hashes or round-trips the serialized value — reads the same value back.
pub fn now() -> ::aws_smithy_types::DateTime {
    let seconds = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(seconds).unwrap_or(0))
}

/// The region the request was signed for, which is the region the resource it
/// creates lives in. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Base36 (`0-9a-z`) rendering of a 128-bit value, most significant digit first.
///
/// `36^25 > 2^128`, so the longest output is 25 characters — which is exactly the
/// budget `DomainId`'s `^d-[0-9a-z]{1,25}$` leaves after the `d-` prefix.
fn base36(mut value: u128) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while value > 0 {
        digits.push(ALPHABET[(value % 36) as usize]);
        value /= 36;
    }
    digits.reverse();
    String::from_utf8(digits).expect("the base36 alphabet is ASCII")
}

/// A fresh v4 UUID, canonical lowercase hyphenated form.
///
/// The randomness is the `uuid` crate's: this reaches it through
/// `emulator_telemetry`'s uuid-backed id newtype (`BatchId::random()` is
/// `uuid::Uuid::new_v4()` rendered canonically), which this crate ALREADY depends on,
/// rather than declaring a second path to the same crate. Nothing here is a
/// hand-rolled generator, and this crate adds no dependency — so the whole service
/// carries no `Cargo.lock` delta, which is what lets it land beside the other fixes
/// that are being merged into the same lock file concurrently.
fn fresh_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// The 128 bits of a fresh v4 UUID.
fn fresh_uuid_bits() -> u128 {
    let hyphenless: String = fresh_uuid().chars().filter(|c| *c != '-').collect();
    let bytes = ::hex::decode(hyphenless).expect("a canonical UUID is 32 hex digits");
    let octets: [u8; 16] = bytes
        .try_into()
        .expect("a canonical UUID is exactly 16 bytes");
    u128::from_be_bytes(octets)
}

/// A fresh `domainId`.
///
/// `DomainId`'s model doc: "Minted by the backend as \"d-\" + base36(UUID) — at most
/// \"d-\" plus 25 chars", so that is what this mints.
pub fn mint_domain_id() -> String {
    format!("d-{}", base36(fresh_uuid_bits()))
}

/// A fresh `spaceId`. `SpaceId`'s model doc: "a v4 UUID minted by the backend".
pub fn mint_space_id() -> String {
    fresh_uuid()
}

/// A fresh `profileId`.
///
/// `ProfileId` states no format of its own ("assigned when the profile is created")
/// beyond `@length(1..64)` and `^[a-zA-Z0-9_-]+$`; every other id this backend mints
/// and does not document a prefix for is a v4 UUID (`SpaceId`, `GrantId`,
/// `DashboardId`), and a canonical UUID is 36 characters drawn from that character
/// class, so that is what this mints.
pub fn mint_profile_id() -> String {
    fresh_uuid()
}

/// A fresh `dashboardId`. `DashboardId`'s model doc: "a lowercase v4 UUID assigned
/// when the dashboard is created".
pub fn mint_dashboard_id() -> String {
    fresh_uuid()
}

/// A fresh `grantId`. `GrantId`'s model doc: "a v4 UUID assigned when the grant is
/// created".
pub fn mint_grant_id() -> String {
    fresh_uuid()
}

/// A fresh view id — the id [`view_arn`] carries after the view's name.
///
/// The model declares no member for it and therefore no format of its own, so it is
/// minted the way every other id this backend mints without a documented prefix is: a
/// canonical v4 UUID.
pub fn mint_view_id() -> String {
    fresh_uuid()
}

/// A fresh `alertId`.
///
/// `AlertId`'s model doc: "a 32-character lowercase hex uuid, minted by the backend on
/// create and immutable across updates" — a canonical v4 UUID with its hyphens removed
/// is exactly that, so that is what this mints.
pub fn mint_alert_id() -> String {
    fresh_uuid().chars().filter(|c| *c != '-').collect()
}

/// The ARN of a domain, in this service's ARN namespace.
pub fn domain_arn(region: &str, domain_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_id}"))
}

/// The ARN of a space, in this service's ARN namespace.
pub fn space_arn(region: &str, space_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("space/{space_id}"))
}

/// The resource type an `AccessProfile`'s ARN carries.
pub const ACCESS_PROFILE_ARN_RESOURCE_TYPE: &str = "access-profile";

/// The resource type an `AccessGrant`'s ARN carries.
pub const ACCESS_GRANT_ARN_RESOURCE_TYPE: &str = "access-grant";

/// The ARN of an access profile, in this service's ARN namespace.
///
/// The profile's own id is the whole resource path: a profile is addressed by the id the
/// backend minted for it, and its parent space — which `AccessProfile.spaceId` already
/// reports — is not part of the ARN.
pub fn access_profile_arn(region: &str, profile_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{ACCESS_PROFILE_ARN_RESOURCE_TYPE}/{profile_id}"),
    )
}

/// The ARN of an access grant, in this service's ARN namespace.
///
/// Like an access profile's, the grant's own id is the whole resource path; the space
/// the grant applies to is reported by `AccessGrant.spaceId` rather than by the ARN.
pub fn access_grant_arn(region: &str, grant_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{ACCESS_GRANT_ARN_RESOURCE_TYPE}/{grant_id}"),
    )
}

/// The ARN of a dashboard, in this service's ARN namespace.
///
/// The form is the one the model's own `CreateOmniDashboard` example documents for
/// `OmniDashboard.arn`: the `omni-dashboard` resource type and the dashboard's id, in
/// the account and the region the dashboard lives in. A dashboard's ARN therefore does
/// not repeat its parent space.
pub fn omni_dashboard_arn(region: &str, dashboard_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{OMNI_DASHBOARD_ARN_RESOURCE_TYPE}/{dashboard_id}"),
    )
}

/// The ARN of an alert, in this service's ARN namespace.
///
/// The form the model's own `CreateAlert` examples document for `Alert.alertArn`: the
/// `alert` resource type and the alert's id, in the account and the region the alert
/// lives in. Like a dashboard's, an alert's ARN therefore does not repeat its parent
/// space — the alert id alone addresses it.
pub fn alert_arn(region: &str, alert_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{ALERT_ARN_RESOURCE_TYPE}/{alert_id}"),
    )
}

/// The ARN of a view, in this service's ARN namespace.
///
/// A view is addressed by its name, which `CreateView` documents as unique within the
/// account and region, and the backend also mints an id for it: both appear in the
/// resource path, the name first, so the ARN of a view created under a name that was
/// released and taken again is a different ARN.
pub fn view_arn(region: &str, name: &str, view_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{VIEW_ARN_RESOURCE_TYPE}/{name}/{view_id}"),
    )
}

/// The principal that made this request, for a `createdBy` member.
///
/// Derived from the request's own SigV4 access key id, so the member reports who
/// called rather than a fabricated identity. That id is caller-controlled text, so it
/// is first reduced to the characters an access key id can hold; a request that carries
/// no credential scope (an unsigned, in-process call) reports the emulator's default
/// principal.
pub fn caller_principal(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    let filtered: String = ::engine::extract_akid(&ctx.parts().headers)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(128)
        .collect();
    let access_key_id = if filtered.is_empty() {
        "default"
    } else {
        filtered.as_str()
    };
    // An IAM principal is account-scoped, hence the empty region segment.
    ::engine::emulator_arn("iam", "", &format!("user/{access_key_id}"))
}

/// The host a domain's user-facing endpoint URL lives under.
///
/// A domain's endpoint is the address a console or deep-link client navigates to, and it
/// is GLOBAL rather than regional: the host is this service's endpoint prefix under one
/// application host for every region, so the URL carries no region at all.
pub const DOMAIN_ENDPOINT_HOST: &str = "global.app.aws";

/// A domain's HTTPS endpoint URL, built from its id and the global application host.
pub fn domain_endpoint_url(domain_id: &str) -> String {
    format!("https://{domain_id}.{ENDPOINT_PREFIX}.{DOMAIN_ENDPOINT_HOST}")
}

/// The additional endpoint URLs the model describes as "derived from the domain
/// name": the same endpoint addressed by name rather than by id.
pub fn custom_endpoint_urls(name: &str) -> Vec<String> {
    vec![format!(
        "https://{name}.{ENDPOINT_PREFIX}.{DOMAIN_ENDPOINT_HOST}"
    )]
}

/// The Identity Center application ARN for a domain, or `None` — the model says it
/// is "Absent for IAM-only domains", so it exists exactly when the domain lists the
/// `IDC` identity provider. When the request named an Identity Center instance the
/// application is minted under that instance.
pub fn identity_center_application_arn(
    identity_providers: &[String],
    configuration: Option<&IdentityProviderConfiguration>,
) -> Option<String> {
    if !identity_providers
        .iter()
        .any(|provider| provider == IDENTITY_PROVIDER_IDC)
    {
        return None;
    }
    let instance = configuration
        .and_then(|configuration| configuration.identity_center_configuration.as_ref())
        .and_then(|center| center.identity_center_instance_arn.as_deref())
        .and_then(|arn| arn.split('/').next_back())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("ssoins-{}", short_hex()));
    // Identity Center applications are account-scoped, hence the empty region segment.
    Some(::engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance}/apl-{}", short_hex()),
    ))
}

/// Sixteen hex characters of a fresh v4 UUID — the body AWS Identity Center ids use.
fn short_hex() -> String {
    fresh_uuid()
        .chars()
        .filter(|c| *c != '-')
        .take(16)
        .collect()
}

/// The `EncryptionConfiguration` a space with no customer managed key reports.
///
/// The `Space.encryptionConfiguration` model doc: "Always populated: a space with no
/// customer managed key reports `encryptionStrategy` AWS_OWNED and no `kmsKeyArn`".
pub fn aws_owned_encryption() -> EncryptionConfiguration {
    EncryptionConfiguration {
        encryption_strategy: ENCRYPTION_STRATEGY_AWS_OWNED.to_string(),
        kms_key_arn: None,
    }
}

/// The tag map a resource reports.
///
/// A resource's own aggregate members — its tags, its principal's attribute conditions,
/// its identity-provider configuration — are part of the resource whether or not the
/// request filled them, so a response carries the EMPTY aggregate rather than dropping
/// the member: a client iterating it finds a container with no entries instead of
/// nothing. Omission is reserved for a member the resource has no value for at all (a
/// description nobody gave, an ARN the model documents as absent).
pub fn reported_tags(
    tags: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    Some(tags.unwrap_or_default())
}

/// The principal a grant reports: the principal as given, with its attribute
/// conditions reported as an empty list when it carries none.
///
/// The same rule as [`reported_tags`]: `principalAttributes` is a member of every
/// principal, so it is reported empty rather than dropped.
pub fn reported_principal(principal: AccessGrantPrincipal) -> AccessGrantPrincipal {
    AccessGrantPrincipal {
        principal_attributes: Some(principal.principal_attributes.unwrap_or_default()),
        ..principal
    }
}

/// The identity-provider configuration a domain reports.
///
/// The same rule as [`reported_tags`]: a domain's identity-provider configuration is a
/// member of every domain, so a domain configured with no provider-specific block
/// reports the empty structure rather than dropping the member. Its own
/// `identityCenterConfiguration` stays absent, because a domain with no Identity Center
/// instance has no such configuration.
pub fn reported_identity_provider_configuration(
    configuration: Option<IdentityProviderConfiguration>,
) -> Option<IdentityProviderConfiguration> {
    Some(configuration.unwrap_or_default())
}

/// Project a stored space into the `SpaceSummary` the list operations return.
///
/// Every `SpaceSummary` member is a member of `Space` with the same name, so the
/// projection is a straight copy — a summary never reports a value the full space
/// does not.
pub fn space_summary(space: &Space) -> SpaceSummary {
    SpaceSummary {
        space_id: space.space_id.clone(),
        name: space.name.clone(),
        space_arn: space.space_arn.clone(),
        domain_arn: space.domain_arn.clone(),
        region: space.region.clone(),
        owner_account_id: space.owner_account_id.clone(),
        status: space.status.clone(),
        status_reason: space.status_reason.clone(),
        created_at: space.created_at,
        updated_at: space.updated_at,
    }
}

/// Whether one group of `grant`'s scoped actions both names the action assuming an
/// access profile is and scopes that same group to the profile `profile_arn`.
///
/// `ScopedActions` is "a group of actions within an access grant, together with the
/// resource scopes and context conditions that apply to exactly those actions", so the
/// pairing is per group: an action listed in one group is not scoped by another group's
/// resources. A scope reaches the profile when its `resourceType` is
/// [`ACCESS_PROFILE_RESOURCE_TYPE`] and its `resourceArns` name that ARN or are absent —
/// `resourceArns`'s model doc: "When absent, defaults to \"*\"".
fn grant_assumes_profile(grant: &AccessGrant, profile_arn: &str) -> bool {
    grant.scoped_actions.iter().flatten().any(|group| {
        group
            .actions
            .iter()
            .any(|action| action == ASSUME_ACCESS_PROFILE_ACTION)
            && group.resources.iter().flatten().any(|scope| {
                scope.resource_type == ACCESS_PROFILE_RESOURCE_TYPE
                    && scope.resource_arns.as_ref().is_none_or(|arns| {
                        arns.is_empty() || arns.iter().any(|arn| arn == profile_arn)
                    })
            })
    })
}

/// Project a stored access profile into the `AccessProfile` an operation returns.
///
/// Every member but `assumeStatus` is the persisted record verbatim. `assumeStatus`
/// reports "the calling principal's authorization to assume this access profile", and
/// that authorization comes from a trust grant recorded against the profile — so it is
/// read from the grants the store holds for it: a profile no grant names cannot be
/// assumed, and reports DENIED.
pub fn access_profile_view(stored: &StoredAccessProfile) -> AccessProfile {
    let assume_status = if stored.assume_grant_ids.is_empty() {
        ASSUME_STATUS_DENIED
    } else {
        ASSUME_STATUS_ALLOWED
    };
    AccessProfile {
        assume_status: Some(assume_status.to_string()),
        ..stored.profile.clone()
    }
}

/// The state an alert reports when its last evaluation produced no data.
///
/// `NoData`'s model doc: it "Configures how the alert reacts when an evaluation produces
/// no data. `treatAs` carries the resulting state; defaults to `NODATA` when omitted" —
/// so the rule's own `treatAs` decides, and a rule that configures none reports
/// [`ALERT_STATE_NODATA`].
pub fn alert_no_data_state(rule: &Rule) -> String {
    let Rule::telemetryRule(telemetry) = rule;
    telemetry
        .no_data
        .as_ref()
        .map(|no_data| no_data.treat_as.clone())
        .unwrap_or_else(|| ALERT_STATE_NODATA.to_string())
}

/// Project a stored alert into the `Alert` a reporting operation returns.
///
/// Every member but `state` is the persisted record verbatim. `state` is "Live
/// evaluation state (read-only, system-managed)", so it is NOT persisted: it is
/// computed when the alert is reported. This store holds no telemetry for a rule's
/// query to read, so every evaluation of an alert produces no data, and the state that
/// yields is the rule's own ([`alert_no_data_state`]). The alert entered it when the
/// rule it is evaluating last changed, which is the record's `updatedAt`.
///
/// `contributorSummary` stays absent — its model doc: "absent until the first
/// contributor breaches a WARNING or CRITICAL threshold" — and so does `data`, which
/// carries the threshold a breach crossed.
pub fn alert_view(stored: &StoredAlert) -> Alert {
    Alert {
        state: Some(AlertStateInfo {
            value: alert_no_data_state(&stored.alert.rule),
            transitioned_at: Some(stored.alert.updated_at),
            contributor_summary: None,
            data: None,
        }),
        ..stored.alert.clone()
    }
}

/// Why `rule` is not a complete alert rule, or `None` when it is one.
///
/// `Rule`'s model doc: "`telemetryRule` members are optional at the Smithy level to
/// support PATCH semantics on UpdateAlert (send only the sub-blocks you want to
/// change). On CreateAlert, presence is enforced by the service-side validator" — so a
/// rule that defines an alert carries the query to run, the condition that fires it and
/// the cadence it runs at, and this is that validator. `noData` stays optional: its
/// model doc gives it a default.
///
/// The returned text names the member and what was wrong with it, which is all a caller
/// has to fix the request from.
pub fn alert_rule_rejection(rule: &Rule) -> Option<String> {
    let Rule::telemetryRule(telemetry) = rule;

    let Some(query) = telemetry.query.as_ref() else {
        return Some("rule.telemetryRule.query is required.".to_string());
    };
    if !QUERY_LANGUAGE_VALUES.contains(&query.language.as_str()) {
        return Some(format!(
            "rule.telemetryRule.query.language: '{}' is not a valid QueryLanguage; \
             expected one of [{}]",
            query.language,
            QUERY_LANGUAGE_VALUES.join(", ")
        ));
    }

    let Some(condition) = telemetry.condition.as_ref() else {
        return Some("rule.telemetryRule.condition is required.".to_string());
    };
    // `AlertCondition`'s model doc: "A condition must keep at least one tier", the tiers
    // being its warning and critical thresholds — a condition with neither says nothing
    // about when the alert fires.
    if condition.warning_threshold.is_none() && condition.critical_threshold.is_none() {
        return Some(
            "rule.telemetryRule.condition must set warningThreshold, criticalThreshold, or both."
                .to_string(),
        );
    }
    // The same doc: `thresholdMode` and `comparator` "are optional at the Smithy level
    // (so a single-tier condition is expressible) but are required whenever a threshold
    // is present" — a threshold with neither cannot be compared against anything.
    let Some(threshold_mode) = condition.threshold_mode.as_deref() else {
        return Some(
            "rule.telemetryRule.condition.thresholdMode is required when a threshold is present."
                .to_string(),
        );
    };
    if !THRESHOLD_MODE_VALUES.contains(&threshold_mode) {
        return Some(format!(
            "rule.telemetryRule.condition.thresholdMode: '{threshold_mode}' is not a valid \
             ThresholdMode; expected one of [{}]",
            THRESHOLD_MODE_VALUES.join(", ")
        ));
    }
    let Some(comparator) = condition.comparator.as_deref() else {
        return Some(
            "rule.telemetryRule.condition.comparator is required when a threshold is present."
                .to_string(),
        );
    };
    if !COMPARATOR_VALUES.contains(&comparator) {
        return Some(format!(
            "rule.telemetryRule.condition.comparator: '{comparator}' is not a valid Comparator; \
             expected one of [{}]",
            COMPARATOR_VALUES.join(", ")
        ));
    }

    let Some(evaluation) = telemetry.evaluation.as_ref() else {
        return Some("rule.telemetryRule.evaluation is required.".to_string());
    };
    // `AlertEvaluation`'s model doc: "`intervalSeconds` must be one of {30, 60, 120,
    // 300, 600, 900, 1800, 3600}; `pendingDurationSeconds` and
    // `recoveryDurationSeconds` must be multiples of `intervalSeconds`. A duration of 0
    // means fire/clear immediately with no delay."
    let interval = evaluation.interval_seconds;
    if !ALERT_EVALUATION_INTERVALS.contains(&interval) {
        return Some(format!(
            "rule.telemetryRule.evaluation.intervalSeconds: {interval} is not a supported \
             evaluation interval; expected one of [{}]",
            ALERT_EVALUATION_INTERVALS
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for (member, duration) in [
        (
            "pendingDurationSeconds",
            evaluation.pending_duration_seconds,
        ),
        (
            "recoveryDurationSeconds",
            evaluation.recovery_duration_seconds,
        ),
    ] {
        if let Some(duration) = duration {
            if duration < 0 || duration % interval != 0 {
                return Some(format!(
                    "rule.telemetryRule.evaluation.{member}: {duration} must be a non-negative \
                     multiple of intervalSeconds {interval}"
                ));
            }
        }
    }

    if let Some(no_data) = telemetry.no_data.as_ref() {
        if !ALERT_STATE_VALUES.contains(&no_data.treat_as.as_str()) {
            return Some(format!(
                "rule.telemetryRule.noData.treatAs: '{}' is not a valid AlertState; \
                 expected one of [{}]",
                no_data.treat_as,
                ALERT_STATE_VALUES.join(", ")
            ));
        }
    }

    None
}

/// Why `rules` are not notification rules this service can act on, or `None` when they
/// are.
///
/// An enum member arrives as text, so the values the model admits for a rule's target
/// type and for the states that trigger it are checked here rather than at the
/// boundary.
pub fn notification_rules_rejection(rules: &[NotificationRule]) -> Option<String> {
    for (index, rule) in rules.iter().enumerate() {
        let target_type = rule.target.r#type.as_str();
        if !NOTIFICATION_TARGET_TYPE_VALUES.contains(&target_type) {
            return Some(format!(
                "notificationRules[{index}].target.type: '{target_type}' is not a valid \
                 NotificationTargetType; expected one of [{}]",
                NOTIFICATION_TARGET_TYPE_VALUES.join(", ")
            ));
        }
        for state in rule.trigger.state_values.iter().flatten() {
            if !ALERT_STATE_VALUES.contains(&state.as_str()) {
                return Some(format!(
                    "notificationRules[{index}].trigger.stateValues: '{state}' is not a valid \
                     AlertState; expected one of [{}]",
                    ALERT_STATE_VALUES.join(", ")
                ));
            }
        }
    }
    None
}

/// The dashboard-body schema a `body` is validated against, and the refusal text a
/// violated schema renders. It lives beside the store because several operations take a
/// dashboard definition and all of them validate it the same way.
mod dashboard_body;

/// What a view's `definition` has to be, and the refusal text a definition that is not
/// it renders. It lives beside the store because every operation that takes a view
/// definition applies the same rule to it.
mod view_definition;

pub use view_definition::{view_definition_rejection, INVALID_VIEW_DEFINITION};

pub use dashboard_body::{
    dashboard_body_rejection, dashboard_body_violations, DashboardBodyViolation,
    DASHBOARD_BODY_PANELS, DOCUMENT_ROOT,
};

/// Project a stored dashboard into the `OmniDashboardSummary` the list operation
/// returns.
///
/// Every `OmniDashboardSummary` member is a member of `OmniDashboard` with the same
/// name except `body` — the summary's doc reads "Summary of a dashboard. Call
/// GetOmniDashboard for the full dashboard" — so the projection is a copy with the
/// definition left out, and a summary never reports a value the full dashboard does not.
pub fn omni_dashboard_summary(dashboard: &OmniDashboard) -> OmniDashboardSummary {
    OmniDashboardSummary {
        dashboard_id: dashboard.dashboard_id.clone(),
        arn: dashboard.arn.clone(),
        name: dashboard.name.clone(),
        created_by: dashboard.created_by.clone(),
        description: dashboard.description.clone(),
        tags: dashboard.tags.clone(),
        created_at: dashboard.created_at,
        updated_at: dashboard.updated_at,
    }
}

/// The prefix a space-list page token carries, so a token minted for one collection
/// cannot be replayed against another.
const SPACE_PAGE_TOKEN_PREFIX: &str = "spaces:";

/// The prefix a dashboard-list page token carries, for the same reason.
const OMNI_DASHBOARD_PAGE_TOKEN_PREFIX: &str = "omni-dashboards:";

/// Mint the opaque `nextToken` that resumes a listing of `prefix`'s collection after
/// `seq`.
fn encode_page_token(prefix: &str, seq: u64) -> String {
    ::hex::encode(format!("{prefix}{seq}"))
}

/// Read a `nextToken` back, or `None` when it is not one this service minted for
/// `prefix`'s collection.
fn decode_page_token(prefix: &str, token: &str) -> Option<u64> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix(prefix)?.parse().ok()
}

/// Mint the opaque `nextToken` that resumes a space listing after `seq`.
pub fn encode_space_page_token(seq: u64) -> String {
    encode_page_token(SPACE_PAGE_TOKEN_PREFIX, seq)
}

/// Read a space-list `nextToken` back, or `None` when it is not one this service minted.
pub fn decode_space_page_token(token: &str) -> Option<u64> {
    decode_page_token(SPACE_PAGE_TOKEN_PREFIX, token)
}

/// Mint the opaque `nextToken` that resumes a dashboard listing after `seq`.
pub fn encode_omni_dashboard_page_token(seq: u64) -> String {
    encode_page_token(OMNI_DASHBOARD_PAGE_TOKEN_PREFIX, seq)
}

/// Read a dashboard-list `nextToken` back, or `None` when it is not one this service
/// minted for a dashboard listing.
pub fn decode_omni_dashboard_page_token(token: &str) -> Option<u64> {
    decode_page_token(OMNI_DASHBOARD_PAGE_TOKEN_PREFIX, token)
}

/// The prefix a view-list page token carries, for the same reason.
const VIEW_PAGE_TOKEN_PREFIX: &str = "views:";

/// Mint the opaque `nextToken` that resumes a view listing after `seq`.
pub fn encode_view_page_token(seq: u64) -> String {
    encode_page_token(VIEW_PAGE_TOKEN_PREFIX, seq)
}

/// Read a view-list `nextToken` back, or `None` when it is not one this service minted
/// for a view listing.
pub fn decode_view_page_token(token: &str) -> Option<u64> {
    decode_page_token(VIEW_PAGE_TOKEN_PREFIX, token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base36_never_exceeds_the_domain_id_budget() {
        assert_eq!(base36(0), "0", "zero renders as a single digit");
        assert_eq!(base36(35), "z", "the last alphabet digit");
        assert_eq!(base36(36), "10", "the first two-digit value");
        assert!(
            base36(u128::MAX).len() <= 25,
            "base36(u128::MAX) is {} chars, over DomainId's 25-char budget",
            base36(u128::MAX).len()
        );
    }

    #[test]
    fn a_profile_reports_assume_status_from_the_grants_recorded_against_it() {
        // Rows: no trust grant, one, and several. The status is read from the grant
        // list, so it is DENIED exactly when the list is empty.
        let at = now();
        let profile = AccessProfile {
            profile_id: "profile".to_string(),
            space_id: "space".to_string(),
            arn: "arn:aws:cloudwatch:us-east-1:000000000000:access-profile/profile".to_string(),
            name: "a profile".to_string(),
            description: None,
            created_at: at,
            updated_at: at,
            assume_status: None,
            profile_type: Some(ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED.to_string()),
        };
        for (grants, expected) in [
            (vec![], ASSUME_STATUS_DENIED),
            (vec!["grant-a".to_string()], ASSUME_STATUS_ALLOWED),
            (
                vec!["grant-a".to_string(), "grant-b".to_string()],
                ASSUME_STATUS_ALLOWED,
            ),
        ] {
            let stored = StoredAccessProfile {
                seq: 1,
                profile: profile.clone(),
                assume_grant_ids: grants.clone(),
                tags: HashMap::new(),
            };
            let view = access_profile_view(&stored);
            assert_eq!(
                view.assume_status.as_deref(),
                Some(expected),
                "case {grants:?}: expected assumeStatus {expected}, got {:?}",
                view.assume_status
            );
            assert_eq!(
                view.profile_id, stored.profile.profile_id,
                "case {grants:?}: the projection must not change profileId, got {:?}",
                view.profile_id
            );
        }
    }

    #[test]
    fn a_page_token_round_trips_and_rejects_foreign_text() {
        for seq in [1_u64, 2, 99, u64::MAX] {
            let token = encode_space_page_token(seq);
            assert_eq!(
                decode_space_page_token(&token),
                Some(seq),
                "case {seq}: token {token} did not round-trip"
            );
        }
        for bogus in ["", "not-hex", &::hex::encode("domains:1")] {
            assert_eq!(
                decode_space_page_token(bogus),
                None,
                "case {bogus:?}: a foreign token must not decode"
            );
        }
    }
}
