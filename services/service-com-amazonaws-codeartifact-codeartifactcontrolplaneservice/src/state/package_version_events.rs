// SPDX-License-Identifier: CC0-1.0

//! The event a package version's own state change announces, and how it reaches the
//! account's default event bus.
//!
//! CodeArtifact announces a package version's state change on the DEFAULT EventBridge bus
//! of the domain owner's account: one event per change, `source` [`EVENT_SOURCE`],
//! `detail-type` [`PACKAGE_VERSION_STATE_CHANGE`], `resources` naming the PACKAGE the
//! version belongs to, and a `detail` carrying the version's coordinate, the state and
//! revision it now has, what the change did to its content, and which change of that
//! version this is. A rule on that bus matching the pair therefore reaches its targets
//! without anything polling the control plane.
//!
//! Every member of the event is read off the records the change itself wrote
//! ([`super::PackageVersion`]), so the revision an event announces is the revision
//! `DescribePackageVersion` reports for the same version and the state it announces is the
//! state `ListPackageVersions` lists it in. Nothing here derives a value from the request
//! that caused the change.
//!
//! ## How a change gets here
//!
//! A change is RECORDED while the store's lock is held
//! ([`super::ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::record_version_change`])
//! and PUBLISHED after it is released ([`publish`]), so no lock is held across the
//! in-process call into EventBridge. The queue is what makes the two halves independent: an
//! operation that records several changes publishes them together, and the drain takes them
//! off the queue, so no change is announced twice.
//!
//! Publication is BEST EFFORT, as the announcement itself is: the state change is already
//! committed and the operation's own answer does not depend on a bus being mounted, so a
//! publication that could not be made is logged and nothing is retried — an emulator with
//! no event bus mounted serves CodeArtifact exactly as it did before.

use ::serde_json::{json, Value};
use ::std::time::SystemTime;

use super::{Package, PackageVersion, Repository};

/// The `source` every CodeArtifact event carries.
pub const EVENT_SOURCE: &str = "aws.codeartifact";

/// The `detail-type` of the event a package version's state change announces.
pub const PACKAGE_VERSION_STATE_CHANGE: &str = "CodeArtifact Package Version State Change";

/// The bus an AWS service's own events go to: the account's default event bus.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// `operationType` for a version that did not exist in the repository before this change.
pub const OPERATION_CREATED: &str = "Created";

/// `operationType` for a change to a version the repository already held.
pub const OPERATION_UPDATED: &str = "Updated";

/// `operationType` for a version the repository no longer holds after the change.
pub const OPERATION_DELETED: &str = "Deleted";

/// How many changes one drain publishes.
///
/// This module's OWN bound, the companion to [`MAX_PENDING_VERSION_EVENTS`]: that bounds
/// what the queue holds, this bounds what one operation's drain turns into in-process
/// requests. What a drain leaves behind stays queued and is published by the next one, so a
/// burst is spread over calls rather than dropped. The figure is far above the changes one
/// request can make (a publish document's versions, a copy request's versions — each itself
/// capped by [`super::MAX_VERSIONS_PER_COPY_REQUEST`]).
const MAX_PUBLISHED_PER_DRAIN: usize = 200;

/// How many entries one `PutEvents` request carries: EventBridge models
/// `PutEvents$Entries` as `length 1..10`, so a drain holding more is published over several
/// requests rather than one the bus would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes the queue holds before the oldest make room for a new one.
///
/// The queue is drained by the very operation that fills it, so it holds a request's worth
/// of changes and not a workload's; this is the backstop for a store whose events nothing
/// ever drains (an emulator with no bus mounted), so the records cannot grow without end.
pub const MAX_PENDING_VERSION_EVENTS: usize = 10_000;

/// What one change did to a package version's content, as the event's `changes` object
/// states it.
///
/// Counts of ASSETS, plus whether the version's metadata document and its status moved.
/// Every field is a fact about the change that was just committed, so a consumer deciding
/// whether to re-scan a version reads it rather than diffing two reads of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionChanges {
    /// How many assets the change added to the version.
    pub assets_added: i64,
    /// How many it removed.
    pub assets_removed: i64,
    /// How many it replaced with different content under the same name.
    pub assets_updated: i64,
    /// Whether the version's metadata document was written by the change.
    pub metadata_updated: bool,
    /// Whether the version's status is not the status it had before the change — true for
    /// a version the change created, which had no status at all before it.
    pub status_changed: bool,
}

impl VersionChanges {
    /// The change a PUBLISH of a version makes: its assets arrive, and it comes to rest in
    /// a status it did not have before. `metadata_written` is whether the version's
    /// metadata document arrived with them — a package manager's publish carries one, the
    /// generic publish API does not.
    pub fn published(assets: usize, metadata_written: bool) -> Self {
        Self {
            assets_added: i64::try_from(assets).unwrap_or(i64::MAX),
            metadata_updated: metadata_written,
            status_changed: true,
            ..Self::default()
        }
    }

    /// The change a STATUS move makes: nothing about the content, only the status.
    pub fn status_moved() -> Self {
        Self {
            status_changed: true,
            ..Self::default()
        }
    }

    /// This object as the event states it.
    fn detail(&self) -> Value {
        json!({
            "assetsAdded": self.assets_added,
            "assetsRemoved": self.assets_removed,
            "assetsUpdated": self.assets_updated,
            "metadataUpdated": self.metadata_updated,
            "statusChanged": self.status_changed,
        })
    }
}

/// One package-version state change, as an event announces it.
///
/// Built from the stored records by [`Self::of`] (or [`Self::deleted`]), so nothing in it
/// is read from the request that caused the change.
#[derive(Debug, Clone)]
pub struct VersionChange {
    /// The Region the repository's own ARN names, which is the Region the event is
    /// published in and the one the package ARN carries.
    region: String,
    domain: String,
    domain_owner: String,
    repository: String,
    repository_administrator: String,
    format: String,
    /// `None` for a package whose format carries no namespace component, and for an
    /// un-scoped npm package — reported as `null`, which is what "this package has no
    /// namespace" is.
    namespace: Option<String>,
    package: String,
    version: String,
    version_state: String,
    version_revision: String,
    operation_type: String,
    changes: VersionChanges,
    sequence_number: i64,
    at: SystemTime,
}

impl VersionChange {
    /// The change `version` of `package` in `repository` has just undergone, as the event
    /// announces it.
    ///
    /// The state, the revision and the sequence number are the STORED version's own, so an
    /// event cannot announce a revision no read of that version reports.
    pub fn of(
        repository: &Repository,
        package: &Package,
        version: &PackageVersion,
        operation_type: &str,
        changes: VersionChanges,
        at: SystemTime,
    ) -> Self {
        Self {
            region: arn_region(&repository.arn),
            domain: repository.domain_name.clone(),
            domain_owner: repository.domain_owner.clone(),
            repository: repository.name.clone(),
            repository_administrator: repository.administrator_account.clone(),
            format: package.format.clone(),
            namespace: package.namespace.clone(),
            package: package.name.clone(),
            version: version.version.clone(),
            version_state: version.status.clone(),
            version_revision: version.revision(),
            operation_type: operation_type.to_string(),
            changes,
            sequence_number: version.sequence_number,
            at,
        }
    }

    /// The change a DELETION announces: the record as it last stood, in the state a
    /// version the repository no longer holds is in, with its assets counted as removed.
    ///
    /// The sequence number is the next one after the record's, because the record itself is
    /// gone and cannot carry it.
    pub fn deleted(
        repository: &Repository,
        package: &Package,
        version: &PackageVersion,
        at: SystemTime,
    ) -> Self {
        let changes = VersionChanges {
            assets_removed: i64::try_from(version.assets.len()).unwrap_or(i64::MAX),
            status_changed: true,
            ..VersionChanges::default()
        };
        Self {
            version_state: super::PACKAGE_VERSION_STATUS_DELETED.to_string(),
            sequence_number: version.sequence_number.saturating_add(1),
            ..Self::of(
                repository,
                package,
                version,
                OPERATION_DELETED,
                changes,
                at,
            )
        }
    }

    /// The ARN of the PACKAGE this version belongs to — what the event names in
    /// `resources`.
    ///
    /// Rendered by the service's one package-ARN rule
    /// ([`crate::state::package_arn`]), which is also the resource a package-scoped action is
    /// authorized against: an event and a refusal must name one package by one ARN.
    pub fn package_arn(&self) -> String {
        super::package_arn(
            &self.region,
            &self.domain,
            &self.repository,
            &self.format,
            self.namespace.as_deref(),
            &self.package,
        )
    }

    /// The event's `detail`.
    pub fn detail(&self) -> Value {
        json!({
            "domainName": self.domain,
            "domainOwner": self.domain_owner,
            "repositoryName": self.repository,
            "repositoryAdministrator": self.repository_administrator,
            "packageFormat": self.format,
            "packageNamespace": self.namespace,
            "packageName": self.package,
            "packageVersion": self.version,
            "packageVersionState": self.version_state,
            "packageVersionRevision": self.version_revision,
            "changes": self.changes.detail(),
            "operationType": self.operation_type,
            "sequenceNumber": self.sequence_number,
            "eventDeduplicationId": self.deduplication_id(),
        })
    }

    /// `eventDeduplicationId`: the token a consumer tells a repeat of the SAME
    /// announcement by.
    ///
    /// Derived from the change itself — the version's whole coordinate, the state and
    /// revision it reached, which change of the version this is, and what it did — so one
    /// change always carries one id and two different changes never share one. It is not
    /// derived from the clock or from the request, which would make a repeat of the same
    /// announcement look like a new one.
    fn deduplication_id(&self) -> String {
        use ::base64::Engine as _;

        let changes = format!(
            "{}:{}:{}:{}:{}",
            self.changes.assets_added,
            self.changes.assets_removed,
            self.changes.assets_updated,
            self.changes.metadata_updated,
            self.changes.status_changed
        );
        let sequence = self.sequence_number.to_string();
        let digest = super::identity_digest(&[
            self.domain.as_str(),
            self.domain_owner.as_str(),
            self.repository.as_str(),
            self.format.as_str(),
            self.namespace.as_deref().unwrap_or_default(),
            self.package.as_str(),
            self.version.as_str(),
            self.version_state.as_str(),
            self.version_revision.as_str(),
            self.operation_type.as_str(),
            sequence.as_str(),
            changes.as_str(),
        ]);
        ::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }

    /// WHEN the version changed, which is the instant the event states.
    fn at(&self) -> SystemTime {
        self.at
    }
}

/// The Region an ARN this service minted names, or [`super::DEFAULT_REGION`] when the
/// value is not an ARN this service could have minted.
///
/// The repository's own ARN is the one place the Region a record lives in is kept: it was
/// minted from the request that created the repository, so the package ARN the event names
/// and the repository ARN a client already holds agree on the Region.
fn arn_region(arn: &str) -> String {
    ::service_common::arn::parse(arn)
        .map(|parsed| parsed.region.to_string())
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| super::DEFAULT_REGION.to_string())
}

/// Announce every recorded change this store holds, on the caller's own path.
///
/// The ENTRY POINT every operation that changed a package version calls, once it has
/// written the change AND released the store's lock: a lock over state shared between
/// requests may not be held across an await, which is what the one `{ … }` block each
/// handler keeps its store work in is for. Nothing is spawned — the announcement is made
/// on the request's own task — so a change is on the bus by the time the operation answers
/// and nothing can be left unannounced by a task that never ran.
pub async fn publish<Eng: Send + Sync + 'static>(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
) {
    publish_through(ctx.callback(), engine).await;
}

/// Publish every recorded change this store holds onto the account's default event bus,
/// and leave the queue empty.
///
/// The announcement itself, reached from [`announce`]'s task — and directly by a test that
/// drives the publication without a runtime of its own. EventBridge is reached the way any
/// emulated service reaches another, through the request's own callback, so this crate
/// names no other service crate.
pub async fn publish_through<Eng: Send + Sync + 'static>(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    // The guard is taken and dropped inside this statement: the records move out here, so
    // nothing is announced twice and no lock is held across the await below.
    let changes = state
        .lock()
        .take_pending_version_changes(MAX_PUBLISHED_PER_DRAIN);
    if changes.is_empty() {
        return;
    }
    // One client per Region the changes name: the bus an event goes to is the one in the
    // Region the changed resource lives in.
    let mut regions: ::std::collections::BTreeMap<String, Vec<VersionChange>> =
        ::std::collections::BTreeMap::new();
    for change in changes {
        regions
            .entry(change.region.clone())
            .or_default()
            .push(change);
    }
    for (region, changes) in regions {
        let client = event_bridge(callback, &region);
        for window in changes.chunks(MAX_PUT_EVENTS_ENTRIES) {
            let mut request = client.put_events();
            for change in window {
                let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS)
                    .source(EVENT_SOURCE)
                    .detail_type(PACKAGE_VERSION_STATE_CHANGE)
                    // The event's `time` is WHEN the version changed, not when the queue
                    // was drained: `PutEventsRequestEntry$Time` is "the time stamp of the
                    // event, per RFC3339. If no time stamp is provided, the time stamp of
                    // the PutEvents call is used."
                    .time(::aws_sdk_eventbridge::primitives::DateTime::from(change.at()))
                    .resources(change.package_arn())
                    .detail(change.detail().to_string())
                    .build();
                request = request.entries(entry);
            }
            match request.send().await {
                Ok(answer) if answer.failed_entry_count() == 0 => {}
                Ok(answer) => ::tracing::warn!(
                    failed_entry_count = answer.failed_entry_count(),
                    "the event bus refused a package version state change"
                ),
                Err(error) => ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a package version state change could not be published to the event bus"
                ),
            }
        }
    }
}

/// The EventBridge client this service publishes through: the emulator's own in-process
/// transport and credentials, addressed to the bus endpoint of `region`.
fn event_bridge(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_eventbridge::Client {
    let config = ::aws_sdk_eventbridge::Config::builder()
        .behavior_version(::aws_sdk_eventbridge::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_eventbridge::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://events.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_eventbridge::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eventbridge::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eventbridge::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_eventbridge::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_eventbridge::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository record to build a change against, with the ARN a repository of
    /// `region` carries.
    fn repository(region: &str) -> Repository {
        Repository {
            name: "repo".to_string(),
            domain_name: "acme".to_string(),
            domain_owner: "000000000000".to_string(),
            administrator_account: "000000000000".to_string(),
            arn: super::super::repository_arn(region, "acme", "repo"),
            description: None,
            upstreams: Vec::new(),
            external_connections: Vec::new(),
            created_time: SystemTime::UNIX_EPOCH,
            tags: Vec::new(),
            policy: None,
            packages: ::std::collections::BTreeMap::new(),
        }
    }

    /// A version record with one asset.
    fn version(status: &str) -> PackageVersion {
        PackageVersion {
            version: "1.0.0".to_string(),
            status: status.to_string(),
            published_time: SystemTime::UNIX_EPOCH,
            origin_type: super::super::PACKAGE_VERSION_ORIGIN_INTERNAL.to_string(),
            entry_point_repository: "repo".to_string(),
            assets: ::std::collections::BTreeMap::from([(
                super::super::NPM_TARBALL_ASSET.to_string(),
                super::super::Asset::new(super::super::NPM_TARBALL_ASSET, b"tarball".to_vec()),
            )]),
            manifest: Some(r#"{"name":"left-pad","version":"1.0.0"}"#.to_string()),
            readme: None,
            sequence_number: 1,
        }
    }

    /// RULE: the package ARN an event names carries every segment of the package's
    /// coordinate positionally, so a package with no namespace leaves that segment empty.
    /// The rows span the namespace dimension and the Region the repository lives in.
    #[test]
    fn the_package_arn_carries_the_whole_coordinate() {
        let rows: [(&str, &str, Option<&str>); 3] = [
            ("un-scoped npm", "us-west-2", None),
            ("scoped npm", "us-west-2", Some("acme")),
            ("another region", "eu-central-1", Some("acme")),
        ];
        for (case, region, namespace) in rows {
            let package = Package::new(super::super::PACKAGE_FORMAT_NPM, namespace, "left-pad");
            let change = VersionChange::of(
                &repository(region),
                &package,
                &version(super::super::PACKAGE_VERSION_STATUS_PUBLISHED),
                OPERATION_CREATED,
                VersionChanges::published(1, true),
                SystemTime::UNIX_EPOCH,
            );
            let expected = format!(
                "arn:aws:codeartifact:{region}:000000000000:package/acme/repo/npm/{}/left-pad",
                namespace.unwrap_or_default()
            );
            assert_eq!(
                change.package_arn(),
                expected,
                "case {case:?}: expected the package ARN {expected}, got {}",
                change.package_arn()
            );
        }
    }

    /// RULE: the deduplication id is a function of the change, so the same change states
    /// the same id and a different one states a different id.
    #[test]
    fn the_deduplication_id_follows_the_change() {
        let package = Package::new(super::super::PACKAGE_FORMAT_NPM, None, "left-pad");
        let published = version(super::super::PACKAGE_VERSION_STATUS_PUBLISHED);
        let created = |at| {
            VersionChange::of(
                &repository("us-west-2"),
                &package,
                &published,
                OPERATION_CREATED,
                VersionChanges::published(1, true),
                at,
            )
            .deduplication_id()
        };
        assert_eq!(
            created(SystemTime::UNIX_EPOCH),
            created(SystemTime::now()),
            "case \"the same change\": the id must not depend on the clock, got {} and {}",
            created(SystemTime::UNIX_EPOCH),
            created(SystemTime::now())
        );
        let unlisted = VersionChange::of(
            &repository("us-west-2"),
            &package,
            &version(super::super::PACKAGE_VERSION_STATUS_UNLISTED),
            OPERATION_UPDATED,
            VersionChanges::status_moved(),
            SystemTime::UNIX_EPOCH,
        )
        .deduplication_id();
        assert_ne!(
            created(SystemTime::UNIX_EPOCH),
            unlisted,
            "case \"a different change\": two changes must not share an id, both are \
             {unlisted}"
        );
    }

    /// RULE: a deletion announces the version in the state a version the repository no
    /// longer holds is in, with its assets counted as removed and the next sequence number.
    #[test]
    fn a_deletion_announces_the_record_it_removed() {
        let package = Package::new(super::super::PACKAGE_FORMAT_NPM, None, "left-pad");
        let held = version(super::super::PACKAGE_VERSION_STATUS_PUBLISHED);
        let change = VersionChange::deleted(
            &repository("us-west-2"),
            &package,
            &held,
            SystemTime::UNIX_EPOCH,
        );
        let detail = change.detail();
        for (pointer, expected) in [
            (
                "/packageVersionState",
                json!(super::super::PACKAGE_VERSION_STATUS_DELETED),
            ),
            ("/operationType", json!(OPERATION_DELETED)),
            ("/changes/assetsRemoved", json!(1)),
            ("/changes/statusChanged", json!(true)),
            ("/sequenceNumber", json!(2)),
        ] {
            let actual = detail.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case \"a deleted version\": expected {pointer} to be {expected}, got \
                 {actual} in {detail}"
            );
        }
    }
}
