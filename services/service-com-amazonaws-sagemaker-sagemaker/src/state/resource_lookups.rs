// SPDX-License-Identifier: CC0-1.0

//! The resource-lookup slice of this service's state: the one resolution each of the
//! HyperPod scheduling, experiment-trial, model-card-export and partner-app stores is
//! reached by, so no handler re-implements one.

use super::*;

impl ComAmazonawsSagemakerSagemakerState {
    /// The cluster a `ClusterNameOrArn` member names: by ARN when the value is one, by
    /// name otherwise — the two forms the model accepts for the one resource.
    /// A cluster whose deletion the clock already completed answers `None` here, as it
    /// does from every other cluster lookup ([`Cluster::is_released`]).
    pub fn cluster_by_name_or_arn(&self, value: &str) -> Option<&Cluster> {
        let now = SystemTime::now();
        if value.starts_with(ARN_PREFIX) {
            return self
                .clusters
                .values()
                .find(|cluster| cluster.arn == value)
                .filter(|cluster| !cluster.is_released(now));
        }
        self.clusters
            .get(value)
            .filter(|cluster| !cluster.is_released(now))
    }

    /// [`Self::cluster_by_name_or_arn`], for a caller that writes.
    pub fn cluster_by_name_or_arn_mut(&mut self, value: &str) -> Option<&mut Cluster> {
        let now = SystemTime::now();
        if value.starts_with(ARN_PREFIX) {
            return self
                .clusters
                .values_mut()
                .find(|cluster| cluster.arn == value)
                .filter(|cluster| !cluster.is_released(now));
        }
        self.clusters
            .get_mut(value)
            .filter(|cluster| !cluster.is_released(now))
    }

    /// [`Self::cluster_by_arn`], for a caller that writes.
    pub fn cluster_by_arn_mut(&mut self, value: &str) -> Option<&mut Cluster> {
        let now = SystemTime::now();
        self.clusters
            .values_mut()
            .find(|cluster| cluster.arn == value)
            .filter(|cluster| !cluster.is_released(now))
    }

    /// The partner app whose ARN is `value`.
    pub fn partner_app_by_arn_mut(&mut self, value: &str) -> Option<&mut PartnerApp> {
        self.partner_apps
            .values_mut()
            .find(|app| app.arn == value)
    }

    /// Whether this account holds a partner-application subscription for `app_type`, which
    /// is what `CreatePartnerApp` admits a `Type` by.
    ///
    /// A partner AI app runs a third party's licensed application: the account subscribes
    /// to it in AWS Marketplace, which is a purchase outside SageMaker's API surface — no
    /// operation this service models adds to this set. So an account that has subscribed to
    /// nothing holds none, and every type is refused ([`PARTNER_APP_NOT_SUBSCRIBED`]) until
    /// a subscription for it is held.
    pub fn subscribed_to_partner_app(&self, app_type: &str) -> bool {
        self.partner_app_subscriptions.contains(app_type)
    }

    /// Whether another partner app fits, i.e. the store is below [`PARTNER_APP_QUOTA`].
    pub fn partner_apps_below_quota(&self) -> bool {
        self.partner_apps.len() < PARTNER_APP_QUOTA
    }

    /// The partner app this account holds under `name`, the one identity a caller chooses
    /// for an app (its id and ARN are SageMaker's to mint).
    ///
    /// ACCOUNT-WIDE rather than per Region, which is the scope
    /// [`Self::partner_apps`] has: the store is keyed by the app id alone and
    /// `ListPartnerApps` pages all of it, so a name is unique across everything a caller
    /// of this emulator can see. Narrowing both to the Region of the app's ARN is one
    /// change covering the listing as well, not a property of the create alone.
    pub fn partner_app_named(&self, name: &str) -> Option<&PartnerApp> {
        self.partner_apps.values().find(|app| app.name == name)
    }

    /// The partner app a create presenting `token` already minted, so the retry of that
    /// create answers the app it made rather than a second one.
    pub fn partner_app_for_client_token(&self, token: &str) -> Option<&PartnerApp> {
        self.partner_apps
            .values()
            .find(|app| app.client_token.as_deref() == Some(token))
    }

    /// Whether another trial fits, i.e. the store is below [`TRIAL_QUOTA`].
    pub fn trials_below_quota(&self) -> bool {
        self.trials.len() < TRIAL_QUOTA
    }

    /// The trial `name` identifies, whatever case it is written in — `TrialName` is
    /// unique per account and not case-sensitive, so the lookup folds case the way
    /// [`Self::experiment`] does for an experiment.
    pub fn trial(&self, name: &str) -> Option<&Trial> {
        self.trials.get(&entity_key(name))
    }

    /// Whether another compute quota fits, i.e. the store is below [`COMPUTE_QUOTA_QUOTA`].
    pub fn compute_quotas_below_quota(&self) -> bool {
        self.compute_quotas.len() < COMPUTE_QUOTA_QUOTA
    }
}
