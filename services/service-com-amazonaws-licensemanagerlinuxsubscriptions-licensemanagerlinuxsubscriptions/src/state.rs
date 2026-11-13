// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions service.
//!
//! One store holds every resource this service's operations share: the registered
//! third-party subscription providers, the per-account Linux subscriptions service
//! settings and the instances Linux subscription discovery has found. The lookup / list
//! / filter / pagination helpers live here too, so the operation handlers in `ops/` all
//! resolve a resource the same way instead of each growing its own copy.
//!
//! The instances are DISCOVERED rather than registered through this service's API, so what
//! writes them is the discovery pass in [`discovery`], which reads the account's EC2
//! inventory in the configured source Regions.

/// The discovery pass that fills the discovered-instance store from the account's EC2
/// inventory, through the service that owns instances and AMIs.
pub mod discovery;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Filter, LinuxSubscriptionsDiscoverySettings, RegisteredSubscriptionProvider, OPERATOR_VALUES,
};

/// `SubscriptionProviderStatus` of a registration that is USABLE: License Manager holds
/// the provider access token and retrieves subscription data with it
/// (`SubscriptionProviderStatus`: ACTIVE | INVALID | PENDING).
///
/// This is the terminal status a registration the account asked for reaches, and the one a
/// client script or the console polls for before it expects third-party subscription data
/// — which is why a registration carrying it also reports a
/// [`SubscriptionProviderRecord::last_successful_data_retrieval_time`].
pub const PROVIDER_STATUS_ACTIVE: &str = "ACTIVE";

/// `SubscriptionProviderStatus` of a registration whose access token License Manager has
/// not finished validating: the status `RegisterSubscriptionProvider` reports, because the
/// registration resource exists from that call on while the validation of its token runs
/// behind it (`GetRegisteredSubscriptionProvider` is what reports where it got to).
pub const PROVIDER_STATUS_PENDING: &str = "PENDING";

/// `SubscriptionProviderStatus` of a registration whose access token the service rejected.
/// Terminal: the registration stays in the store reporting it, together with the
/// [`SubscriptionProviderRecord::status_message`] the service has about the token, which is
/// the only way a client can tell a refused token from one still being checked.
///
/// Nothing in the registration path reaches it: a registration is refused at the request
/// boundary or it is taken, and a taken one becomes usable
/// ([`SubscriptionProviderRecord::settle`]). It stays named here because it is a value of
/// the modelled enum a client may branch on.
pub const PROVIDER_STATUS_INVALID: &str = "INVALID";

/// How long a fresh registration stays [`PROVIDER_STATUS_PENDING`] before the clock owes
/// it its terminal `SubscriptionProviderStatus`.
///
/// Validating the access token is work that happens BEHIND the registration call — the
/// call answers `PENDING` and the verdict lands later — and AWS documents no duration for
/// it, so this span is CHOSEN rather than cited: long enough that a caller polling
/// straight after `RegisterSubscriptionProvider` still observes `PENDING`, short enough
/// that a client script does not wait on it. The due instant is stored on the record
/// ([`SubscriptionProviderRecord::settles_at`]) and materialized by whichever later call
/// runs after it falls due ([`SubscriptionProviderRecord::settle`]) — nothing sleeps.
pub const TOKEN_VALIDATION_DELAY: Duration = Duration::from_millis(500);

/// The `LinuxSubscriptionsDiscovery` value an account that never called
/// `UpdateServiceSettings` reports (`LinuxSubscriptionsDiscovery`: Disabled | Enabled).
pub const DISCOVERY_DISABLED: &str = "Disabled";

/// The `LinuxSubscriptionsDiscovery` value of an account that asked for its Linux
/// subscriptions to be discovered — the other value of the same modelled enum, and the one
/// a discovery pass runs for ([`discovery::materialize_for_report`]).
pub const DISCOVERY_ENABLED: &str = "Enabled";

/// The `Status` a settled change to the Linux subscriptions service settings reports
/// (`Status`: Completed | Failed | InProgress | Successful — `smithy_query(query="shape",
/// shape="Status")`).
///
/// `Completed` is the value the recorded service reports once an enable or a disable of
/// `LinuxSubscriptionsDiscovery` has been applied, and it is the value every client's
/// onboarding poll waits for. The settings singleton is written before the call that
/// changed it returns, so the change is already applied — the status is terminal on the
/// first read and no later transition is owed on it. It follows that an account carrying
/// no settings singleton reports NO status: there is no change for one to report on.
pub const SETTINGS_STATUS_COMPLETED: &str = "Completed";

/// The fixed text a refusal naming no registered subscription provider carries.
///
/// The recorded service states this refusal as a CONSTANT that names the resource type
/// and nothing from the request: the ARN the caller sent is not interpolated back into
/// it. The TAG APIs have their own taxonomy for the same condition — a different code
/// and a different constant, `crate::scaffolded::NOT_FOUND_MESSAGE`.
pub const PROVIDER_NOT_FOUND_MESSAGE: &str = "Registered Subscription Provider not found";

/// Why an operation could not be served from state.
///
/// Each handler maps this onto its OWN modelled error enum (the generated enums are
/// per-operation types), so the lookup rules themselves live here exactly once.
#[derive(Debug, Clone)]
pub enum StateError {
    /// The addressed resource is not in the store — `ResourceNotFoundException`.
    NotFound(String),
    /// The request is malformed in a way the model cannot express — `ValidationException`.
    Validation(String),
}

/// A registered Linux subscription provider, as `RegisterSubscriptionProvider` stored it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SubscriptionProviderRecord {
    /// The minted `SubscriptionProviderArn`; the key of `providers`.
    pub arn: String,
    /// The `SubscriptionProviderSource` that was registered (the model allows `RedHat`).
    pub source: String,
    /// The Secrets Manager secret holding the provider's access token.
    pub secret_arn: String,
    /// `SubscriptionProviderStatus` for the stored access token.
    pub status: String,
    /// Set only when the service has something to report about the token; never fabricated.
    pub status_message: Option<String>,
    /// When License Manager last retrieved subscription details from this registered
    /// provider, ISO-8601 UTC ([`iso8601`]). Set when the registration became usable
    /// ([`PROVIDER_STATUS_ACTIVE`]), which is the retrieval the status reports on, and
    /// absent while the registration has not got there — never invented for a registration
    /// that has not.
    pub last_successful_data_retrieval_time: Option<String>,
    /// The metadata tags the registration carried.
    pub tags: HashMap<String, String>,
    /// When the clock owes this registration its terminal `SubscriptionProviderStatus`,
    /// while the token validation behind the registration call has not landed yet; `None`
    /// once the status is terminal, so nothing moves twice.
    pub settles_at: Option<SystemTime>,
}

impl SubscriptionProviderRecord {
    /// A registration of `source` against `secret_arn`, as the registration call leaves
    /// it: `PENDING`, with the terminal status the clock owes it due at
    /// `now + `[`TOKEN_VALIDATION_DELAY`].
    pub fn registered(
        arn: String,
        source: String,
        secret_arn: String,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            arn,
            source,
            secret_arn,
            status: PROVIDER_STATUS_PENDING.to_string(),
            // Nothing to report about the token while it is still being checked.
            status_message: None,
            last_successful_data_retrieval_time: None,
            tags,
            settles_at: Some(now + TOKEN_VALIDATION_DELAY),
        }
    }

    /// A registration already in a terminal `status` as of `now`, for a record this
    /// service's state holds without a registration call having produced it.
    ///
    /// A registration in [`PROVIDER_STATUS_ACTIVE`] is usable, so it carries the retrieval
    /// time that status reports on, exactly as one that got there through
    /// [`settle`](Self::settle) does: the member cannot depend on how the record was
    /// placed.
    pub fn settled(
        arn: String,
        source: String,
        secret_arn: String,
        status: String,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Self {
        let active = status == PROVIDER_STATUS_ACTIVE;
        Self {
            arn,
            source,
            secret_arn,
            status,
            status_message: None,
            last_successful_data_retrieval_time: active.then(|| iso8601(now)),
            tags,
            settles_at: None,
        }
    }

    /// Materialize the status transition the clock owes this registration by `now`.
    ///
    /// A registration the account asked for and the service took becomes USABLE: the
    /// validation running behind the registration call finishes and the registration
    /// reports [`PROVIDER_STATUS_ACTIVE`], with nothing to report about the token (so no
    /// `SubscriptionProviderStatusMessage`) and the subscription data retrieval that status
    /// reports on recorded in
    /// [`last_successful_data_retrieval_time`](Self::last_successful_data_retrieval_time).
    /// A failure status belongs to a registration the service rejected, which is a
    /// condition the request boundary answers before a record exists at all.
    ///
    /// The transition was owed at `settles_at` whether or not anybody called, which is what
    /// lets a report materialize it: no caller's read decides it, and the values it writes
    /// are derived from the DUE instant rather than from the reader's clock, so every later
    /// read of the same registration answers the same thing.
    fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = PROVIDER_STATUS_ACTIVE.to_string();
        self.status_message = None;
        self.last_successful_data_retrieval_time = Some(iso8601(due));
    }

    /// Point this registration at `secret_arn` and start the token validation that change
    /// owes.
    ///
    /// The `SubscriptionProviderStatus` reports on the access token License Manager holds
    /// for the registration, so a registration pointed at another secret holds a token
    /// nothing has checked yet: it reports [`PROVIDER_STATUS_PENDING`] again, with the
    /// terminal status owed at `now + `[`TOKEN_VALIDATION_DELAY`] exactly as a fresh
    /// registration's is, and with nothing to report about the new token.
    ///
    /// `LastSuccessfulDataRetrievalTime` is left as it stands: it reports WHEN License
    /// Manager last retrieved subscription details from the provider, which is a retrieval
    /// that happened and does not un-happen because the secret the next one will use
    /// changed.
    fn point_at_secret(&mut self, secret_arn: &str, now: SystemTime) {
        if self.secret_arn == secret_arn {
            return;
        }
        self.secret_arn = secret_arn.to_string();
        self.status = PROVIDER_STATUS_PENDING.to_string();
        self.status_message = None;
        self.settles_at = Some(now + TOKEN_VALIDATION_DELAY);
    }
}

/// The `Status` of a discovered instance that is running, and so counts towards its
/// subscription's `InstanceCount` (`Subscription$InstanceCount`: "The total amount of
/// running instances using this subscription"). The member carries the EC2 instance
/// state name, which is `running` for an instance that is up.
pub const INSTANCE_STATUS_RUNNING: &str = "running";

/// One instance Linux subscription discovery has found, with the subscription it uses.
///
/// Discovery reads the account's EC2 inventory in the configured source Regions; nothing
/// in THIS service's API registers an instance, so the store below is written only by a
/// discovery pass ([`discovery::materialize_for_report`]). Both
/// `ListLinuxSubscriptionInstances` (which reports these records) and
/// `ListLinuxSubscriptions` (which aggregates them into subscriptions) project it, so
/// the two lists can never disagree about what was discovered.
#[derive(Debug, Clone, Default)]
pub struct DiscoveredInstanceRecord {
    /// `Instance$InstanceID` — the instance id, and the key of `discovered_instances`.
    pub instance_id: String,
    /// `Instance$AccountID` — the account that owns the instance.
    pub account_id: String,
    /// `Instance$AmiId` — the AMI the instance was launched from.
    pub ami_id: String,
    /// `Instance$InstanceType`.
    pub instance_type: String,
    /// `Instance$Status` — the instance state; [`INSTANCE_STATUS_RUNNING`] is what makes
    /// the instance count towards its subscription.
    pub status: String,
    /// `Instance$Region` — the Region the instance runs in.
    pub region: String,
    /// `Instance$UsageOperation` — the billing usage operation the instance reports.
    pub usage_operation: String,
    /// `Instance$ProductCode` — the product codes on the instance's AMI.
    pub product_codes: Vec<String>,
    /// `Instance$LastUpdatedTime` — when discovery last refreshed this record.
    pub last_updated_time: Option<String>,
    /// `Instance$SubscriptionName` — the name of the subscription the instance uses, and
    /// half of the key `ListLinuxSubscriptions` groups on.
    pub subscription_name: String,
    /// The `Subscription$Type` of that subscription (subscription-included with Amazon
    /// EC2, BYOS, or from the Amazon Web Services Marketplace), the other half of that
    /// key. Discovery records it per instance, because the same subscription name can be
    /// licensed in more than one way.
    pub subscription_type: String,
    /// `Instance$OsVersion`.
    pub os_version: Option<String>,
    /// `Instance$SubscriptionProviderCreateTime`, set only for an instance whose
    /// subscription comes from a registered third-party provider.
    pub subscription_provider_create_time: Option<String>,
    /// `Instance$SubscriptionProviderUpdateTime`, set on the same condition.
    pub subscription_provider_update_time: Option<String>,
    /// `Instance$DualSubscription`.
    pub dual_subscription: Option<String>,
    /// `Instance$RegisteredWithSubscriptionProvider`.
    pub registered_with_subscription_provider: Option<String>,
}

/// One discovered Linux subscription: a `Subscription` as `ListLinuxSubscriptions`
/// reports it, aggregated from the discovered instances that use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSubscription {
    /// `Subscription$Name`.
    pub name: String,
    /// `Subscription$Type`.
    pub subscription_type: String,
    /// `Subscription$InstanceCount` — how many of the discovered instances using this
    /// subscription are running.
    pub instance_count: i64,
}

/// Where a discovery pass STOPPED reading, so the next one picks the plan up there
/// instead of re-reading the prefix the previous one already spent its budget on.
///
/// One pass reads a bounded amount of the owner's inventory
/// ([`discovery::materialize_for_report`]), which can be less than the source Regions its
/// plan names hold; what makes every Region of that plan eventually read is that the point
/// a pass stopped at is RECORDED and the next pass starts from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryResume {
    /// The pass read this Region's inventory to its end, so the next one starts at the
    /// first Region of the plan ordered AFTER it.
    After(String),
    /// The pass stopped INSIDE this Region's inventory, at the owner's inventory cursor it
    /// had reached (`None` being the beginning of that inventory, for a pass that stopped
    /// before it recorded a whole page of it), so the next one resumes that Region there.
    Inside(String, Option<String>),
}

impl DiscoveryResume {
    /// The source Region the pass stopped in or on — the point the plan is rotated to.
    pub fn region(&self) -> &str {
        match self {
            Self::After(region) | Self::Inside(region, _) => region,
        }
    }
}

/// The source Regions ONE discovery pass asks about, in the order it asks about them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryPlan {
    /// Every Region the account discovers right now, each exactly once: the sorted set,
    /// rotated so the walk begins where the previous pass stopped. A pass therefore asks
    /// about no Region twice, and successive passes cover them all.
    pub regions: Vec<String>,
    /// The owner's inventory cursor the FIRST of those Regions resumes from, when the
    /// previous pass stopped inside it; `None` when that Region's inventory is read from
    /// its beginning.
    pub resume_cursor: Option<String>,
}

/// The per-account Linux subscriptions service settings, as `UpdateServiceSettings`
/// persisted them.
#[derive(Debug, Clone)]
pub struct ServiceSettingsRecord {
    /// Whether discovery of Linux subscriptions is enabled.
    pub linux_subscriptions_discovery: String,
    /// The discovery settings: the source Regions and the Organizations integration.
    pub discovery_settings: LinuxSubscriptionsDiscoverySettings,
    /// The `Status` of the settings being applied.
    pub status: String,
    /// Detail about that status. Empty when the service has nothing to report.
    pub status_message: HashMap<String, String>,
    /// The Regions in which License Manager displays the aggregated data, in the order
    /// they became home Regions.
    pub home_regions: Vec<String>,
}

/// Per-service state for com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState {
    /// Registered providers by `SubscriptionProviderArn`, which is also the order the
    /// list operation reports and pages them in.
    ///
    /// The registrations this account holds are one per supported
    /// `SubscriptionProviderSource`
    /// ([`register_provider`](Self::register_provider)), so the map is keyed by the ARN
    /// every other operation addresses a registration by rather than by the source.
    ///
    /// The key is a TOTAL ORDER over this account's registrations, and a minted id is
    /// monotonic in registration order ([`provider_id`]), so the sequence a page is cut
    /// from is decided by the registrations themselves rather than by how many of them the
    /// store held when a page was cut — which is what lets a pagination cursor name an
    /// element of that sequence and keep its meaning across a write.
    pub providers: BTreeMap<String, SubscriptionProviderRecord>,
    /// How many registrations this account has ever minted, so a fresh
    /// `SubscriptionProviderArn` is unique even after the registration that held the
    /// previous one was deregistered.
    pub minted_providers: u64,
    /// The per-account settings singleton, once an update persisted one.
    pub service_settings: Option<ServiceSettingsRecord>,
    /// The instances Linux subscription discovery has found, by `Instance$InstanceID`.
    ///
    /// An instance id is unique, so the map both keeps a re-discovery of the same
    /// instance from adding a second record and gives the two list operations a stable,
    /// pageable order. Discovery is the only writer (see [`DiscoveredInstanceRecord`]):
    /// until one has run, the account has discovered nothing and both lists are empty,
    /// which is exactly what an account with no instances reports.
    pub discovered_instances: BTreeMap<String, DiscoveredInstanceRecord>,
    /// Where the last discovery pass stopped, when it left part of its plan unread.
    ///
    /// A pass reads a bounded amount of the owner's inventory, so a plan with more source
    /// Regions than that bound serves is read over several passes: this is the one record
    /// of how far the last of them got ([`DiscoveryResume`]), and `None` means the next
    /// pass picks the plan up at its beginning — which is what a pass that read its whole
    /// plan leaves behind.
    pub discovery_resume: Option<DiscoveryResume>,
}

impl ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState {
    /// Materialize every provider status transition the clock owes by `now`.
    ///
    /// Called by the operations that REPORT a `SubscriptionProviderStatus` before they read
    /// it, so what they report is the status that was already due at the instant they ran
    /// — the same status a caller would have seen had nobody called at all.
    pub fn settle_providers(&mut self, now: SystemTime) {
        for record in self.providers.values_mut() {
            record.settle(now);
        }
    }

    /// Register `source` against `secret_arn`, and answer the registration to report.
    ///
    /// A registration's identity is the `SubscriptionProviderSource` it is for: an account
    /// holds ONE registration per supported source — "Register THE supported third-party
    /// subscription provider for your Bring Your Own License (BYOL) subscription"
    /// (`smithy_query(query="operation",operation="RegisterSubscriptionProvider")`) — so a
    /// call naming a source this account already registered addresses THAT registration
    /// rather than minting a second resource: its `SubscriptionProviderArn` is the one the
    /// first registration minted, and the secret it holds becomes the one this call named
    /// ([`SubscriptionProviderRecord::point_at_secret`]). The `@idempotent` repeat — the
    /// same source with the same secret — therefore changes nothing at all.
    ///
    /// Any tags the call carries are merged into the registration's tag set, exactly as
    /// `TagResource` merges them.
    ///
    /// `Err` is a registration this account may not take: a resultant tag set past
    /// [`MAX_TAGS_PER_RESOURCE`]. Nothing is written in that case.
    pub fn register_provider(
        &mut self,
        region: &str,
        source: &str,
        secret_arn: &str,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Result<SubscriptionProviderRecord, TagLimitExceeded> {
        self.settle_providers(now);

        if let Some(arn) = self.registered_arn(source) {
            let record = self
                .providers
                .get_mut(&arn)
                .expect("the ARN was just read out of the same map");
            // The call's pairs are merged into the registration's set under the ONE
            // resultant-set bound every tag write of this service obeys ([`merge_tags`]),
            // so this path and `TagResource` cannot disagree about either the merge or
            // the bound; a refused call leaves the registration exactly as it was — which
            // is why the merge is decided BEFORE the secret is written.
            merge_tags(&mut record.tags, tags)?;
            record.point_at_secret(secret_arn, now);
            return Ok(record.clone());
        }

        // A fresh registration starts from nothing stored, so the same merge decides the
        // set it is created with — and refuses it through the same bound.
        let mut fresh_tags = HashMap::new();
        merge_tags(&mut fresh_tags, tags)?;
        self.minted_providers += 1;
        let arn = provider_arn(region, &provider_id(self.minted_providers));
        let record = SubscriptionProviderRecord::registered(
            arn.clone(),
            source.to_string(),
            secret_arn.to_string(),
            fresh_tags,
            now,
        );
        self.providers.insert(arn, record.clone());
        Ok(record)
    }

    /// The `SubscriptionProviderArn` this account already registered `source` under, if
    /// any — the identity a later registration of the same source resolves to.
    ///
    /// The scan is over the registrations this account holds, which is one per supported
    /// `SubscriptionProviderSource`, so the work one registration costs is bounded however
    /// often a caller registers.
    fn registered_arn(&self, source: &str) -> Option<String> {
        self.providers
            .iter()
            .find(|(_, record)| record.source == source)
            .map(|(arn, _)| arn.clone())
    }

    /// The registered providers in `SubscriptionProviderArn` order — the order the list
    /// operation reports them in, and the one a pagination cursor names an element of.
    pub fn providers_in_order(&self) -> Vec<SubscriptionProviderRecord> {
        self.providers.values().cloned().collect()
    }

    /// Resolve a registration by its `SubscriptionProviderArn`.
    ///
    /// `ResourceNotFoundException` when no provider with that ARN is registered.
    pub fn resolve_provider(&self, arn: &str) -> Result<&SubscriptionProviderRecord, StateError> {
        self.providers
            .get(arn)
            .ok_or_else(|| StateError::NotFound(PROVIDER_NOT_FOUND_MESSAGE.to_string()))
    }

    /// The discovered instances in instance-id order.
    pub fn discovered_instances_in_order(&self) -> Vec<DiscoveredInstanceRecord> {
        self.discovered_instances.values().cloned().collect()
    }

    /// Record what one COMPLETE discovery pass found in `region`: `found` becomes
    /// everything this account has discovered THERE, so an instance the Region no longer
    /// holds stops being reported while what was discovered in every other Region is left
    /// alone — each Region's inventory is read by its own pass. A pass that did not read
    /// that whole inventory records through [`Self::merge_discovery_pass`] instead.
    ///
    /// Nothing is written for a Region this account does not discover AT THIS INSTANT
    /// ([`Self::discovers_region`]): what the store holds is decided by the settings as they
    /// are when the write lands, never by what they said when the pass that produced `found`
    /// started reading.
    ///
    /// `Instance$LastUpdatedTime` is "the time in which the last discovery updated the
    /// instance details", so it is written only for an instance this pass discovered for
    /// the first time or whose details it found CHANGED: a pass over an unchanged inventory
    /// leaves every record exactly as it was, which is what makes two reads of a list agree.
    pub fn record_discovery_pass(
        &mut self,
        region: &str,
        found: Vec<DiscoveredInstanceRecord>,
        now: SystemTime,
    ) {
        if !self.discovers_region(region) {
            return;
        }
        let timestamped = self.timestamped(found, now);
        self.discovered_instances
            .retain(|_, record| record.region != region);
        for record in timestamped {
            self.discovered_instances
                .insert(record.instance_id.clone(), record);
        }
    }

    /// Record what a pass found WITHOUT deciding what a Region holds: `found` is merged in,
    /// and a record this pass did not reach is left alone.
    ///
    /// What a pass that stopped before the end of a Region's inventory records: it did not
    /// establish that the instances it did not reach are gone, so an earlier pass's records
    /// for them stay the best evidence there is — the same treatment a Region EC2 could not
    /// be asked about at all gets.
    ///
    /// A record of a Region this account does not discover at this instant is dropped rather
    /// than merged, for the same reason [`Self::record_discovery_pass`] writes nothing for
    /// one: the one set of discovered Regions ([`Self::discovered_regions`]), read off the
    /// settings under the lock this write holds, decides it for every record at once.
    pub fn merge_discovery_pass(&mut self, found: Vec<DiscoveredInstanceRecord>, now: SystemTime) {
        let discovered = self.discovered_regions();
        for record in self.timestamped(found, now) {
            if !discovered.contains(&record.region) {
                continue;
            }
            self.discovered_instances
                .insert(record.instance_id.clone(), record);
        }
    }

    /// `found`, each record carrying the `Instance$LastUpdatedTime` it is to be stored
    /// with: `now` for one this pass discovered for the first time or found CHANGED, and
    /// the stored one for a record whose details the pass found exactly as they were.
    fn timestamped(
        &self,
        found: Vec<DiscoveredInstanceRecord>,
        now: SystemTime,
    ) -> Vec<DiscoveredInstanceRecord> {
        found
            .into_iter()
            .map(|mut record| {
                record.last_updated_time = match self.discovered_instances.get(&record.instance_id)
                {
                    Some(stored) if details_unchanged(stored, &record) => {
                        stored.last_updated_time.clone()
                    }
                    _ => Some(iso8601(now)),
                };
                record
            })
            .collect()
    }

    /// Drop what was discovered in a Region this account no longer discovers.
    ///
    /// The Regions kept are the ones the account discovers NOW
    /// ([`Self::discovered_regions`], read off the settings under the same lock this write
    /// takes rather than handed in by a caller, so a set read before an await cannot decide
    /// it), so discovery holds data for exactly those: a Region taken out of `SourceRegions`
    /// — or every Region at once, for an account whose discovery is no longer enabled and
    /// which therefore discovers none — is one nothing reads any more, and the instances a
    /// previous pass found there stop being reported rather than staying in the store as an
    /// inventory nothing refreshes.
    pub fn retain_discovered_regions(&mut self) {
        let discovered = self.discovered_regions();
        self.discovered_instances
            .retain(|_, record| discovered.contains(&record.region));
    }

    /// The Regions this account discovers right now: the `SourceRegions` its settings name
    /// while `LinuxSubscriptionsDiscovery` is [`DISCOVERY_ENABLED`], and NO Region while it
    /// is not — nor while the account has no settings at all.
    ///
    /// A Region named twice is one Region, so the set is deduplicated and in a stable
    /// order, which is the order [`Self::discovery_plan`] lays a pass's walk of them out
    /// in.
    ///
    /// The ONE place that rule is computed: the plan a pass walks, the pruning step that
    /// ends it ([`Self::retain_discovered_regions`]) and the per-hop and per-write gate
    /// ([`Self::discovers_region`]) all read this set, so the two ends of one pass cannot
    /// come to disagree about which Regions it is about.
    pub fn discovered_regions(&self) -> BTreeSet<String> {
        match self.service_settings.as_ref() {
            Some(settings) if settings.linux_subscriptions_discovery == DISCOVERY_ENABLED => {
                settings
                    .discovery_settings
                    .source_regions
                    .iter()
                    .cloned()
                    .collect()
            }
            _ => BTreeSet::new(),
        }
    }

    /// Whether this account discovers `region` right now — the one question each hop of a
    /// pass and each of its writes settles against the settings as they are then.
    ///
    /// Membership of the ONE set [`Self::discovered_regions`] computes, so the enabled gate
    /// and the Region set are stated once: the per-hop and per-write gate here and the
    /// pruning step and plan that read that set cannot come to disagree about the Regions of
    /// a single pass.
    pub fn discovers_region(&self, region: &str) -> bool {
        self.discovered_regions().contains(region)
    }

    /// The plan the NEXT discovery pass walks: every Region this account discovers right
    /// now, in the order that pass asks about them.
    ///
    /// One pass reads a bounded amount of the owner's inventory, which can be less than
    /// the Regions of its plan hold, so the plan is ROTATED to where the previous pass
    /// stopped ([`Self::discovery_resume`]): at the Region it stopped inside, or at the
    /// first Region ordered after the one whose inventory it finished. Successive passes
    /// therefore read every Region of the plan rather than the same prefix of it, and each
    /// single pass still asks about no Region twice. A recorded stopping point no Region of
    /// the plan is ordered at or after — the settings having dropped the Regions behind it
    /// — rotates to the beginning, as does no recorded stopping point at all.
    pub fn discovery_plan(&self) -> DiscoveryPlan {
        let regions: Vec<String> = self.discovered_regions().into_iter().collect();
        let Some(resume) = self.discovery_resume.as_ref() else {
            return DiscoveryPlan {
                regions,
                resume_cursor: None,
            };
        };
        let start = regions.iter().position(|region| match resume {
            // The Region the previous pass stopped INSIDE is read again, from where it
            // stopped; the one it finished is behind this pass.
            DiscoveryResume::Inside(stopped, _) => region.as_str() >= stopped.as_str(),
            DiscoveryResume::After(stopped) => region.as_str() > stopped.as_str(),
        });
        let Some(start) = start else {
            return DiscoveryPlan {
                regions,
                resume_cursor: None,
            };
        };
        // The inventory cursor belongs to the Region it was reached in, so it is handed on
        // only when the rotation really does begin at that Region: one the settings have
        // since dropped leaves this pass reading the next Region from its beginning.
        let resume_cursor = match resume {
            DiscoveryResume::Inside(stopped, cursor) if *stopped == regions[start] => {
                cursor.clone()
            }
            _ => None,
        };
        let mut rotated: Vec<String> = regions[start..].to_vec();
        rotated.extend_from_slice(&regions[..start]);
        DiscoveryPlan {
            regions: rotated,
            resume_cursor,
        }
    }

    /// Record where the pass that just ran stopped, which is where the next one resumes.
    ///
    /// `None` is what a pass that left nothing of its plan unread records: the next pass
    /// starts the plan at its beginning again.
    pub fn record_discovery_resume(&mut self, resume: Option<DiscoveryResume>) {
        self.discovery_resume = resume;
    }

    /// The discovered subscriptions: one `Subscription` per distinct (name, type) pair
    /// the discovered instances carry, each counting the instances using it that are
    /// running (`Subscription$InstanceCount`). A subscription that was discovered but has
    /// no running instance left still reports, with a count of zero — the two members say
    /// what was discovered and how much of it is running, which are different questions.
    ///
    /// Ordered by (name, type), a total order over the pairs, so a page is stable and a
    /// pagination cursor keeps its meaning between calls.
    pub fn discovered_subscriptions(&self) -> Vec<DiscoveredSubscription> {
        let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
        for instance in self.discovered_instances.values() {
            let running = i64::from(instance.status == INSTANCE_STATUS_RUNNING);
            *counts
                .entry((
                    instance.subscription_name.clone(),
                    instance.subscription_type.clone(),
                ))
                .or_default() += running;
        }
        counts
            .into_iter()
            .map(|((name, subscription_type), instance_count)| DiscoveredSubscription {
                name,
                subscription_type,
                instance_count,
            })
            .collect()
    }

    /// The metadata tags of the resource `arn` addresses, or `None` when this account
    /// holds no such resource.
    ///
    /// The tag APIs accept a `SubscriptionProviderArn`, so a registered subscription
    /// provider is the only resource of this service they can address: every tag
    /// operation resolves its `resourceArn` through this one lookup (and
    /// [`resource_tags_mut`](Self::resource_tags_mut)) rather than reaching into
    /// `providers` itself, so what one of them writes is what the next one reports.
    pub fn resource_tags(&self, arn: &str) -> Option<&HashMap<String, String>> {
        self.providers.get(arn).map(|record| &record.tags)
    }

    /// The metadata tags of the resource `arn` addresses, for a tag write to modify in
    /// place; `None` when this account holds no such resource.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        self.providers.get_mut(arn).map(|record| &mut record.tags)
    }

    /// Remove a registration from the store.
    ///
    /// Nothing else holds the order the list reports — the store's own key IS that order —
    /// so a removal cannot shift the position of any other registration, and a walk
    /// already past this one is unaffected by it.
    pub fn remove_provider(&mut self, arn: &str) -> Option<SubscriptionProviderRecord> {
        self.providers.remove(arn)
    }
}

/// Whether a pass found the same details for an instance as the ones already stored for
/// it — every member except `Instance$LastUpdatedTime`, which reports WHEN the details last
/// changed and so cannot be part of deciding whether they did.
///
/// Stated member by member rather than by comparing whole records, so which member is part
/// of "the instance details" is written down here instead of being implied by blanking one
/// of them.
fn details_unchanged(stored: &DiscoveredInstanceRecord, found: &DiscoveredInstanceRecord) -> bool {
    stored.instance_id == found.instance_id
        && stored.account_id == found.account_id
        && stored.ami_id == found.ami_id
        && stored.instance_type == found.instance_type
        && stored.status == found.status
        && stored.region == found.region
        && stored.usage_operation == found.usage_operation
        && stored.product_codes == found.product_codes
        && stored.subscription_name == found.subscription_name
        && stored.subscription_type == found.subscription_type
        && stored.os_version == found.os_version
        && stored.subscription_provider_create_time == found.subscription_provider_create_time
        && stored.subscription_provider_update_time == found.subscription_provider_update_time
        && stored.dual_subscription == found.dual_subscription
        && stored.registered_with_subscription_provider == found.registered_with_subscription_provider
}

/// A refused tag merge: the resultant set would be larger than
/// [`MAX_TAGS_PER_RESOURCE`].
///
/// [`merge_tags`] is the only thing that produces it, and each caller maps it onto the
/// answer its own surface states — which is the same refusal in both cases, since the
/// bound belongs to the tag map rather than to the operation that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TagLimitExceeded;

/// The resource segment of a `SubscriptionProviderArn`: this service's one resource kind.
pub const PROVIDER_RESOURCE_TYPE: &str = "subscription-provider";

/// The `SubscriptionProviderArn` of the registration carrying `id` in `region`.
///
/// Built through [`emulator_server::emulator_arn`], so the account segment is the
/// emulator's single canonical account id rather than a literal, and the result satisfies
/// the modelled `SubscriptionProviderArn` pattern
/// (`arn:<partition>:<service>:<region>:<account>:<type>/<id>`, every segment
/// `[a-z0-9-\.]`) for any `region` [`request_region`] admits.
pub fn provider_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        PROVIDER_ARN_SERVICE,
        region,
        &format!("{PROVIDER_RESOURCE_TYPE}/{id}"),
    )
}

/// The service segment every ARN of this service carries: its `arnNamespace`
/// (`smithy_query(query="service_info")`).
const PROVIDER_ARN_SERVICE: &str = "license-manager-linux-subscriptions";

/// The resource id of the `sequence`-th registration this account minted.
///
/// `SubscriptionProviderArn`'s pattern allows `[a-z0-9-\.]` in the resource segment and
/// the real service's ids are UUIDs, so this is UUID-shaped — 8-4-4-4-12 lowercase hex
/// with RFC 4122's version (4) and variant (8) nibbles — with the store's own monotonic
/// sequence in the node field. That makes every id unique and id order registration
/// order, so a list page and its cursor stay stable.
pub fn provider_id(sequence: u64) -> String {
    format!("00000000-0000-4000-8000-{sequence:012x}")
}

/// Format `at` the way this service's one timestamp member carries an instant:
/// ISO-8601 UTC at whole-second precision, e.g. `2026-10-24T03:43:12Z`.
///
/// `LastSuccessfulDataRetrievalTime` is a plain `String` in the model
/// (`smithy_query(query="operation",operation="GetRegisteredSubscriptionProvider")`), so
/// the value has to carry its own format; ISO-8601 UTC is what an AWS JSON timestamp
/// rendered as a string is, and it is what both operations that report this member use,
/// so a client parses one format wherever it reads it.
pub fn iso8601(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The `RegisteredSubscriptionProvider` the model describes, built entirely from the
/// stored record — every member is what registration persisted.
pub fn registered_subscription_provider(
    record: &SubscriptionProviderRecord,
) -> RegisteredSubscriptionProvider {
    RegisteredSubscriptionProvider {
        subscription_provider_arn: Some(record.arn.clone()),
        subscription_provider_source: Some(record.source.clone()),
        secret_arn: Some(record.secret_arn.clone()),
        subscription_provider_status: Some(record.status.clone()),
        subscription_provider_status_message: record.status_message.clone(),
        last_successful_data_retrieval_time: record.last_successful_data_retrieval_time.clone(),
    }
}

/// The largest number of home Regions the settings may aggregate data in: the modelled
/// `HomeRegions` list is `len 1..100`, so a 101st home Region could not be reported.
pub const MAX_HOME_REGIONS: usize = 100;

/// The largest number of metadata tags one of this service's resources carries.
///
/// The modelled `Tags` map is `len 0..50` — both as `TagResource`'s input and as
/// `ListTagsForResource`'s output — so a resource holding a 51st tag could not be
/// reported by the operation that lists its tags. A tag write whose RESULTANT set would
/// pass this bound is therefore refused instead of being stored.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Merge the pairs a request carries into one resource's `stored` tag set, or refuse the
/// write because the resultant set would not fit.
///
/// The RESULTANT set — what the resource would carry once the write landed — is the set
/// [`MAX_TAGS_PER_RESOURCE`] bounds, so a key `stored` already holds is an OVERWRITE that
/// does not grow the set while a key it does not hold does. A resultant set past the bound
/// is refused with `stored` untouched, so a refused write stores nothing; otherwise
/// `stored` is left carrying the union of both sets, with the requested value for every
/// key the request names and the value it had for every key the request does not.
///
/// Every tag write of this service merges through here — `TagResource` and
/// [`register_provider`](ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState::register_provider)
/// — so the bound is decided in ONE place and the two callers cannot drift apart.
pub fn merge_tags(
    stored: &mut HashMap<String, String>,
    requested: HashMap<String, String>,
) -> Result<(), TagLimitExceeded> {
    let resultant = stored.len()
        + requested
            .keys()
            .filter(|key| !stored.contains_key(*key))
            .count();
    if resultant > MAX_TAGS_PER_RESOURCE {
        return Err(TagLimitExceeded);
    }
    stored.extend(requested);
    Ok(())
}

/// What `GetServiceSettings` and `UpdateServiceSettings` report for this account.
///
/// Both operations answer with the same five members, so the mapping from the stored
/// singleton onto them lives here once. A member the account has nothing for is `None`
/// rather than an invented value.
#[derive(Debug, Clone, Default)]
pub struct ReportedServiceSettings {
    /// `LinuxSubscriptionsDiscovery`.
    pub linux_subscriptions_discovery: Option<String>,
    /// `LinuxSubscriptionsDiscoverySettings`, absent until an update persisted one.
    pub discovery_settings: Option<LinuxSubscriptionsDiscoverySettings>,
    /// `Status`, absent until a settings change was applied: the member reports the
    /// status OF that change, so an account that never requested one has no status.
    pub status: Option<String>,
    /// `StatusMessage`, absent while the service has nothing to report.
    pub status_message: Option<HashMap<String, String>>,
    /// `HomeRegions`, absent until an update made a Region the home Region.
    pub home_regions: Option<Vec<String>>,
}

/// The settings to report for this account: the persisted singleton, or the default an
/// account that never called `UpdateServiceSettings` has — discovery `Disabled`, with no
/// discovery settings, no status and no home Region, since nothing has been configured.
///
/// `LinuxSubscriptionsDiscovery` is the one member the default branch fills, because
/// discovery being off is the state of an account that never enabled it. `Status` reports
/// on a settings change having been applied, and no change has been requested on such an
/// account, so it is absent like the other members nothing was stored for.
pub fn reported_service_settings(
    settings: Option<&ServiceSettingsRecord>,
) -> ReportedServiceSettings {
    let Some(settings) = settings else {
        return ReportedServiceSettings {
            linux_subscriptions_discovery: Some(DISCOVERY_DISABLED.to_string()),
            discovery_settings: None,
            status: None,
            status_message: None,
            home_regions: None,
        };
    };
    ReportedServiceSettings {
        linux_subscriptions_discovery: Some(settings.linux_subscriptions_discovery.clone()),
        discovery_settings: Some(settings.discovery_settings.clone()),
        status: Some(settings.status.clone()),
        status_message: Some(settings.status_message.clone())
            .filter(|message| !message.is_empty()),
        home_regions: Some(settings.home_regions.clone()).filter(|regions| !regions.is_empty()),
    }
}

/// The Region a request addressed to this service is served in: the Region its SigV4
/// credential scope names, or [`DEFAULT_REGION`] when the request carries no USABLE
/// scope.
///
/// The scope is caller-supplied text, and this is what the service then RECORDS — the
/// account's home Region and the Region segment of every minted
/// `SubscriptionProviderArn` — so a scope is usable only when its Region names an AWS
/// Region ([`is_known_region`]), exactly the set a Region-valued member of this service
/// may name. A scope naming anything else resolves to [`DEFAULT_REGION`], like a request
/// carrying no scope at all: nothing the store holds, and nothing an ARN carries, is then
/// a Region code that does not exist. The catalog's codes are also inside the ARN
/// grammar's region segment (`[a-z0-9-.]{1,63}`), so a minted ARN still satisfies the
/// modelled `SubscriptionProviderArn` pattern.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_known_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request that resolves to none of its own is served in.
///
/// It is a Region of the catalog [`is_known_region`] checks, so the fallback itself
/// cannot put a Region code the catalog does not hold into the store.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Whether `region` names an AWS Region.
///
/// The catalog is `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the
/// wire may carry, sorted for binary search", maintained with the backend contract and
/// already a dependency of this crate) rather than a list copied into this service: this
/// service models no Region enum — `SourceRegions` is a plain `StringList` — so the set
/// a value may name is exactly that shared catalog, and it must not drift from it. The
/// lookup is the binary search the catalog's ordering is maintained for.
pub fn is_known_region(region: &str) -> bool {
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .binary_search(&region)
        .is_ok()
}

/// One page of `items`, plus the token that reaches the next one.
///
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning. A page size below 1 would make no progress, so it is
/// refused too.
///
/// A cursor names the ELEMENT the previous page reported LAST, by `cursor_key` — the
/// element's own identity in the list's order — and the next page starts at the first
/// element ordered after it. It is NOT a count of the elements already reported: the store
/// a list projects can be written between two pages of one walk (a resource removed,
/// another created), and a position re-resolved against the changed sequence would carry
/// an element the walk never reported across the cursor and skip it, or report one twice.
/// Resuming after a named element also ends the walk correctly when that element is the
/// one the write removed.
///
/// `items` must therefore be in ascending `cursor_key` order and the key must identify an
/// element, which is how every list of this service is held: each is ordered by the key it
/// is stored or grouped under.
pub fn paginate<T: Clone>(
    items: &[T],
    kind: &str,
    cursor_key: impl Fn(&T) -> Vec<String>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), StateError> {
    debug_assert!(
        items
            .windows(2)
            .all(|pair| cursor_key(&pair[0]) < cursor_key(&pair[1])),
        "a paged list must be in ascending cursor-key order"
    );
    let page_size = match max_results {
        Some(max) if max < 1 => {
            return Err(StateError::Validation(format!(
                "MaxResults must be at least 1, got {max}"
            )))
        }
        Some(max) => max as usize,
        None => items.len(),
    };
    // A cursor whose element is ordered past every element left in the list resumes at the
    // end of it: the walk reported everything it could and is over.
    let start = match next_token {
        Some(token) => {
            let reported_last = decode_page_token(kind, token)?;
            items
                .iter()
                .position(|item| cursor_key(item) > reported_last)
                .unwrap_or(items.len())
        }
        None => 0,
    };
    let end = start.saturating_add(page_size).min(items.len());
    let page = items[start..end].to_vec();
    let token = (end < items.len())
        .then(|| encode_page_token(kind, &cursor_key(&items[end - 1])));
    Ok((page, token))
}

/// This service's opaque pagination cursor: the identity of the list it belongs to and the
/// key of the element the page that minted it reported last.
///
/// Hex over a JSON pair rather than a hand-rolled framing, so the DECODE below is
/// `serde_json`'s — a cursor is caller-supplied input, and the typed shape it has to parse
/// into is what decides whether it is one. Hex is an encoding, not a secret: a client can
/// decode one. What makes a cursor trustworthy here is the list-identity check in
/// [`decode_page_token`] — a token is only honoured by the list that minted it, so a forged
/// or stale one is refused rather than applied to a different list.
///
/// The identity bound into the token is [`list_identity`]'s FIXED-SIZE digest of `kind`,
/// never `kind` itself, so the token a page mints stays the same size however large the
/// request that selected the list was.
fn encode_page_token(kind: &str, key: &[String]) -> String {
    let token = ::hex::encode(
        ::serde_json::to_vec(&(list_identity(kind), key))
            .expect("a list identity and a cursor key serialize"),
    );
    debug_assert!(
        token.len() <= MAX_NEXT_TOKEN,
        "a minted cursor must fit the modelled NextToken maximum of {MAX_NEXT_TOKEN}, got {}",
        token.len()
    );
    token
}

/// Decode a cursor minted by [`encode_page_token`] for the same list, into the key of the
/// element the minting page reported last.
fn decode_page_token(kind: &str, token: &str) -> Result<Vec<String>, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    let (token_identity, key): (String, Vec<String>) =
        ::serde_json::from_slice(&decoded).map_err(|_| invalid())?;
    if token_identity != list_identity(kind) {
        return Err(invalid());
    }
    Ok(key)
}

/// The identity of the list a cursor belongs to: a fixed-size digest of `kind`, the list's
/// full description (the operation plus whatever selected it).
///
/// A cursor is minted by the service and SENT BACK by the client, so its length is bounded
/// by the model — `NextToken` is `@length 1..16384` on the `Filters`-driven lists — while
/// `kind` renders the whole selection that produced the list, which a selection the model
/// allows (`FilterList` carries no `@length`, over `StringList` values of up to 100 entries
/// of up to 100 characters) can make arbitrarily long. Binding the list into the token as
/// SHA-256 over that description makes the minted cursor's size independent of the request's
/// size, so a walk the service started can always be continued, and still binds the cursor
/// to ONE list: two different descriptions share a digest only on a hash collision, so a
/// cursor minted for another selection is refused exactly as before.
///
/// SHA-256 comes from the `sha2` crate rather than being hand-rolled.
fn list_identity(kind: &str) -> String {
    use ::sha2::Digest as _;
    ::hex::encode(::sha2::Sha256::digest(kind.as_bytes()))
}

/// The longest `NextToken` a client may send back to this service, from the model:
/// `ListLinuxSubscriptionsRequest$NextToken` and
/// `ListLinuxSubscriptionInstancesRequest$NextToken` are `@length 1..16384` (the cursor
/// `ListRegisteredSubscriptionProviders` takes carries no bound of its own, so this is the
/// tightest one every minted cursor of this service has to satisfy).
pub const MAX_NEXT_TOKEN: usize = 16384;

/// A deterministic rendering of the `SubscriptionProviderSources` selection, for the
/// list identity a pagination cursor is minted against: a cursor names an element of ONE
/// list, so it must not be accepted for a list selected by a different filter.
///
/// Every component is LENGTH-PREFIXED, so no two different selections can render to the
/// same signature.
pub fn source_filter_signature(sources: &[String]) -> String {
    length_prefixed(sources.iter().cloned())
}

/// The same rendering for a `FilterList` selection, so the `Filters`-driven lists get
/// cursors that name an element of the list THAT selection produced.
///
/// Each filter contributes its name, its operator and its values, every component
/// length-prefixed, closed by the number of values it carried — so neither a different
/// grouping of the same components nor a different operator can render the same
/// signature.
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        signature.push_str(&length_prefixed(
            [
                filter.name.clone().unwrap_or_default(),
                filter.operator.clone().unwrap_or_default(),
            ]
            .into_iter()
            .chain(filter.values.clone().unwrap_or_default()),
        ));
        signature.push_str(&format!("/{}", filter.values.as_ref().map_or(0, Vec::len)));
    }
    signature
}

/// Render `components` so that no two different sequences render alike: each is prefixed
/// with its own length.
fn length_prefixed(components: impl IntoIterator<Item = String>) -> String {
    let mut rendered = String::new();
    for component in components {
        rendered.push_str(&format!("{}.{component}", component.len()));
    }
    rendered
}

/// The comparison an absent `Filter$Operator` means: `Filters` selects the results that
/// "match one or more sets of key-value pairs that you specify", so a filter with no
/// operator selects on equality (`Operator`: Contains | Equal | NotEqual).
pub const OPERATOR_EQUAL: &str = "Equal";

/// `Operator` for a filter that selects what does NOT equal any of its values.
pub const OPERATOR_NOT_EQUAL: &str = "NotEqual";

/// `Operator` for a filter that selects on a substring of the member's value.
pub const OPERATOR_CONTAINS: &str = "Contains";

/// Refuse a filter the list it was given to cannot apply.
///
/// `valid_names` is the set of `Filter$Name` values that list documents: a filter naming
/// anything else addresses no member, so it can select nothing and is a
/// `ValidationException` rather than a silently empty page. A filter carrying no value
/// has nothing to compare against (the modelled `Values` list is `len 1..100`), and an
/// operator outside the modelled `Operator` set names no comparison.
pub fn validate_filters(filters: &[Filter], valid_names: &[&str]) -> Result<(), StateError> {
    for filter in filters {
        let name = filter.name.as_deref().unwrap_or_default();
        if !valid_names.contains(&name) {
            return Err(StateError::Validation(format!(
                "Filters.Name must be one of {valid_names:?}, got {name:?}"
            )));
        }
        if filter.values.as_ref().is_none_or(Vec::is_empty) {
            return Err(StateError::Validation(format!(
                "Filters.Values must carry at least one value to filter {name:?} by"
            )));
        }
        if let Some(operator) = filter.operator.as_deref() {
            if !OPERATOR_VALUES.contains(&operator) {
                return Err(StateError::Validation(format!(
                    "Filters.Operator must be one of {OPERATOR_VALUES:?}, got {operator:?}"
                )));
            }
        }
    }
    Ok(())
}

/// Does one validated `filter` select a resource whose addressed member holds
/// `member_values`?
///
/// `member_values` is what the member the filter's `Name` addresses holds on that
/// resource: one value for a scalar member, several for a list member (`ProductCode`),
/// none when the resource leaves it unset. A list member satisfies the filter when any of
/// its values does, which is what makes `NotEqual` mean "no value of this member equals
/// any value I named".
///
/// Every operator compares the values the member HOLDS against the values the filter
/// names, so a resource holding none of them satisfies no filter at all: there is nothing
/// on it to equal, to differ from, or to contain what was named. `Equal` and `NotEqual`
/// therefore partition the resources that hold the addressed member, and neither reaches
/// one that leaves it unset.
pub fn filter_selects(filter: &Filter, member_values: &[String]) -> bool {
    if member_values.is_empty() {
        return false;
    }
    let values = filter.values.as_deref().unwrap_or_default();
    let equals = member_values.iter().any(|held| values.contains(held));
    match filter.operator.as_deref().unwrap_or(OPERATOR_EQUAL) {
        OPERATOR_CONTAINS => member_values
            .iter()
            .any(|held| values.iter().any(|value| held.contains(value.as_str()))),
        OPERATOR_NOT_EQUAL => !equals,
        // `Equal`, and the equality an absent operator means.
        _ => equals,
    }
}

/// Does one resource satisfy EVERY filter a `Filters`-driven list was given?
///
/// `Filters` selects the results "that match one or more sets of key-value pairs that you
/// specify", so several filters select what satisfies ALL of them — and an empty
/// `Filters` selects every resource, because there is nothing left for one to fail. This
/// is the whole of how filters COMBINE, held here once, so no list can grow its own answer
/// to it.
///
/// `member_values` is the only part a list states for itself: what the member a
/// `Filter$Name` addresses holds on the resource being considered — one value for a scalar
/// member, every value for a list member, and none for a member the resource leaves unset
/// (which [`filter_selects`] then selects nothing on, whatever the operator). Only a name
/// the list documents reaches it, because [`validate_filters`] has already refused the
/// others.
pub fn filters_select(filters: &[Filter], member_values: impl Fn(&str) -> Vec<String>) -> bool {
    filters.iter().all(|filter| {
        let name = filter.name.as_deref().unwrap_or_default();
        filter_selects(filter, &member_values(name))
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>)
}
