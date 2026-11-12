// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-snowball-awsiesnowballjobmanagementservice service.
//!
//! Besides the state struct itself this module holds the helpers the job, address and
//! long-term-pricing handlers share — id minting in the modelled formats, the
//! record-to-modelled-shape conversions and the list cursor codec — so the handlers agree
//! on one implementation of each instead of carrying a copy apiece that can drift apart.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Resolving the AWS resources an order NAMES — the buckets it transfers, the role it
/// transfers them under, the key it is encrypted with, the images and functions it carries,
/// the topics it notifies — against the services that hold them. Shared by every operation
/// that places or updates an order, so one rule covers all of them.
pub mod named_resources;

/// The Amazon SNS notification a job's state change is published as: the one reading of a
/// job's `Notification` settings, shared by every operation that moves a job, so each of
/// them publishes the same document onto the same topic.
pub mod job_notifications;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Address, ClusterListEntry, ClusterMetadata, CompatibleImage, CreateClusterInput, CreateJobInput,
    DeviceConfiguration, InvalidResourceException, JobListEntry, JobMetadata, JobResource,
    LongTermPricingListEntry, Notification, OnDeviceServiceConfiguration, PickupDetails,
    ServiceVersion, ShippingDetails, TaxDocuments, UpdateClusterInput, UpdateJobInput,
};

/// What a handler says when the engine it was constructed with does not carry this
/// service's store.
///
/// The router always builds the emulator on `engine::InMemoryEngine`, so this is
/// unreachable there; a handler must still answer an error rather than construct a
/// private engine, whose writes nobody else would ever see. This service's model
/// declares no server-side error on any of these operations, so it travels on the
/// framework's refusal channel (`Scaffolded::validation`) rather than as a 501, which
/// would claim the operation is unimplemented.
pub const STATE_UNAVAILABLE: &str =
    "the Snowball emulator's state store is not available on this engine";

/// The page size a list operation uses when the caller declares none.
///
/// `MaxResults` is modelled `range 0..100` on every list operation of this service that
/// takes one (`smithy_query(query="operation", operation="ListJobs")`,
/// `operation="ListServiceVersions"`), so 100 is the largest page the member admits.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The `ServiceName` of Kubernetes as a Snow on-device service
/// (`shared_types::SERVICE_NAME_VALUES`).
pub const ON_DEVICE_SERVICE_KUBERNETES: &str = "KUBERNETES";

/// The `ServiceName` of Amazon EKS Anywhere as a Snow on-device service
/// (`shared_types::SERVICE_NAME_VALUES`).
pub const ON_DEVICE_SERVICE_EKS_ANYWHERE: &str = "EKS_ANYWHERE";

/// The versions of Amazon EKS Anywhere this emulator reports as supported on a Snow
/// Family device — newest first — each paired with the Kubernetes minor versions that
/// release runs, newest first.
///
/// Source: the Snow Family documentation for Amazon EKS Anywhere on Snow ("Using Amazon
/// EKS Anywhere on Snow Family devices", AWS Snowball Edge Developer Guide), whose
/// version table publishes the EKS Anywhere releases available on Snow together with the
/// Kubernetes versions each release supports: successive releases move the supported
/// window forward one Kubernetes minor at a time, and the earliest release on Snow
/// supports a single Kubernetes minor. The matrix below is that table as it stood for the
/// EKS Anywhere 0.11–0.15 releases, the releases that shipped on Snow when this catalog
/// was recorded; a later revision of the guide extends the table, so refresh this matrix
/// from it rather than assuming it is current. The concrete versions a deployment offers
/// are exactly what `ListServiceVersions` reports, which is why a caller reads them from
/// this operation rather than assuming them, and this matrix is what this emulator holds
/// to report.
///
/// This is the ONE place the support matrix is written down: the Kubernetes catalog is
/// derived from it by [`seed_service_versions`], so the two services can never report
/// version sets that contradict each other.
const EKS_ANYWHERE_SUPPORT_MATRIX: &[(&str, &[&str])] = &[
    ("0.15.0", &["1.25", "1.24", "1.23"]),
    ("0.14.0", &["1.24", "1.23", "1.22"]),
    ("0.13.0", &["1.23", "1.22", "1.21"]),
    ("0.12.0", &["1.22", "1.21"]),
    ("0.11.0", &["1.21"]),
];

/// The most addresses this store holds.
///
/// Bounds the memory one emulator process can be made to hold through repeated
/// `CreateAddress` calls: an `Address` carries eleven strings, each modelled at most
/// 1,024 characters, so a full store is a few tens of megabytes. `CreateAddress` refuses
/// past it rather than growing without limit. Nothing in this API deletes an address, so
/// the ceiling is the only thing that bounds the store.
pub const MAX_STORED_ADDRESSES: usize = 1_000;

/// The `AddressType` of an address the customer collects a device from, rather than one a
/// device is shipped to.
///
/// `Address$Type` "differentiates between delivery address and pickup address in the
/// customer account" (`smithy_query(query="shape", shape="Address")`) and is one of
/// `shared_types::ADDRESS_TYPE_VALUES`; this is the value that names a pickup address, so
/// it is the one `ListPickupLocations` reports locations for
/// ([`StoredAddress::is_pickup_location`]).
pub const ADDRESS_TYPE_CUSTOMER_PICKUP: &str = "CUST_PICKUP";

/// The most jobs this store holds.
///
/// Bounds the same way for `CreateJob`, which persists every supplied input against the
/// job it mints. `CancelJob` moves a job to `Cancelled` but — as in Snowball itself — does
/// not remove it, so the ceiling is what keeps repeated job creation from growing the
/// process without limit.
pub const MAX_STORED_JOBS: usize = 1_000;

/// The most long-term pricing contracts this store holds.
///
/// Bounds the same way `MAX_STORED_ADDRESSES` and `MAX_STORED_JOBS` do:
/// `CreateLongTermPricing` persists one record per call and nothing in this API deletes
/// one, so the ceiling is what keeps repeated creates from growing the process without
/// limit.
pub const MAX_STORED_LONG_TERM_PRICING: usize = 1_000;

/// The status a long-term pricing contract carries from the moment it is created.
///
/// `LongTermPricingListEntry$LongTermPricingStatus` is modelled as a plain string with no
/// catalog (`smithy_query(query="shape", shape="LongTermPricingListEntry")`), and a
/// contract AWS has taken the upfront payment for and is holding to is reported as
/// `ACTIVE`.
pub const LONG_TERM_PRICING_STATUS_ACTIVE: &str = "ACTIVE";

/// The status a long-term pricing contract carries once the term it was bought for has run
/// out.
///
/// The same modelled member carries it, and the entry's own end date is what decides which
/// of the two a contract is in: a contract whose end date has passed is no longer one the
/// service is holding to.
pub const LONG_TERM_PRICING_STATUS_EXPIRED: &str = "EXPIRED";

/// The most clusters this store holds.
///
/// Bounds the same way [`MAX_STORED_ADDRESSES`] does for addresses: a cluster carries a
/// handful of strings and the structures a job carries, and nothing in this API deletes a
/// cluster, so the ceiling is what keeps repeated `CreateCluster` calls from growing the
/// process without limit.
pub const MAX_STORED_CLUSTERS: usize = 1_000;

/// How many node jobs a cluster holds once it has the quorum it ships with.
///
/// `CreateCluster`: "Each cluster supports five nodes. You use the CreateJob action
/// separately to create the jobs for each of these nodes. The cluster does not ship until
/// these five node jobs have been created", and `ClusterLimitExceededException`:
/// "Currently, clusters support five nodes … create jobs until your cluster has exactly
/// five nodes."
pub const CLUSTER_NODE_QUORUM: usize = 5;

/// The `JobType` values a device can be ordered for, for each `SnowballType` whose
/// documented description shows a job type it does NOT serve.
///
/// Source: the Snow Family device table in `CreateJob`'s own documentation
/// (`smithy_query(query="doc", shape="CreateJob")`), which pairs each device type with a
/// description of what that device is, together with the Snow Family device-type pages
/// that table links (the Snowball Edge Developer Guide and the Snowcone User Guide). A
/// row is written only where the description names a job type the device cannot serve:
///
/// * `EDGE_S` is described as "Snowball Edge Storage Optimized for data transfer only", so
///   the one thing it is ordered for is a transfer into or out of Amazon S3;
/// * `STANDARD` is "Original Snowball device", the data-transfer-only device that predates
///   on-device compute, so it is ordered for a transfer too;
/// * `SNC1_SSD` and `SNC1_HDD` are Snowcone, which the Snowcone User Guide offers as an
///   import or a local compute-and-storage order and not as an export one.
///
/// Every other device type is deliberately absent, which accepts every job type for it:
/// their descriptions ("Compute Optimized", "Snowblade", "Storage Optimized with EC2
/// Compute", "Storage Optimized 210TB") name a capacity rather than a job type the device
/// cannot be ordered for. This emulator declines a combination it can show the device does
/// not serve, never one it merely holds no record for — the same way `create_address.rs`
/// declines only a destination it can show a Region does not ship to.
pub const DEVICE_JOB_TYPES: &[(&str, &[&str])] = &[
    ("STANDARD", &["IMPORT", "EXPORT"]),
    ("EDGE_S", &["IMPORT", "EXPORT"]),
    ("SNC1_SSD", &["IMPORT", "LOCAL_USE"]),
    ("SNC1_HDD", &["IMPORT", "LOCAL_USE"]),
];

/// The `JobType` values whose order moves data between Amazon S3 and the device.
///
/// `CreateJobRequest$Resources`: "With IMPORT jobs, you specify the bucket or buckets
/// that your transferred data will be imported into. With EXPORT jobs, you specify the
/// bucket or buckets that your transferred data will be exported from" — so an order of
/// one of these types is the transfer of named buckets, which `RoleARN` ("The RoleARN
/// that you want to associate with this job") is the identity the service transfers them
/// under. An order for local use moves nothing to or from S3 and names neither.
pub const S3_TRANSFER_JOB_TYPES: &[&str] = &["IMPORT", "EXPORT"];

/// Whether a device of type `snowball_type` can be ordered for a `job_type` job, per
/// [`DEVICE_JOB_TYPES`].
///
/// A device type with no row supports everything, so a device this emulator holds no
/// capability record for is never what refuses an order.
pub fn device_supports_job_type(snowball_type: &str, job_type: &str) -> bool {
    DEVICE_JOB_TYPES
        .iter()
        .find(|(device, _)| *device == snowball_type)
        .is_none_or(|(_, job_types)| job_types.contains(&job_type))
}

/// Whether an order of type `job_type` moves data between Amazon S3 and the device, per
/// [`S3_TRANSFER_JOB_TYPES`].
pub fn transfers_s3_data(job_type: &str) -> bool {
    S3_TRANSFER_JOB_TYPES.contains(&job_type)
}

/// Whether `resources` names at least one Amazon S3 bucket for a transfer to move.
///
/// A bucket is named by the VALUE of an `S3Resource`'s `BucketArn`, never by the presence
/// of the entry that would carry one: `S3Resource` models no required member and
/// `BucketArn` is modelled `length 0..255`
/// (`smithy_query(query="shape", shape="S3Resource")`), so an entry that leaves the member
/// out and an entry that sends it empty each name nothing — read here through
/// [`supplied_value`], the one reading of an optional string member this crate has, and the
/// same reading [`named_resources`] uses when it resolves the buckets an order names. So an
/// absent `Resources`, one carrying no `S3Resources` list, one carrying an empty list and
/// one whose entries all name nothing read the same way here.
pub fn names_s3_bucket(resources: Option<&JobResource>) -> bool {
    resources
        .and_then(|resources| resources.s3_resources.as_deref())
        .is_some_and(|buckets| {
            buckets
                .iter()
                .any(|bucket| supplied_value(bucket.bucket_arn.as_deref()).is_some())
        })
}

/// The lifecycle states a cluster can be in: the whole `ClusterState` enum
/// (`shared_types::CLUSTER_STATE_VALUES`), so the update rule below is expressed over the
/// modelled set rather than over the two states this slice can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterState {
    AwaitingQuorum,
    Pending,
    InUse,
    Complete,
    Cancelled,
}

impl ClusterState {
    /// This state's wire value, exactly as `ClusterState` spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::AwaitingQuorum => "AwaitingQuorum",
            Self::Pending => "Pending",
            Self::InUse => "InUse",
            Self::Complete => "Complete",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Whether `UpdateCluster` is permitted in this state.
    ///
    /// The model documents the window on `UpdateCluster`: "While a cluster's ClusterState
    /// value is in the AwaitingQuorum state, you can update some of the information
    /// associated with a cluster. Once the cluster changes to a different job state …
    /// this action is no longer available." So `AwaitingQuorum` — the state a cluster is
    /// created in, and the one it stays in until its node jobs are all created — is the
    /// only one a cluster's information can still be updated from.
    pub fn is_updatable(self) -> bool {
        matches!(self, Self::AwaitingQuorum)
    }

    /// Whether `CancelCluster` is permitted in this state.
    ///
    /// The model documents the window on `CancelCluster`: "You can only cancel a cluster
    /// job while it's in the `AwaitingQuorum` status." So `AwaitingQuorum` — the state a
    /// cluster is created in, and the one it stays in until its node jobs are all created
    /// — is the only one a cluster can be cancelled from, `Cancelled` (the state a cluster
    /// that was already cancelled is in) included.
    pub fn is_cancellable(self) -> bool {
        matches!(self, Self::AwaitingQuorum)
    }

    /// Whether a cluster in this state is still taking the node jobs it ships with, so
    /// `CreateJob` naming it can add one.
    ///
    /// A cluster is ordered before its nodes exist and then waits for them: "Each cluster
    /// supports five nodes … The cluster does not ship until these five node jobs have
    /// been created" (`CreateCluster`), and the nodes it is still missing are the "more
    /// nodes to create for this cluster" a caller is told to create
    /// (`ClusterLimitExceededException`). The two states the quorum governs — `AwaitingQuorum`
    /// while the cluster is short of it and `Pending` once it is whole — are the states in
    /// which node creation is still what the cluster is waiting on; in the second of them
    /// the five-node ceiling itself is what refuses a further node. A cluster that has left
    /// both has either shipped (`InUse`, `Complete`) or been terminated — `Cancelled`, which
    /// is where `CancelCluster` leaves a cluster that had not shipped ("You can only cancel
    /// a cluster job while it's in the `AwaitingQuorum` status") — so its node set never
    /// grows again.
    pub fn accepts_node_jobs(self) -> bool {
        matches!(self, Self::AwaitingQuorum | Self::Pending)
    }
}

/// How long the update file a `GetSoftwareUpdates` call answers with stays available.
///
/// `GetSoftwareUpdatesResult$UpdatesURI` publishes the window: "The software update will
/// be available for 2 days after this request is made. To access an update after the 2
/// days have passed, you'll have to make another call to GetSoftwareUpdates." So the
/// grant one call issues expires two days after THAT call, and a caller past the window
/// asks for another one.
pub const SOFTWARE_UPDATE_AVAILABILITY: Duration = Duration::from_secs(2 * 24 * 60 * 60);

/// How long the manifest file a `GetJobManifest` call answers with stays available.
///
/// `GetJobManifest` publishes the window: "You can access the manifest file for up to 60
/// minutes after this request has been made. To access the manifest file after 60 minutes
/// have passed, you'll have to make another call to the GetJobManifest action." So the
/// grant one call issues expires an hour after THAT call, and a caller past the window
/// asks for another one.
pub const JOB_MANIFEST_AVAILABILITY: Duration = Duration::from_secs(60 * 60);

/// The most published-file grants this store holds at once.
///
/// Each `GetSoftwareUpdates` and each `GetJobManifest` call issues one grant, so without a
/// ceiling a caller could grow the process by polling. A grant is a job id, a token and
/// two instants — a couple of hundred bytes — so the ceiling bounds the grant store at a
/// few megabytes. Expired grants are dropped first (see `issue_job_file_grant`); at the
/// ceiling the oldest live grant goes, which is the one nearest its own expiry.
pub const MAX_JOB_FILE_GRANTS: usize = 10_000;

/// The Amazon S3 bucket this emulator publishes Snow device update files under.
///
/// `GetSoftwareUpdates` "Returns an Amazon S3 presigned URL for an update file associated
/// with a specified JobId", so the URL it answers with addresses an S3 bucket; this is the
/// bucket this emulator names.
pub const SOFTWARE_UPDATE_BUCKET: &str = "snowball-software-updates";

/// The object name a job's update file is published as, under the job's own key prefix in
/// [`SOFTWARE_UPDATE_BUCKET`].
pub const SOFTWARE_UPDATE_OBJECT: &str = "update.tar";

/// The Amazon S3 bucket this emulator publishes job manifest files under.
///
/// `GetJobManifest` "Returns a link to an Amazon S3 presigned URL for the manifest file
/// associated with the specified JobId value", so the URL it answers with addresses an S3
/// bucket; this is the bucket this emulator names.
pub const JOB_MANIFEST_BUCKET: &str = "snowball-job-manifests";

/// The object name a job's manifest file is published as, under the job's own key prefix
/// in [`JOB_MANIFEST_BUCKET`].
///
/// `GetJobManifest`: "The manifest is an encrypted file that you can download after your
/// job enters the WithCustomer status."
pub const JOB_MANIFEST_OBJECT: &str = "manifest.bin";

/// How long the task that creates a return shipping label runs before the label exists.
///
/// `CreateReturnShippingLabel` answers a `ShippingLabelStatus` — "the status information
/// of the task on a Snow device that is being returned to Amazon Web Services" — rather
/// than a finished label, and `DescribeReturnShippingLabel` is what a caller polls for the
/// label's URL, so the label is not ready at the instant the creating call is served: it
/// becomes ready this long afterwards. The span is this emulator's own CHOICE — no
/// documented duration governs it — and it is never zero, so a caller asking straight
/// after the create still observes the task in progress, which is also the window in which
/// a second create is the "more than once when other requests are not completed" case
/// `ConflictException` describes.
pub const RETURN_SHIPPING_LABEL_PREPARATION: Duration = Duration::from_millis(500);

/// How long a return shipping label stays usable once the task that creates it has
/// finished.
///
/// `DescribeReturnShippingLabelResult$ExpirationDate` is documented only as "The
/// expiration date of the current return shipping label", and neither the model nor the
/// Snow Family documentation publishes the length of that window, so this one is the
/// emulator's own CHOICE. What the API fixes is the behaviour around it: a label carries
/// an expiry, `DescribeReturnShippingLabel` reports it, a second create while a valid
/// label exists is refused, and a create once the label has expired mints a new one.
pub const RETURN_SHIPPING_LABEL_VALIDITY: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The Amazon S3 bucket this emulator publishes return shipping labels under.
///
/// `DescribeReturnShippingLabelResult$ReturnShippingLabelURI` is "The pre-signed Amazon S3
/// URI used to download the return shipping label", so the URI a caller receives addresses
/// an S3 bucket; this is the bucket this emulator names.
pub const RETURN_SHIPPING_LABEL_BUCKET: &str = "snowball-return-shipping-labels";

/// The object a job's return shipping label is published as, under the job's own key
/// prefix in [`RETURN_SHIPPING_LABEL_BUCKET`].
pub const RETURN_SHIPPING_LABEL_OBJECT: &str = "return-shipping-label.pdf";

/// How long a job's device credentials — its manifest and its unlock code — can be read
/// for after the job was created.
///
/// `GetJobUnlockCode` publishes the window: "A particular UnlockCode value can be accessed
/// for up to 360 days after the associated job has been created", and `GetJobManifest`
/// states the same of the pair: "The credentials of a given job, including its manifest
/// file and unlock code, expire 360 days after the job is created." So the window is the
/// job's own creation instant plus 360 days, it is the SAME window for both credentials,
/// and it is not something a read moves.
pub const DEVICE_CREDENTIAL_VALIDITY: Duration = Duration::from_secs(360 * 24 * 60 * 60);

/// How many hyphen-separated groups a job's unlock code carries, and how many alphanumeric
/// characters are in each.
///
/// `GetJobUnlockCode` documents the form: "The UnlockCode value is a 29-character code with
/// 25 alphanumeric characters and 4 hyphens" — 25 characters split evenly by the 4 hyphens
/// is five groups of five, which is what [`UNLOCK_CODE_LEN`] counts up to.
pub const UNLOCK_CODE_GROUPS: usize = 5;
pub const UNLOCK_CODE_GROUP_LEN: usize = 5;

/// How long a job's unlock code is: its groups and the hyphens between them, i.e. the 29
/// characters `GetJobUnlockCode` documents.
pub const UNLOCK_CODE_LEN: usize = UNLOCK_CODE_GROUPS * UNLOCK_CODE_GROUP_LEN + UNLOCK_CODE_GROUPS - 1;

/// The account's Snow device service limit, as `GetSnowballUsage` reports it.
///
/// `GetSnowballUsageResult$SnowballLimit` is "the service limit for number of Snow devices
/// this account can have at once. The default service limit is 1 (one)", and the operation
/// documents the only way it moves: "If you want to increase your service limit, contact
/// Amazon Web Services Support." No operation of this API raises or lowers it, so it is
/// not something a caller's request can change and the store holds nothing to move: this
/// emulator reports the documented default every account starts at.
pub const SNOWBALL_SERVICE_LIMIT: i32 = 1;

/// The lifecycle states a job can be in: the whole `JobState` enum
/// (`shared_types::JOB_STATE_VALUES`), so the cancellation rule below is expressed
/// over the modelled set rather than over the two states this slice can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    New,
    PreparingAppliance,
    PreparingShipment,
    InTransitToCustomer,
    WithCustomer,
    InTransitToAWS,
    WithAWSSortingFacility,
    WithAWS,
    InProgress,
    Complete,
    Cancelled,
    Listing,
    Pending,
}

impl JobState {
    /// This state's wire value, exactly as `JobState` spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::PreparingAppliance => "PreparingAppliance",
            Self::PreparingShipment => "PreparingShipment",
            Self::InTransitToCustomer => "InTransitToCustomer",
            Self::WithCustomer => "WithCustomer",
            Self::InTransitToAWS => "InTransitToAWS",
            Self::WithAWSSortingFacility => "WithAWSSortingFacility",
            Self::WithAWS => "WithAWS",
            Self::InProgress => "InProgress",
            Self::Complete => "Complete",
            Self::Cancelled => "Cancelled",
            Self::Listing => "Listing",
            Self::Pending => "Pending",
        }
    }

    /// Whether `CancelJob` is permitted in this state.
    ///
    /// The model documents the window on `CancelJob`: "You can only cancel a job before
    /// its `JobState` value changes to `PreparingAppliance`" — so `New`, the state a job
    /// is created in, is the only one from which a job can still be cancelled.
    pub fn is_cancellable(self) -> bool {
        matches!(self, Self::New)
    }

    /// Whether `UpdateJob` is permitted in this state.
    ///
    /// The model documents the window on `UpdateJob`: "While a job's `JobState` value is
    /// `New`, you can update some of the information associated with a job. Once the job
    /// changes to a different job state … this action is no longer available." So `New`
    /// is the only state a job's information can still be updated from — including
    /// `Cancelled`, which a cancelled job is in and from which no update is available.
    pub fn is_updatable(self) -> bool {
        matches!(self, Self::New)
    }

    /// Whether a device has actually been prepared and shipped for a job in this state, so
    /// the job has a Snow device of its own — and with it that device's files, its software
    /// update file among them.
    ///
    /// `JobMetadata$SnowballId` is the "unique ID associated with a device", which a job
    /// names only once a device exists for it, and the `JobState` values split on exactly
    /// that. While the order is still being placed and made ready no device has been built
    /// and sent for it: `New`, the state `CreateJob` mints, `PreparingAppliance`, in which
    /// an appliance is still being prepared, and `PreparingShipment`, in which its shipment
    /// is still being made up. `Cancelled` stops the order before any appliance is prepared
    /// at all ("You can only cancel a job before its `JobState` value changes to
    /// `PreparingAppliance`", `CancelJob`), so no device is ever built for it. `Listing` and
    /// `Pending` are the states an export order's main job passes through while its objects
    /// are listed and its job parts are still to come, and a main job is associated with no
    /// device at all ("Main jobs aren't associated with any Snowballs", `JobListEntry$IsMaster`).
    ///
    /// Every remaining `JobState` value places a built device somewhere — on its way to the
    /// caller, in the caller's hands, on its way back, at the sorting facility, or back at
    /// Amazon Web Services with the transfer running or finished — so from those a device
    /// exists for the job. [`Self::has_returnable_device`] and
    /// [`Self::releases_device_credentials`] are narrower windows inside this one, each
    /// fixed by what its own operation documents.
    pub fn has_prepared_device(self) -> bool {
        matches!(
            self,
            Self::InTransitToCustomer
                | Self::WithCustomer
                | Self::InTransitToAWS
                | Self::WithAWSSortingFacility
                | Self::WithAWS
                | Self::InProgress
                | Self::Complete
        )
    }

    /// Whether a job in this state has a device that can still be returned to AWS.
    ///
    /// `CreateReturnShippingLabel` "creates a shipping label that will be used to return
    /// the Snow device to Amazon Web Services", so the label presupposes a device that has
    /// left Amazon Web Services and has not been received back: `ShippingDetails` splits a
    /// job's journey into the `OutboundShipment` of "a Snow device being delivered to the
    /// address that you specified for a particular job" and the `InboundShipment` of "a Snow
    /// device being returned to Amazon Web Services for a particular job", and the
    /// `JobState` values that say where the device is put it on the customer's side of that
    /// journey in exactly three of them: `InTransitToCustomer`, `WithCustomer` and
    /// `InTransitToAWS`.
    ///
    /// Every other state leaves nothing to return. Before those three the order is still
    /// being prepared and no device has been handed over: `New`, the state a job is created
    /// in, `PreparingAppliance`, `PreparingShipment`, and `Cancelled`, which stopped before
    /// any appliance was prepared for it ("You can only cancel a job before its `JobState`
    /// value changes to `PreparingAppliance`", `CancelJob`), so no device of its own will
    /// ever ship; `Listing` and `Pending` likewise say nothing about a device in the
    /// customer's hands. After those three — `WithAWSSortingFacility`, `WithAWS`,
    /// `InProgress`, `Complete` — the device is back at Amazon Web Services already.
    pub fn has_returnable_device(self) -> bool {
        matches!(
            self,
            Self::InTransitToCustomer | Self::WithCustomer | Self::InTransitToAWS
        )
    }

    /// Whether a job in this state releases the credentials that unlock its device — its
    /// manifest and its unlock code.
    ///
    /// `GetJobUnlockCode` documents the window: "The only valid status for calling this API
    /// is WithCustomer as the manifest and Unlock code values are used for securing your
    /// device and should only be used when you have the device", and `GetJobManifest` says
    /// the same of the manifest half of the pair. `WithCustomer` is the one `JobState` in
    /// which the device is in the caller's hands, so it is the one state these credentials
    /// are released from.
    pub fn releases_device_credentials(self) -> bool {
        matches!(self, Self::WithCustomer)
    }

    /// Whether a node job in this state is still one of the nodes its cluster ships with.
    ///
    /// A cluster's quorum is the set of node jobs it will ship as: "Each cluster supports
    /// five nodes. You use the CreateJob action separately to create the jobs for each of
    /// these nodes. The cluster does not ship until these five node jobs have been created"
    /// (`CreateCluster`), and `ClusterLimitExceededException` states the same rule from the
    /// join side: "If you have fewer than five nodes for your cluster and you have more
    /// nodes to create for this cluster, try again and create jobs until your cluster has
    /// exactly five nodes." `Cancelled` is the one `JobState` in which the order was
    /// stopped before any device was ever prepared for it ("You can only cancel a job
    /// before its `JobState` value changes to `PreparingAppliance`", `CancelJob`), so a
    /// cancelled node is a node the cluster will never ship with and the cluster is one
    /// node short again. From every other state the node is still part of the cluster's
    /// order — being prepared, shipped, with the customer, back at Amazon Web Services, or
    /// finished.
    pub fn is_live_cluster_node(self) -> bool {
        !matches!(self, Self::Cancelled)
    }

    /// Whether a job in this state has a Snow device allocated to the account.
    ///
    /// `GetSnowballUsage` reports "the number of Snow devices that this account is
    /// currently using", so a job counts while its order still holds a device. Two
    /// `JobState` values leave an order holding none: `Cancelled`, from which no appliance
    /// is ever prepared ("You can only cancel a job before its `JobState` value changes to
    /// `PreparingAppliance`", `CancelJob`), and `Complete`, the state an order has finished
    /// in, with its device back at Amazon Web Services and free for another order. From
    /// every other state the order is live and a device is allocated to it — being
    /// prepared, in transit either way, with the customer, or back at Amazon Web Services
    /// with the job not yet finished.
    pub fn holds_device(self) -> bool {
        !matches!(self, Self::Cancelled | Self::Complete)
    }

    /// This state, read back from the wire value [`Self::as_wire`] spells it as, or `None`
    /// when `value` is not a `JobState` this service models.
    pub fn from_wire(value: &str) -> Option<Self> {
        [
            Self::New,
            Self::PreparingAppliance,
            Self::PreparingShipment,
            Self::InTransitToCustomer,
            Self::WithCustomer,
            Self::InTransitToAWS,
            Self::WithAWSSortingFacility,
            Self::WithAWS,
            Self::InProgress,
            Self::Complete,
            Self::Cancelled,
            Self::Listing,
            Self::Pending,
        ]
        .into_iter()
        .find(|state| state.as_wire() == value)
    }
}

/// A leg of a device's journey, as the caller reports it: the whole `ShipmentState` enum
/// (`shared_types::SHIPMENT_STATE_VALUES`).
///
/// Each value names a leg that has just finished — "Set to `RECEIVED` when the device
/// arrives at your location. Set to `RETURNED` when you have returned the device to Amazon
/// Web Services" (`UpdateJobShipmentStateRequest$ShipmentState`) — so a report is only
/// meaningful while the device is on that leg, and once made it leaves the device where the
/// leg ended. [`Self::reported_from`] and [`Self::reported_job_state`] are those two ends,
/// both expressed as the `JobState` values that say where a device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShipmentState {
    Received,
    Returned,
}

impl ShipmentState {
    /// This shipment state's wire value, exactly as `ShipmentState` spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Received => "RECEIVED",
            Self::Returned => "RETURNED",
        }
    }

    /// This shipment state, read back from the wire value [`Self::as_wire`] spells it as,
    /// or `None` when `value` is not a `ShipmentState` this service models.
    pub fn from_wire(value: &str) -> Option<Self> {
        [Self::Received, Self::Returned]
            .into_iter()
            .find(|state| state.as_wire() == value)
    }

    /// The `JobState` a job is in while the leg this report names is still running, i.e.
    /// the only state the report can be made from.
    ///
    /// `RECEIVED` says the device "arrives at your location", which is the end of the leg
    /// `InTransitToCustomer` names; `RETURNED` says the device has been returned to Amazon
    /// Web Services, which is the end of the stay `WithCustomer` names. From any other
    /// state the device is not on the reported leg at all, so there is no shipment state to
    /// change.
    pub fn reported_from(self) -> JobState {
        match self {
            Self::Received => JobState::InTransitToCustomer,
            Self::Returned => JobState::WithCustomer,
        }
    }

    /// The `JobState` the finished leg leaves the job in: the device is in the caller's
    /// hands once it has arrived there, and on its way back to Amazon Web Services once the
    /// caller has returned it.
    pub fn reported_job_state(self) -> JobState {
        match self {
            Self::Received => JobState::WithCustomer,
            Self::Returned => JobState::InTransitToAWS,
        }
    }
}

/// One stored shipping address: every `Address` member this service keeps, so
/// `DescribeAddress` rebuilds the submitted address instead of echoing a request.
///
/// `PrefectureOrDistrict` and `Landmark` are deliberately absent: the model documents
/// both as "This field is no longer used and the value is ignored", so a submitted
/// value is not stored and not reported back.
#[derive(Debug, Clone, Default)]
pub struct StoredAddress {
    pub address_id: String,
    pub name: Option<String>,
    pub company: Option<String>,
    pub street1: Option<String>,
    pub street2: Option<String>,
    pub street3: Option<String>,
    pub city: Option<String>,
    pub state_or_province: Option<String>,
    pub country: Option<String>,
    pub postal_code: Option<String>,
    pub phone_number: Option<String>,
    pub is_restricted: bool,
    pub address_type: Option<String>,
}

impl StoredAddress {
    /// Whether this address is a location a customer picks a device up from, i.e. carries
    /// [`ADDRESS_TYPE_CUSTOMER_PICKUP`] as its `AddressType`.
    ///
    /// Asked of the stored type rather than of anything else the record carries: the type
    /// is the member that "differentiates between delivery address and pickup address in
    /// the customer account", so an address typed any other way — or one carrying no type
    /// at all — is a delivery address and not a pickup location.
    pub fn is_pickup_location(&self) -> bool {
        self.address_type.as_deref() == Some(ADDRESS_TYPE_CUSTOMER_PICKUP)
    }

    /// This record as the modelled `Address`, including the id the service minted.
    pub fn to_modeled(&self) -> Address {
        Address {
            address_id: Some(self.address_id.clone()),
            name: self.name.clone(),
            company: self.company.clone(),
            street1: self.street1.clone(),
            street2: self.street2.clone(),
            street3: self.street3.clone(),
            city: self.city.clone(),
            state_or_province: self.state_or_province.clone(),
            prefecture_or_district: None,
            landmark: None,
            country: self.country.clone(),
            postal_code: self.postal_code.clone(),
            phone_number: self.phone_number.clone(),
            is_restricted: self.is_restricted,
            r#type: self.address_type.clone(),
        }
    }
}

/// One stored job: the state the service minted for it plus every `CreateJob` input it
/// was given, so `DescribeJob` and `ListJobs` report what was persisted.
///
/// A record is either a plain job, the *main* job of an export order, or one of that
/// order's *job parts* — the three things this API's job ids name. [`Self::job_part_ids`]
/// is what tells them apart.
#[derive(Debug, Clone)]
pub struct StoredJob {
    pub job_id: String,
    pub job_state: JobState,
    pub job_type: Option<String>,
    pub snowball_type: Option<String>,
    pub snowball_capacity_preference: Option<String>,
    pub shipping_option: Option<String>,
    pub creation_date: SystemTime,
    pub description: Option<String>,
    pub address_id: Option<String>,
    pub forwarding_address_id: Option<String>,
    pub role_arn: Option<String>,
    pub kms_key_arn: Option<String>,
    pub resources: Option<JobResource>,
    pub notification: Option<Notification>,
    pub on_device_service_configuration: Option<OnDeviceServiceConfiguration>,
    /// The cluster this job is a node of, and `None` for a job that is not a cluster
    /// node: a node job is created either by `CreateCluster`'s `InitialClusterSize` or by
    /// a `CreateJob` that names the cluster.
    pub cluster_id: Option<String>,
    pub long_term_pricing_id: Option<String>,
    pub tax_documents: Option<TaxDocuments>,
    pub device_configuration: Option<DeviceConfiguration>,
    pub remote_management: Option<String>,
    pub impact_level: Option<String>,
    pub pickup_details: Option<PickupDetails>,
    /// This job's job parts, in the order they were created: the device-bearing jobs an
    /// export order was split into. Empty for a job part and for every job that is not an
    /// export order's main job.
    pub job_part_ids: Vec<String>,
    /// The unlock code this service minted for the job when the job was created: the
    /// credential `GetJobUnlockCode` reports, which "is used to decrypt the manifest file
    /// when it is passed along with the manifest to the Snow device". It is the job's own
    /// and is minted once, so every read of it answers the same code.
    pub unlock_code: String,
    /// The Region this order was placed in: the SigV4 credential scope of the create that
    /// made the record ([`request_region`]), which is the only Region a request carries.
    ///
    /// Nothing in THIS service's API reads it — "Calling this API action in one of the US
    /// regions will return jobs from the list of all jobs associated with this account in
    /// all US regions" (`ListJobs`), so a job is reported wherever it is asked for — but
    /// the device this order ships is a REGIONAL resource of the service that manages it,
    /// and the Region it is managed in is the Region the order was placed in. An empty
    /// string when the create carried no scope to read a Region from, which is a request
    /// this emulator can place in no Region.
    pub order_region: String,
}

impl StoredJob {
    /// Whether this record is a main job.
    ///
    /// `JobListEntry$IsMaster`: "A main job represents a successful request to create an
    /// export job. Main jobs aren't associated with any Snowballs. Instead, each main job
    /// will have at least one job part, and each job part is associated with a Snowball."
    /// Holding at least one job part is therefore exactly what makes a job a main job,
    /// which is why this is derived from [`Self::job_part_ids`] rather than stored beside
    /// it: a main job with no parts, or a part-bearing job that reports itself as a plain
    /// job, cannot be represented.
    pub fn is_master(&self) -> bool {
        !self.job_part_ids.is_empty()
    }

    /// When this job's device credentials stop being readable:
    /// [`DEVICE_CREDENTIAL_VALIDITY`] after the job was created.
    pub fn device_credentials_expire_at(&self) -> SystemTime {
        self.creation_date + DEVICE_CREDENTIAL_VALIDITY
    }

    /// Whether this job's device credentials can still be read at `now`, i.e. the job is
    /// still inside the window [`Self::device_credentials_expire_at`] closes.
    pub fn device_credentials_are_readable(&self, now: SystemTime) -> bool {
        now < self.device_credentials_expire_at()
    }

    /// This record as the modelled `JobMetadata` `DescribeJob` reports.
    ///
    /// The members that describe work no device has done yet are absent rather than
    /// zeroed: `DataTransferProgress` is documented as available only while a job is
    /// `InProgress`, `JobLogInfo` only once a transfer has produced a report, and
    /// `SnowballId` only once a device is associated with the job — which for a main job
    /// is never ("Main jobs aren't associated with any Snowballs") and for every other
    /// job, job parts included, is once a device has been prepared and shipped for it
    /// ([`JobState::has_prepared_device`]), the same window the job's device files are
    /// published from. Nothing in this API assigns a device id, so the store holds none to
    /// report and the member is absent.
    pub fn to_job_metadata(&self) -> JobMetadata {
        JobMetadata {
            job_id: Some(self.job_id.clone()),
            job_state: Some(self.job_state.as_wire().to_string()),
            job_type: self.job_type.clone(),
            snowball_type: self.snowball_type.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            resources: self.resources.clone(),
            description: self.description.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            role_arn: self.role_arn.clone(),
            address_id: self.address_id.clone(),
            // The shipping speed the job was created with. Both shipments are absent
            // until a device actually moves: a job in `New` has no tracking number.
            shipping_details: self.shipping_option.clone().map(|option| ShippingDetails {
                shipping_option: Some(option),
                inbound_shipment: None,
                outbound_shipment: None,
            }),
            snowball_capacity_preference: self.snowball_capacity_preference.clone(),
            notification: self.notification.clone(),
            data_transfer_progress: None,
            job_log_info: None,
            cluster_id: self.cluster_id.clone(),
            forwarding_address_id: self.forwarding_address_id.clone(),
            tax_documents: self.tax_documents.clone(),
            device_configuration: self.device_configuration.clone(),
            remote_management: self.remote_management.clone(),
            long_term_pricing_id: self.long_term_pricing_id.clone(),
            on_device_service_configuration: self.on_device_service_configuration.clone(),
            impact_level: self.impact_level.clone(),
            pickup_details: self.pickup_details.clone(),
            snowball_id: None,
        }
    }

    /// This record as the modelled `JobListEntry` `ListJobs` pages over.
    pub fn to_job_list_entry(&self) -> JobListEntry {
        JobListEntry {
            job_id: Some(self.job_id.clone()),
            job_state: Some(self.job_state.as_wire().to_string()),
            // The main job of an export order is reported as the main job; its job parts
            // and every plain job are not (`JobListEntry$IsMaster`).
            is_master: self.is_master(),
            job_type: self.job_type.clone(),
            snowball_type: self.snowball_type.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            description: self.description.clone(),
        }
    }
}

/// One stored long-term pricing contract: the id the service minted for it, the term and
/// device type it was bought for, and the window it runs over.
///
/// `CurrentActiveJob` and `JobIds` are deliberately not held here: each names a job
/// associated with the contract, and a job carries the `LongTermPricingId` it was ordered
/// under ([`StoredJob::long_term_pricing_id`]), so the association is read off the jobs the
/// store holds
/// ([`ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState::jobs_billed_to_long_term_pricing`])
/// and the two can never disagree. `ReplacementJob` is held, since nothing but
/// `UpdateLongTermPricing` names it: it is the job a caller declared replaces the device
/// ordered under the contract, not an association any other record carries.
#[derive(Debug, Clone)]
pub struct StoredLongTermPricing {
    pub long_term_pricing_id: String,
    /// The term the contract was bought for, spelled as `LongTermPricingType` spells it.
    pub long_term_pricing_type: String,
    /// The device type the contract covers, spelled as `SnowballType` spells it.
    pub snowball_type: String,
    /// Whether the contract renews itself when it reaches its end date. Absent on the
    /// create means "do not renew", so every stored contract answers the question.
    pub is_long_term_pricing_auto_renew: bool,
    /// The instant the contract was created, which is the instant its term starts.
    pub long_term_pricing_start_date: SystemTime,
    /// The instant the term bought ends: the start plus the whole calendar months
    /// `LongTermPricingType` names. A contract that renews itself has it moved on by another
    /// whole term each time the clock reaches it
    /// ([`StoredLongTermPricing::settle_term_at`]), so it is always the end of the term the
    /// contract is currently running.
    pub long_term_pricing_end_date: SystemTime,
    pub long_term_pricing_status: String,
    /// The job that replaces the device ordered under this contract, once
    /// `UpdateLongTermPricing` has named one. `None` on a contract no update has named one
    /// for, which is what a create stores.
    pub replacement_job: Option<String>,
}

impl StoredLongTermPricing {
    /// The term this contract was bought for, in whole calendar months: the term its
    /// `LongTermPricingType` names, read from the one table that pairs the two
    /// ([`crate::ops::create_long_term_pricing::LONG_TERM_PRICING_TERM_MONTHS`]), so the
    /// length a renewal runs for cannot drift from the length the create measured the first
    /// term with. `None` for a stored type that table does not carry, which no accepted
    /// create can produce.
    pub fn term_months(&self) -> Option<u32> {
        crate::ops::create_long_term_pricing::LONG_TERM_PRICING_TERM_MONTHS
            .iter()
            .find(|(pricing_type, _)| *pricing_type == self.long_term_pricing_type)
            .map(|(_, months)| *months)
    }

    /// The instant the term this contract is currently running ends, at `now`.
    ///
    /// A contract bought with automatic renewal is renewed for another whole term each time
    /// it reaches that instant, so the term it is running at `now` is the first one whose end
    /// is still ahead: each renewal runs the term's calendar months on from the end of the
    /// one before, and a contract nobody looked at for several terms is that many terms on.
    /// A contract bought without automatic renewal, and one the service has already stopped
    /// holding to ([`LONG_TERM_PRICING_STATUS_EXPIRED`]), keeps the end date it holds: it has
    /// no further term to run.
    ///
    /// The walk over the boundaries always terminates, and in a step count nothing a caller
    /// sends can grow: every step moves the end on by a whole calendar month at least, and
    /// stops at the first instant after `now`, so it takes one step per term that has run out
    /// since the contract was bought — and a bought contract's term ends in the future, with
    /// each boundary the clock passes written back by [`Self::settle_term_at`].
    pub fn end_date_at(&self, now: SystemTime) -> SystemTime {
        let mut end = self.long_term_pricing_end_date;
        if !self.is_long_term_pricing_auto_renew
            || self.long_term_pricing_status == LONG_TERM_PRICING_STATUS_EXPIRED
        {
            return end;
        }
        let Some(months) = self.term_months() else {
            return end;
        };
        // Renewal happens "before the long-term pricing contract expires", so a term is
        // renewed at the instant it would otherwise run out, not after it has.
        while end <= now {
            match crate::ops::create_long_term_pricing::plus_calendar_months(end, months) {
                Some(renewed) if renewed > end => end = renewed,
                // A term naming no later instant — a zero-length one, or one ending outside
                // the range a timestamp can represent — has no boundary to renew to.
                _ => return end,
            }
        }
        end
    }

    /// This contract's `LongTermPricingStatus` at `now`: the status it was stored with while
    /// the term it is running ([`Self::end_date_at`]) still has time to run, and
    /// [`LONG_TERM_PRICING_STATUS_EXPIRED`] from the end of its last term on — a change the
    /// clock owes rather than one a call makes, so it is derived rather than asked for.
    ///
    /// A contract that renews itself therefore never reaches that point: the clock's due
    /// change at its end date is a renewal, not an expiry.
    pub fn status_at(&self, now: SystemTime) -> &str {
        if now < self.end_date_at(now) {
            &self.long_term_pricing_status
        } else {
            LONG_TERM_PRICING_STATUS_EXPIRED
        }
    }

    /// Write onto this record the change its term boundary has already made due at `now`:
    /// the renewal of a contract bought with automatic renewal, and the expiry of one bought
    /// without.
    ///
    /// Both were owed at that boundary whether or not anybody called, so materialising them
    /// reports exactly what [`Self::end_date_at`] and [`Self::status_at`] derive — one rule,
    /// one implementation. Persisting them is what makes the decision at the boundary rather
    /// than at each later read: a contract renewed for another term keeps that term even if
    /// automatic renewal is switched off inside it, and one whose last term has run out is
    /// not revived by switching automatic renewal on afterwards.
    pub fn settle_term_at(&mut self, now: SystemTime) {
        self.long_term_pricing_end_date = self.end_date_at(now);
        self.long_term_pricing_status = self.status_at(now).to_string();
    }

    /// This record as the modelled `LongTermPricingListEntry` `ListLongTermPricing` pages
    /// over, for a store whose clock reads `now` and in which `billed` are the jobs ordered
    /// under this contract, in creation order.
    ///
    /// `JobIds` is "The IDs of the jobs that are associated with a long-term pricing type",
    /// so it is every one of those jobs; a contract no job has been ordered under yet
    /// reports none at all rather than an empty list. `CurrentActiveJob` is "The current
    /// active jobs on the device the long-term pricing type", so it is the newest of them
    /// whose order still has a device of its own ([`StoredJob::is_master`],
    /// [`JobState::holds_device`]) — the device the contract is paying for right now — and
    /// `None` once none of them has one. `ReplacementJob` is "a new device that replaces a
    /// device that is ordered with long-term pricing", so it is the job
    /// `UpdateLongTermPricing` named as that replacement, and absent until one does.
    pub fn to_list_entry(&self, now: SystemTime, billed: &[&StoredJob]) -> LongTermPricingListEntry {
        let job_ids: Vec<String> = billed.iter().map(|job| job.job_id.clone()).collect();
        let current_active_job = billed
            .iter()
            .rfind(|job| !job.is_master() && job.job_state.holds_device())
            .map(|job| job.job_id.clone());
        LongTermPricingListEntry {
            long_term_pricing_id: Some(self.long_term_pricing_id.clone()),
            long_term_pricing_end_date: Some(::aws_smithy_types::DateTime::from(
                self.end_date_at(now),
            )),
            long_term_pricing_start_date: Some(::aws_smithy_types::DateTime::from(
                self.long_term_pricing_start_date,
            )),
            long_term_pricing_type: Some(self.long_term_pricing_type.clone()),
            current_active_job,
            replacement_job: self.replacement_job.clone(),
            is_long_term_pricing_auto_renew: Some(self.is_long_term_pricing_auto_renew),
            long_term_pricing_status: Some(self.status_at(now).to_string()),
            snowball_type: Some(self.snowball_type.clone()),
            job_ids: (!job_ids.is_empty()).then_some(job_ids),
        }
    }
}

/// One stored cluster: the id and state the service minted for it plus every
/// `CreateCluster` input a cluster keeps, so `DescribeCluster` reports what was persisted
/// and a node job created for the cluster inherits it.
///
/// The members `CreateClusterRequest` makes required are held as plain values, since a
/// stored cluster always has them; the rest are held exactly as supplied.
/// `SnowballCapacityPreference`, `RemoteManagement` and `LongTermPricingIds` are kept even
/// though `ClusterMetadata` models none of them, because a node job does model them and
/// inherits them from the cluster.
#[derive(Debug, Clone)]
pub struct StoredCluster {
    pub cluster_id: String,
    pub cluster_state: ClusterState,
    pub job_type: String,
    pub snowball_type: String,
    pub shipping_option: String,
    pub address_id: String,
    pub creation_date: SystemTime,
    pub snowball_capacity_preference: Option<String>,
    pub description: Option<String>,
    pub forwarding_address_id: Option<String>,
    pub role_arn: Option<String>,
    pub kms_key_arn: Option<String>,
    pub resources: Option<JobResource>,
    pub notification: Option<Notification>,
    pub on_device_service_configuration: Option<OnDeviceServiceConfiguration>,
    pub tax_documents: Option<TaxDocuments>,
    pub remote_management: Option<String>,
    /// The long-term pricing ids the cluster's automatically created jobs are associated
    /// with, in the order they were supplied (`CreateClusterRequest$LongTermPricingIds`).
    pub long_term_pricing_ids: Vec<String>,
    /// The Region this cluster order was placed in, which every node job of it inherits
    /// ([`StoredJob::order_region`]).
    pub order_region: String,
    /// This cluster's node jobs, in the order they were created.
    ///
    /// Every node ever registered against the cluster is here, whatever state it reached:
    /// nothing in this API removes a job, so a cancelled node is a node whose `JobState`
    /// says so. Which of them are the nodes the cluster SHIPS with is therefore read off
    /// the jobs themselves
    /// ([`ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState::live_cluster_nodes`])
    /// rather than from the length of this list.
    pub node_job_ids: Vec<String>,
}

impl StoredCluster {
    /// This record as the modelled `ClusterMetadata` `DescribeCluster` reports.
    pub fn to_cluster_metadata(&self) -> ClusterMetadata {
        ClusterMetadata {
            cluster_id: Some(self.cluster_id.clone()),
            description: self.description.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            role_arn: self.role_arn.clone(),
            cluster_state: Some(self.cluster_state.as_wire().to_string()),
            job_type: Some(self.job_type.clone()),
            snowball_type: Some(self.snowball_type.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            resources: self.resources.clone(),
            address_id: Some(self.address_id.clone()),
            shipping_option: Some(self.shipping_option.clone()),
            notification: self.notification.clone(),
            forwarding_address_id: self.forwarding_address_id.clone(),
            tax_documents: self.tax_documents.clone(),
            on_device_service_configuration: self.on_device_service_configuration.clone(),
        }
    }

    /// This record as the modelled `ClusterListEntry` `ListClusters` pages over.
    ///
    /// The entry reports the same members off the same record `DescribeCluster` reports
    /// from, so a listing and a describe can never disagree about a cluster.
    pub fn to_cluster_list_entry(&self) -> ClusterListEntry {
        ClusterListEntry {
            cluster_id: Some(self.cluster_id.clone()),
            cluster_state: Some(self.cluster_state.as_wire().to_string()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            description: self.description.clone(),
        }
    }

    /// A node job of this cluster under the id `job_id`, created at `created`, carrying
    /// the unlock code `unlock_code` the store minted for it.
    ///
    /// "If you're creating a job for a node in a cluster, you only need to provide the
    /// clusterId value; the other job attributes are inherited from the cluster"
    /// (`CreateJobRequest$ClusterId`), so every member the cluster carries is what this
    /// job inherits. This is the INHERITED record: a member the request supplied is the
    /// job's own and is applied over it by the caller of this helper. The members only a
    /// job models — its device configuration, its impact level, its pickup details, the
    /// long-term pricing it is associated with — are left for that caller too, since the
    /// cluster holds nothing to inherit them from.
    pub fn node_job(&self, job_id: String, created: SystemTime, unlock_code: String) -> StoredJob {
        StoredJob {
            job_id,
            // A node job AWS has accepted but prepared no device for is `New`, exactly as
            // a job created on its own is.
            job_state: JobState::New,
            job_type: Some(self.job_type.clone()),
            snowball_type: Some(self.snowball_type.clone()),
            snowball_capacity_preference: self.snowball_capacity_preference.clone(),
            shipping_option: Some(self.shipping_option.clone()),
            creation_date: created,
            description: self.description.clone(),
            address_id: Some(self.address_id.clone()),
            forwarding_address_id: self.forwarding_address_id.clone(),
            role_arn: self.role_arn.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            resources: self.resources.clone(),
            notification: self.notification.clone(),
            on_device_service_configuration: self.on_device_service_configuration.clone(),
            cluster_id: Some(self.cluster_id.clone()),
            long_term_pricing_id: None,
            tax_documents: self.tax_documents.clone(),
            device_configuration: None,
            remote_management: self.remote_management.clone(),
            impact_level: None,
            pickup_details: None,
            // A cluster's node is a job of its own, never an export order's main job, so
            // it holds no job parts.
            job_part_ids: Vec::new(),
            unlock_code,
            // A node ships with the cluster, so its order was placed where the cluster's
            // was.
            order_region: self.order_region.clone(),
        }
    }

    /// The first attribute of the order this cluster ships that `input` asks a node of it
    /// to hold a different value for, named as `CreateJobRequest` names the member that
    /// carried it — or `None` when every attribute the request names is the one the cluster
    /// already holds.
    ///
    /// `ClusterMetadata` reports ONE value per attribute for the whole cluster — one job
    /// type, one device type, one address, one forwarding address, one description, one
    /// shipping speed, one role, one key, one set of resources, one notification, one set of
    /// tax documents, one on-device service configuration — and a node job of the cluster
    /// carries those attributes rather than copies of its own: "you need to provide only
    /// this clusterId value. The other job attributes are inherited from the cluster"
    /// (`CreateJobRequest$ClusterId`). So a create that names one of them is repeating what
    /// the cluster holds, which changes nothing, or it is asking for a node whose devices
    /// ship elsewhere, at another speed, under another role than the rest of the order,
    /// which is a set of inputs that cannot hold together. Asking this question in one place
    /// is what keeps the create from storing a divergence the two updates
    /// ([`ClusterOrderMembers`]) then silently write away.
    ///
    /// An attribute the cluster holds NO value for is no different: the order has none, so
    /// a node of it has none either, and a request that names one is asking for the same
    /// divergence. The structure members are compared by value, and the order of a list
    /// member is part of that value, since both records report such a list exactly as it was
    /// supplied.
    ///
    /// The eight attributes an update can still change are asked about through
    /// [`ClusterOrderMembers`], which is the ONE place that set is written down, so this
    /// check and the two updates cannot drift apart over what a cluster's order carries.
    /// Only the four `ClusterMetadata` members no update models are named here: the job type
    /// and the device type the order was placed in, the key its manifests are encrypted
    /// under and the tax documents it carries.
    ///
    /// The members `ClusterMetadata` does not model — the capacity a device is configured
    /// with, the remote-management setting, and the members only a job has (its long-term
    /// pricing, its device configuration, its impact level, its pickup details) — are the
    /// node's own, so they are not compared here.
    pub fn first_disagreeing_order_attribute(
        &self,
        input: &CreateJobInput,
    ) -> Option<&'static str> {
        for (member, requested, held) in [
            (
                "JobType",
                input.job_type.as_deref(),
                Some(self.job_type.as_str()),
            ),
            (
                "SnowballType",
                input.snowball_type.as_deref(),
                Some(self.snowball_type.as_str()),
            ),
            (
                "KmsKeyARN",
                input.kms_key_arn.as_deref(),
                self.kms_key_arn.as_deref(),
            ),
        ] {
            if supplied_value(requested).is_some_and(|requested| held != Some(requested)) {
                return Some(member);
            }
        }
        if input
            .tax_documents
            .as_ref()
            .is_some_and(|requested| self.tax_documents.as_ref() != Some(requested))
        {
            return Some("TaxDocuments");
        }

        ClusterOrderMembers::from(input).first_disagreeing_with(self)
    }
}

/// What a request supplied for an optional string member: a member the caller left out and
/// a member the caller sent empty both name nothing, so each counts as not supplied.
///
/// `RoleARN` is modelled `length 0..255`, so an empty string does reach a handler.
pub(crate) fn supplied_value(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

/// The members a cluster holds for its whole order, as one request supplied them: the
/// address the devices ship to, the forwarding address, the description, the role, the
/// shipping speed, the notification, the resources and the on-device services.
///
/// A cluster's node job holds no copy of these — "you need to provide only this clusterId
/// value. The other job attributes are inherited from the cluster"
/// (`CreateJobRequest$ClusterId`), and `ClusterMetadata` models ONE address and ONE
/// shipping speed for the whole cluster — so they are the cluster's to hold and every
/// record of the order carries the same value for each. `UpdateClusterRequest` and
/// `UpdateJobRequest` both model all eight, and an update to either end of the order
/// writes them to the same two kinds of record (the cluster, and each node job of it), so
/// the rule "apply the member the request supplied, leave an omitted member as it was"
/// lives here once, borrowed from whichever request carried it, rather than once per
/// (record, request) pair in each handler: the two operations cannot drift into disagreeing
/// about what a cluster's order carries, and a member added to or removed from the set is
/// changed in this one place.
#[derive(Debug, Clone, Copy)]
pub struct ClusterOrderMembers<'a> {
    pub address_id: Option<&'a str>,
    pub forwarding_address_id: Option<&'a str>,
    pub description: Option<&'a str>,
    pub role_arn: Option<&'a str>,
    pub shipping_option: Option<&'a str>,
    pub notification: Option<&'a Notification>,
    pub resources: Option<&'a JobResource>,
    pub on_device_service_configuration: Option<&'a OnDeviceServiceConfiguration>,
}

impl<'a> ClusterOrderMembers<'a> {
    /// Whether the request supplied any of these members at all.
    ///
    /// Asked of the same set [`Self::apply_to_cluster`] and [`Self::apply_to_job`] write, so
    /// a request this answers `false` for changes nothing about the order.
    pub fn any_supplied(&self) -> bool {
        self.address_id.is_some()
            || self.forwarding_address_id.is_some()
            || self.description.is_some()
            || self.role_arn.is_some()
            || self.shipping_option.is_some()
            || self.notification.is_some()
            || self.resources.is_some()
            || self.on_device_service_configuration.is_some()
    }

    /// Apply the members the request supplied to `cluster`, leaving every member it omitted
    /// as it was.
    ///
    /// Neither request models a member that clears a stored value — `Description` is
    /// modelled `length 1..1024`, so not even an empty string reaches a handler — so an
    /// absent member means "leave this as it is", which is what makes an update partial. The
    /// members `CreateClusterRequest` makes required are held as plain values, so they are
    /// replaced rather than wrapped. Nothing else about the cluster (its id, its job type,
    /// its device type, its state, the instant it was created, the key it was created with,
    /// the node jobs it holds) is something an update takes.
    pub fn apply_to_cluster(&self, cluster: &mut StoredCluster) {
        if let Some(address_id) = self.address_id {
            cluster.address_id = address_id.to_string();
        }
        if let Some(forwarding_address_id) = self.forwarding_address_id {
            cluster.forwarding_address_id = Some(forwarding_address_id.to_string());
        }
        if let Some(description) = self.description {
            cluster.description = Some(description.to_string());
        }
        if let Some(role_arn) = self.role_arn {
            cluster.role_arn = Some(role_arn.to_string());
        }
        if let Some(shipping_option) = self.shipping_option {
            cluster.shipping_option = shipping_option.to_string();
        }
        if let Some(notification) = self.notification {
            cluster.notification = Some(notification.clone());
        }
        if let Some(resources) = self.resources {
            cluster.resources = Some(resources.clone());
        }
        if let Some(configuration) = self.on_device_service_configuration {
            cluster.on_device_service_configuration = Some(configuration.clone());
        }
    }

    /// Apply the members the request supplied to `job`, one node job of the order, leaving
    /// every member it omitted as it was.
    ///
    /// The same set lands on the node as on the cluster, since the node carries the
    /// cluster's attributes. The members only a job has — the device it is configured with,
    /// its impact level, its pickup details, the long-term pricing it is associated with, its
    /// capacity preference — are not the cluster's, so they are left alone here, as are the
    /// node's own id, state, unlock code and creation instant.
    pub fn apply_to_job(&self, job: &mut StoredJob) {
        if let Some(address_id) = self.address_id {
            job.address_id = Some(address_id.to_string());
        }
        if let Some(forwarding_address_id) = self.forwarding_address_id {
            job.forwarding_address_id = Some(forwarding_address_id.to_string());
        }
        if let Some(description) = self.description {
            job.description = Some(description.to_string());
        }
        if let Some(role_arn) = self.role_arn {
            job.role_arn = Some(role_arn.to_string());
        }
        if let Some(shipping_option) = self.shipping_option {
            job.shipping_option = Some(shipping_option.to_string());
        }
        if let Some(notification) = self.notification {
            job.notification = Some(notification.clone());
        }
        if let Some(resources) = self.resources {
            job.resources = Some(resources.clone());
        }
        if let Some(configuration) = self.on_device_service_configuration {
            job.on_device_service_configuration = Some(configuration.clone());
        }
    }

    /// The first of these members `cluster` holds a different value for, named as the
    /// requests spell it — or `None` when every member supplied is the one the cluster
    /// already holds.
    ///
    /// The question [`Self::apply_to_cluster`] and [`Self::apply_to_job`] answer for an
    /// update, asked instead of answered: a `CreateJob` that names a cluster does not change
    /// the order it joins, so for it a member that disagrees with the cluster is a set of
    /// inputs that cannot hold together rather than a value to write. Asked over the same
    /// set those two write, from this one declaration of it, so the create and the updates
    /// cannot drift apart over what a cluster's order carries. A member the cluster holds no
    /// value for counts as disagreeing with any value supplied for it: the order has none,
    /// so no record of it has one.
    pub fn first_disagreeing_with(&self, cluster: &StoredCluster) -> Option<&'static str> {
        for (member, requested, held) in [
            ("AddressId", self.address_id, Some(cluster.address_id.as_str())),
            (
                "ForwardingAddressId",
                self.forwarding_address_id,
                cluster.forwarding_address_id.as_deref(),
            ),
            (
                "Description",
                self.description,
                cluster.description.as_deref(),
            ),
            ("RoleARN", self.role_arn, cluster.role_arn.as_deref()),
            (
                "ShippingOption",
                self.shipping_option,
                Some(cluster.shipping_option.as_str()),
            ),
        ] {
            if supplied_value(requested).is_some_and(|requested| held != Some(requested)) {
                return Some(member);
            }
        }
        if self
            .notification
            .is_some_and(|requested| cluster.notification.as_ref() != Some(requested))
        {
            return Some("Notification");
        }
        if self
            .resources
            .is_some_and(|requested| cluster.resources.as_ref() != Some(requested))
        {
            return Some("Resources");
        }
        if self.on_device_service_configuration.is_some_and(|requested| {
            cluster.on_device_service_configuration.as_ref() != Some(requested)
        }) {
            return Some("OnDeviceServiceConfiguration");
        }
        None
    }
}

impl<'a> From<&'a CreateJobInput> for ClusterOrderMembers<'a> {
    /// `CreateJobRequest` models all eight too, beside the cluster it may name, the two
    /// `ClusterMetadata` members no update models and the members that are the node's own.
    /// A create never APPLIES this projection — a node carries the cluster's values for
    /// these, so there is nothing for it to write — it only asks
    /// [`Self::first_disagreeing_with`] whether what the request names is what the cluster
    /// already holds.
    fn from(input: &'a CreateJobInput) -> Self {
        Self {
            address_id: input.address_id.as_deref(),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
            description: input.description.as_deref(),
            role_arn: input.role_arn.as_deref(),
            shipping_option: input.shipping_option.as_deref(),
            notification: input.notification.as_ref(),
            resources: input.resources.as_ref(),
            on_device_service_configuration: input.on_device_service_configuration.as_ref(),
        }
    }
}

impl<'a> From<&'a UpdateClusterInput> for ClusterOrderMembers<'a> {
    /// Every member `UpdateClusterRequest` models beyond the cluster it names is one the
    /// cluster holds for its order, so this projection drops nothing.
    fn from(input: &'a UpdateClusterInput) -> Self {
        Self {
            address_id: input.address_id.as_deref(),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
            description: input.description.as_deref(),
            role_arn: input.role_arn.as_deref(),
            shipping_option: input.shipping_option.as_deref(),
            notification: input.notification.as_ref(),
            resources: input.resources.as_ref(),
            on_device_service_configuration: input.on_device_service_configuration.as_ref(),
        }
    }
}

impl<'a> From<&'a UpdateJobInput> for ClusterOrderMembers<'a> {
    /// `UpdateJobRequest` models these eight members too, beside the job it names and the
    /// members only a job has (`SnowballCapacityPreference`, `PickupDetails`), which are the
    /// named job's own and are therefore not part of the order's set.
    fn from(input: &'a UpdateJobInput) -> Self {
        Self {
            address_id: input.address_id.as_deref(),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
            description: input.description.as_deref(),
            role_arn: input.role_arn.as_deref(),
            shipping_option: input.shipping_option.as_deref(),
            notification: input.notification.as_ref(),
            resources: input.resources.as_ref(),
            on_device_service_configuration: input.on_device_service_configuration.as_ref(),
        }
    }
}

/// The addresses one order request names: the address the Snow device ships to and the
/// forwarding address.
///
/// Every operation that places or updates an order names an address through these two
/// members — `CreateJob`, `CreateCluster`, `UpdateJob` and `UpdateCluster`, with only
/// `CreateClusterRequest` making the first one required — so the one rule asked of them
/// ([`ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState::first_unstored_address`])
/// is asked of this projection, borrowed from whichever request carried it, rather than
/// written out once per handler: the four operations cannot drift into disagreeing about
/// which members name an address or about how an id that answers to no stored address is
/// refused, exactly as [`ClusterOrderMembers`] keeps the two updates agreeing about what a
/// cluster's order carries.
#[derive(Debug, Clone, Copy, Default)]
pub struct OrderAddresses<'a> {
    /// `AddressId`: "The ID for the address that you want the Snow device shipped to".
    pub address_id: Option<&'a str>,
    /// `ForwardingAddressId`: "The forwarding address ID for a job". A forwarding address
    /// is an address too, so the same rule holds for it.
    pub forwarding_address_id: Option<&'a str>,
}

impl<'a> OrderAddresses<'a> {
    /// The address ids the request actually supplied, in the order a refusal considers
    /// them: the address the device ships to first, the forwarding address after it.
    fn named_ids(self) -> impl Iterator<Item = &'a str> {
        [self.address_id, self.forwarding_address_id]
            .into_iter()
            .flatten()
    }
}

impl<'a> From<&'a CreateJobInput> for OrderAddresses<'a> {
    /// `CreateJobRequest` leaves both members optional: a job ordered into a cluster takes
    /// the cluster's address instead of naming one.
    fn from(input: &'a CreateJobInput) -> Self {
        Self {
            address_id: input.address_id.as_deref(),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
        }
    }
}

impl<'a> From<&'a CreateClusterInput> for OrderAddresses<'a> {
    /// `CreateClusterRequest$AddressId` is required — a cluster is created for the one
    /// address its devices ship to — so it is always one of the ids the rule is asked
    /// about.
    fn from(input: &'a CreateClusterInput) -> Self {
        Self {
            address_id: Some(input.address_id.as_str()),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
        }
    }
}

impl<'a> From<&'a UpdateJobInput> for OrderAddresses<'a> {
    /// An update names an address only when it is changing one, so an omitted member names
    /// nothing to resolve.
    fn from(input: &'a UpdateJobInput) -> Self {
        Self {
            address_id: input.address_id.as_deref(),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
        }
    }
}

impl<'a> From<&'a UpdateClusterInput> for OrderAddresses<'a> {
    /// `UpdateClusterRequest` models the same two optional members, for the same reason.
    fn from(input: &'a UpdateClusterInput) -> Self {
        Self {
            address_id: input.address_id.as_deref(),
            forwarding_address_id: input.forwarding_address_id.as_deref(),
        }
    }
}

/// Which of a job's files a published URL addresses.
///
/// `GetSoftwareUpdates` "Returns an Amazon S3 presigned URL for an update file associated
/// with a specified JobId" and `GetJobManifest` "Returns a link to an Amazon S3 presigned
/// URL for the manifest file associated with the specified JobId value": each publishes a
/// URL for ONE file of ONE job, available for a window that starts at the call, so this is
/// what tells the two kinds of published URL apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFile {
    /// The Snow device software update file `GetSoftwareUpdates` publishes.
    SoftwareUpdate,
    /// The encrypted job manifest `GetJobManifest` publishes.
    Manifest,
}

impl JobFile {
    /// The Amazon S3 bucket this file is published under.
    pub fn bucket(self) -> &'static str {
        match self {
            Self::SoftwareUpdate => SOFTWARE_UPDATE_BUCKET,
            Self::Manifest => JOB_MANIFEST_BUCKET,
        }
    }

    /// The object this file is published as, under the job's own key prefix in
    /// [`Self::bucket`].
    pub fn object(self) -> &'static str {
        match self {
            Self::SoftwareUpdate => SOFTWARE_UPDATE_OBJECT,
            Self::Manifest => JOB_MANIFEST_OBJECT,
        }
    }

    /// How long a URL published for this file stays available, counted from the call that
    /// published it — the window each operation documents for its own file.
    pub fn availability(self) -> Duration {
        match self {
            Self::SoftwareUpdate => SOFTWARE_UPDATE_AVAILABILITY,
            Self::Manifest => JOB_MANIFEST_AVAILABILITY,
        }
    }

    /// The query parameter a published URL carries the grant's token in.
    pub fn token_parameter(self) -> &'static str {
        match self {
            Self::SoftwareUpdate => "updateToken",
            Self::Manifest => "manifestToken",
        }
    }

    /// The label [`mint_digest`] hashes this file's grant tokens under, so a token minted
    /// for one file is never a token minted for the other.
    fn token_label(self) -> &'static str {
        match self {
            Self::SoftwareUpdate => "software-update",
            Self::Manifest => "job-manifest",
        }
    }
}

/// One issued grant: what a `GetSoftwareUpdates` or `GetJobManifest` call handed out.
///
/// A grant is the service's record of a URL it published — which file of which job the URL
/// addresses, the token that distinguishes it from every other URL this store issued, and
/// the window it was issued for. [`Self::uri`] renders it as the `UpdatesURI` or
/// `ManifestURI` the operation reports, so the URL a caller receives is read out of this
/// record rather than assembled twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFileGrant {
    /// Which of the job's files this grant covers.
    pub file: JobFile,
    /// The job whose file this grant covers.
    pub job_id: String,
    /// This grant's own token: what makes two URLs issued for one file of one job
    /// distinct.
    pub token: String,
    /// When the call that issued this grant was served.
    pub issued_at: SystemTime,
    /// When the window closes: [`JobFile::availability`] after [`Self::issued_at`].
    pub expires_at: SystemTime,
}

impl JobFileGrant {
    /// Whether this grant is still inside its availability window at `now`.
    pub fn is_active(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }

    /// This grant as the URL the operation that issued it reports: the file's location in
    /// its bucket under the job's own key prefix, carrying the grant's token and the
    /// instant its window closes.
    ///
    /// NO SigV4 signature is attached, and no credential of this emulator's own appears in
    /// it: a fabricated signature would claim a computation nothing performed, and the
    /// emulator's own credentials must never travel in a response. The query carries what
    /// this emulator really holds for the grant — the token it minted and the expiry it
    /// recorded, as whole epoch seconds.
    pub fn uri(&self, region: &str) -> String {
        // The Region label is honoured only when the signed scope carries one with an AWS
        // Region's SHAPE: it is interpolated into a URL host, so an arbitrary header value
        // must not be able to shape one. Without such a scope the URL names S3's global
        // endpoint, which carries no Region at all.
        let bucket = self.file.bucket();
        let host = if is_region_shaped(region) {
            format!("{bucket}.s3.{region}.amazonaws.com")
        } else {
            format!("{bucket}.s3.amazonaws.com")
        };
        format!(
            "https://{host}/{job_id}/{object}?{parameter}={token}&Expires={expires}",
            job_id = self.job_id,
            object = self.file.object(),
            parameter = self.file.token_parameter(),
            token = self.token,
            expires = epoch_seconds(self.expires_at),
        )
    }
}

/// The status of the task that creates a job's return shipping label: the whole
/// `ShippingLabelStatus` enum (`shared_types::SHIPPING_LABEL_STATUS_VALUES`), so a status
/// is reported from the modelled set rather than from the two values this emulator's own
/// task can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShippingLabelStatus {
    InProgress,
    Succeeded,
    /// The task gave up before producing a label. Nothing in this emulator abandons the
    /// task it starts, so no stored label reports this.
    TimedOut,
    /// The task ran and produced no label. Nothing in this emulator fails the task it
    /// starts, so no stored label reports this.
    Failed,
}

impl ShippingLabelStatus {
    /// This status's wire value, exactly as `ShippingLabelStatus` spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "InProgress",
            Self::Succeeded => "Succeeded",
            Self::TimedOut => "TimedOut",
            Self::Failed => "Failed",
        }
    }
}

/// The current return shipping label of one job: what a `CreateReturnShippingLabel` call
/// started and what `DescribeReturnShippingLabel` reports.
///
/// The record carries the instants the label's whole lifecycle is derived from rather than
/// a stored status, so the status a caller reads is always the one the clock makes true at
/// the instant it asks: the creating task is in progress until [`Self::ready_at`] and has
/// succeeded from then on, and the label is usable until [`Self::expires_at`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReturnShippingLabel {
    /// The job whose device this label returns.
    pub job_id: String,
    /// This label's own token: what makes two labels this store minted distinct.
    pub token: String,
    /// The shipping speed the return travels at: the `ShippingOption` the create supplied,
    /// or — when it supplied none — the one the job itself was created with.
    pub shipping_option: Option<String>,
    /// When the `CreateReturnShippingLabel` call that started this label was served.
    pub requested_at: SystemTime,
    /// When the creating task finishes and the label exists:
    /// [`RETURN_SHIPPING_LABEL_PREPARATION`] after [`Self::requested_at`].
    pub ready_at: SystemTime,
    /// When the label stops being usable: [`RETURN_SHIPPING_LABEL_VALIDITY`] after
    /// [`Self::ready_at`].
    pub expires_at: SystemTime,
}

impl StoredReturnShippingLabel {
    /// The status of this label's creating task at `now`.
    pub fn status(&self, now: SystemTime) -> ShippingLabelStatus {
        if now < self.ready_at {
            ShippingLabelStatus::InProgress
        } else {
            ShippingLabelStatus::Succeeded
        }
    }

    /// Whether this is a label a caller can still use at `now`: the task that creates it
    /// has finished and the label has not expired.
    ///
    /// This is the "valid return shipping label already exists" that
    /// `ReturnShippingLabelAlreadyExistsException` describes, so a label whose window has
    /// closed is not one and a fresh create replaces it.
    pub fn is_valid(&self, now: SystemTime) -> bool {
        self.status(now) == ShippingLabelStatus::Succeeded && now < self.expires_at
    }

    /// This label as the `ReturnShippingLabelURI` `DescribeReturnShippingLabel` reports:
    /// the label file's location in [`RETURN_SHIPPING_LABEL_BUCKET`] under the job's own
    /// key prefix, carrying the label's token, the instant it expires and the shipping
    /// speed the return travels at.
    ///
    /// NO SigV4 signature is attached, and no credential of this emulator's own appears in
    /// it: a fabricated signature would claim a computation nothing performed, and the
    /// emulator's own credentials must never travel in a response. The query carries what
    /// this emulator really holds for the label — the token it minted, the expiry it
    /// recorded as whole epoch seconds, and the stored shipping option, which is a value of
    /// the modelled `ShippingOption` enum and so cannot carry a caller's own characters.
    pub fn label_uri(&self, region: &str) -> String {
        // The Region label is honoured only when the signed scope carries one with an AWS
        // Region's SHAPE: it is interpolated into a URL host, so an arbitrary header value
        // must not be able to shape one. Without such a scope the URL names S3's global
        // endpoint, which carries no Region at all.
        let host = if is_region_shaped(region) {
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.{region}.amazonaws.com")
        } else {
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.amazonaws.com")
        };
        let mut uri = format!(
            "https://{host}/{job_id}/{RETURN_SHIPPING_LABEL_OBJECT}\
             ?labelToken={token}&Expires={expires}",
            job_id = self.job_id,
            token = self.token,
            expires = epoch_seconds(self.expires_at),
        );
        if let Some(shipping_option) = &self.shipping_option {
            uri.push_str("&shippingOption=");
            uri.push_str(shipping_option);
        }
        uri
    }
}

/// `at` as whole seconds since the UNIX epoch — how a URL's `Expires` parameter spells an
/// instant.
fn epoch_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// True when `value` has an AWS Region's SHAPE: at least two lowercase alphabetic segments
/// joined by `-`, ending in a digit group (`us-west-2`, `ap-southeast-3`).
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// The region this request was signed for: the SigV4 credential scope is the only region
/// a request carries.
///
/// An empty string when the request carries no scope to read a region from — a request
/// this emulator cannot place in any Region, which `CreateAddress` treats as a Region
/// with no recorded shipping area rather than as an unserviceable one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_default()
}

/// How this service refuses `value`, submitted for the modelled member `member`, when it
/// is outside the enum catalog `catalog`: the wording the boundary validator uses for a
/// value a modelled constraint rejects.
///
/// One implementation for every handler that checks an enum member against its catalog —
/// `CreateAddress` for `Address$Type`, `CreateLongTermPricing` for `LongTermPricingType`
/// and `SnowballType` — so the wire wording cannot drift between them.
pub fn outside_catalog_message(member: &str, value: &str, catalog: &[&str]) -> String {
    format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         enum value set: [{}]",
        catalog.join(", ")
    )
}

/// Mint an id in the `<prefix><8-4-4-4-12 lowercase hex>` form this service's modelled
/// id patterns admit.
///
/// The 32 hex digits are the leading 16 bytes of a SHA-256 over the store's own
/// monotonic sequence number and the instant the record is created, so an id is the
/// service's own, is distinct from every other id this process mints, and is not a count
/// a caller can read a resource total out of.
fn mint_id(prefix: &str, seq: u64, now: SystemTime) -> String {
    let digest = mint_digest(prefix, seq, now);
    format!(
        "{prefix}{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// Hex of a SHA-256 over `label`, the store's own monotonic sequence number and the
/// instant the record is created: the opaque bytes [`mint_id`] cuts an id out of and
/// [`mint_update_token`] uses whole.
fn mint_digest(label: &str, seq: u64, now: SystemTime) -> String {
    let nanos = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(label.as_bytes());
    hasher.update(seq.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    ::hex::encode(hasher.finalize())
}

/// Mint a job's unlock code in the form `GetJobUnlockCode` documents:
/// [`UNLOCK_CODE_GROUPS`] groups of [`UNLOCK_CODE_GROUP_LEN`] alphanumeric characters
/// joined by hyphens, [`UNLOCK_CODE_LEN`] characters in all.
///
/// The characters are the leading digits of a SHA-256 over the store's own monotonic
/// sequence number and the instant the job is created — alphanumeric, as the documented
/// form requires — so a code is the service's own, is distinct from every other code this
/// process mints, and is derived from neither the job id a caller holds nor anything the
/// request carried.
fn mint_unlock_code(seq: u64, now: SystemTime) -> String {
    let digest = mint_digest("job-unlock-code", seq, now);
    let groups: Vec<String> = (0..UNLOCK_CODE_GROUPS)
        .map(|group| {
            let start = group * UNLOCK_CODE_GROUP_LEN;
            digest[start..start + UNLOCK_CODE_GROUP_LEN].to_string()
        })
        .collect();
    groups.join("-")
}

/// One supported version of a Snow on-device service, and what that version runs on.
///
/// The catalog record behind `ServiceVersion`: it carries the version itself plus one
/// entry per dependent-service pairing the version supports, so a release that runs three
/// Kubernetes minors carries three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredServiceVersion {
    /// The version, as `ServiceVersion$Version` reports it.
    pub version: String,
    /// The `ServiceName` of each service this version depends on, paired with a version
    /// of that service it supports.
    pub dependencies: Vec<(String, String)>,
}

impl StoredServiceVersion {
    /// This record as the modelled `ServiceVersion`.
    pub fn to_modeled(&self) -> ServiceVersion {
        ServiceVersion {
            version: Some(self.version.clone()),
        }
    }

    /// Whether this version runs `version` of the dependent service `service_name` —
    /// or any version of it, when `version` is `None`.
    pub fn depends_on(&self, service_name: &str, version: Option<&str>) -> bool {
        self.dependencies.iter().any(|(dependency, supported)| {
            dependency == service_name && version.is_none_or(|needed| supported == needed)
        })
    }
}

/// This emulator's on-device service version catalog, keyed by `ServiceName`.
///
/// Derived from [`EKS_ANYWHERE_SUPPORT_MATRIX`]: the EKS Anywhere entry is the matrix
/// itself, and the Kubernetes entry is every Kubernetes version some supported EKS
/// Anywhere release runs, in first-seen order over a matrix written newest-first — so
/// the Kubernetes catalog is newest-first too. Kubernetes on a Snow device depends on no
/// further on-device service, so its entries carry no dependencies.
fn seed_service_versions() -> BTreeMap<String, Vec<StoredServiceVersion>> {
    let eks_anywhere: Vec<StoredServiceVersion> = EKS_ANYWHERE_SUPPORT_MATRIX
        .iter()
        .map(|(version, kubernetes_versions)| StoredServiceVersion {
            version: (*version).to_string(),
            dependencies: kubernetes_versions
                .iter()
                .map(|kubernetes_version| {
                    (
                        ON_DEVICE_SERVICE_KUBERNETES.to_string(),
                        (*kubernetes_version).to_string(),
                    )
                })
                .collect(),
        })
        .collect();

    let mut kubernetes: Vec<StoredServiceVersion> = Vec::new();
    for (_, kubernetes_versions) in EKS_ANYWHERE_SUPPORT_MATRIX {
        for kubernetes_version in *kubernetes_versions {
            if kubernetes
                .iter()
                .all(|held| held.version != *kubernetes_version)
            {
                kubernetes.push(StoredServiceVersion {
                    version: (*kubernetes_version).to_string(),
                    dependencies: Vec::new(),
                });
            }
        }
    }

    BTreeMap::from([
        (ON_DEVICE_SERVICE_EKS_ANYWHERE.to_string(), eks_anywhere),
        (ON_DEVICE_SERVICE_KUBERNETES.to_string(), kubernetes),
    ])
}

/// One Amazon EC2-compatible image this store holds for the account.
///
/// The record behind `CompatibleImage`: the image's `AmiId`, and the `Name` the store
/// holds for it when it holds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCompatibleImage {
    /// The image's identifier, as `CompatibleImage$AmiId` reports it.
    pub ami_id: String,
    /// The image's name, as `CompatibleImage$Name` — "the optional name of a compatible
    /// image" — reports it, or `None` when the store holds no name for the image.
    pub name: Option<String>,
}

impl StoredCompatibleImage {
    /// This record as the modelled `CompatibleImage`.
    pub fn to_modeled(&self) -> CompatibleImage {
        CompatibleImage {
            ami_id: Some(self.ami_id.clone()),
            name: self.name.clone(),
        }
    }
}

/// A list operation's cursor for a page that starts at `key` — a job id for `ListJobs`,
/// a version for `ListServiceVersions`, an AMI id for `ListCompatibleImages`.
///
/// Hex of the key: the token carries nothing but the key the next page resumes at, and a
/// cursor is only ever handed back unchanged. It is not a capability — a list operation
/// resumes only from an entry the store actually holds, and refuses anything else — so the
/// encoding's job is to keep the token out of a caller's business logic, not to be
/// unguessable.
pub fn encode_cursor(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None` when
/// the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// How many entries a list operation puts on one page, given the `MaxResults` the request
/// carried.
///
/// `MaxResults` is modelled `range 0..100` on every list operation of this service that
/// takes one, and is the `@paginated` `pageSize`. A page of zero entries with a
/// continuation token would never advance the walk, so the only reading of 0 under which
/// the paginated contract terminates is "unspecified" — the same as omitting the member,
/// which gets [`DEFAULT_MAX_RESULTS`], the largest page the member admits.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|max| usize::try_from(max).ok())
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS)
}

/// One page of `records`: the list entries a listing operation reports, together with the
/// cursor its next page resumes at — `None` on the last page, since only a page with
/// records after it carries a token.
///
/// `key` is the id a cursor over this listing names (a job id, a cluster id) and `entry`
/// turns one stored record into the modelled list entry the operation reports. The whole
/// paging rule of this service lives here, so every list operation walks its records the
/// same way whatever it lists.
///
/// `next_token` is a cursor an earlier page of the SAME listing minted, naming the record
/// that page stopped before. A token that decodes to nothing, or to a record this listing
/// does not hold — one the store no longer holds, or one belonging to a different listing —
/// cannot be resumed from, which is this function's `None`: the caller answers its own
/// operation's `InvalidNextTokenException` for it.
#[allow(clippy::type_complexity)]
pub fn page_of<T, E>(
    records: &[&T],
    next_token: Option<&str>,
    page_size: usize,
    key: impl Fn(&T) -> &str,
    entry: impl Fn(&T) -> E,
) -> Option<(Vec<E>, Option<String>)> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let resume_at = decode_cursor(token)?;
            records
                .iter()
                .position(|record| key(record) == resume_at.as_str())?
        }
    };
    let page: Vec<E> = records[start..]
        .iter()
        .take(page_size)
        .map(|record| entry(record))
        .collect();
    let next = records
        .get(start + page.len())
        .map(|record| encode_cursor(key(record)));
    Some((page, next))
}

/// One page of `jobs`, as [`page_of`] pages any listing: the `JobListEntry` list a
/// job-listing operation reports, with the cursor its next page resumes at.
#[allow(clippy::type_complexity)]
pub fn page_of_jobs(
    jobs: &[&StoredJob],
    next_token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<JobListEntry>, Option<String>)> {
    page_of(
        jobs,
        next_token,
        page_size,
        |job| job.job_id.as_str(),
        StoredJob::to_job_list_entry,
    )
}

/// One page of `records`, as [`page_of`] pages any listing: the `LongTermPricingListEntry`
/// list `ListLongTermPricing` reports, with the cursor its next page resumes at.
///
/// `billed` answers, for one record, the jobs ordered under it in creation order, so an
/// entry is built from the whole store rather than from the record alone.
#[allow(clippy::type_complexity)]
pub fn page_of_long_term_pricing<'a>(
    records: &[&StoredLongTermPricing],
    next_token: Option<&str>,
    page_size: usize,
    now: SystemTime,
    billed: impl Fn(&str) -> Vec<&'a StoredJob>,
) -> Option<(Vec<LongTermPricingListEntry>, Option<String>)> {
    page_of(
        records,
        next_token,
        page_size,
        |record| record.long_term_pricing_id.as_str(),
        |record| record.to_list_entry(now, &billed(&record.long_term_pricing_id)),
    )
}

/// One page of `clusters`, as [`page_of`] pages any listing: the `ClusterListEntry` list
/// `ListClusters` reports, with the cursor its next page resumes at.
#[allow(clippy::type_complexity)]
pub fn page_of_clusters(
    clusters: &[&StoredCluster],
    next_token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<ClusterListEntry>, Option<String>)> {
    page_of(
        clusters,
        next_token,
        page_size,
        |cluster| cluster.cluster_id.as_str(),
        StoredCluster::to_cluster_list_entry,
    )
}

/// One page of `addresses`, as [`page_of`] pages any listing: the `Address` list an
/// address-listing operation reports, with the cursor its next page resumes at.
#[allow(clippy::type_complexity)]
pub fn page_of_addresses(
    addresses: &[&StoredAddress],
    next_token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<Address>, Option<String>)> {
    page_of(
        addresses,
        next_token,
        page_size,
        |address| address.address_id.as_str(),
        StoredAddress::to_modeled,
    )
}

/// One page of `images`, as [`page_of`] pages any listing: the `CompatibleImage` list
/// `ListCompatibleImages` reports, with the cursor its next page resumes at.
#[allow(clippy::type_complexity)]
pub fn page_of_compatible_images(
    images: &[StoredCompatibleImage],
    next_token: Option<&str>,
    page_size: usize,
) -> Option<(Vec<CompatibleImage>, Option<String>)> {
    // The images are read off the store's own records rather than held as a collection of
    // their own, so this is where the borrowed view `page_of` walks is taken.
    let records: Vec<&StoredCompatibleImage> = images.iter().collect();
    page_of(
        &records,
        next_token,
        page_size,
        |image| image.ami_id.as_str(),
        StoredCompatibleImage::to_modeled,
    )
}

/// What a list operation says when the `NextToken` it was handed is not one it minted for
/// the listing being walked, as `InvalidNextTokenException` documents the condition.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str =
    "The NextToken string was altered unexpectedly, and the operation has stopped. Run the \
     operation without changing the NextToken string, and try again.";

/// Per-service state for com-amazonaws-snowball-awsiesnowballjobmanagementservice. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState {
    /// Every address `CreateAddress` stored, keyed by the `AddressId` it minted.
    pub addresses: HashMap<String, StoredAddress>,
    /// The ids in `addresses` in creation order, so an address listing pages
    /// deterministically the way `job_order` lets `ListJobs` page over jobs.
    pub address_order: Vec<String>,
    /// Every job `CreateJob` stored, keyed by the `JobId` it minted.
    pub jobs: HashMap<String, StoredJob>,
    /// Every cluster `CreateCluster` stored, keyed by the `ClusterId` it minted.
    pub clusters: HashMap<String, StoredCluster>,
    /// The ids in `jobs` in creation order, so `ListJobs` pages deterministically.
    pub job_order: Vec<String>,
    /// Every long-term pricing contract `CreateLongTermPricing` stored, keyed by the
    /// `LongTermPricingId` it minted — the ONE record of which contracts this store holds,
    /// and so of which ids a job or cluster order can be billed under.
    ///
    /// A contract is created by `CreateLongTermPricing`, which is what puts a record here,
    /// so a store in which none has been created holds none and no id names a contract. An
    /// order naming one is checked against this map (through
    /// [`Self::holds_long_term_pricing`]) rather than against the id's form, so exactly the
    /// ids this emulator minted are the ones it accepts.
    pub long_term_pricing: HashMap<String, StoredLongTermPricing>,
    /// The ids in `long_term_pricing` in creation order, so `ListLongTermPricing` pages
    /// over the contracts deterministically the way `job_order` lets `ListJobs` page over
    /// jobs.
    pub long_term_pricing_order: Vec<String>,
    /// The ids in `clusters` in creation order, so `ListClusters` pages deterministically.
    pub cluster_order: Vec<String>,
    /// The versions of each Snow on-device service this emulator supports, keyed by the
    /// service's `ServiceName` and held newest-version-first, so `ListServiceVersions`
    /// pages deterministically. Seeded by [`seed_service_versions`] when the store is
    /// first touched: the supported versions of an on-device service are the service's
    /// own catalog, not something a caller creates.
    service_versions: BTreeMap<String, Vec<StoredServiceVersion>>,
    /// Every grant `GetSoftwareUpdates` and `GetJobManifest` have issued and that is still
    /// inside its availability window, oldest first — this service's record of which file
    /// URLs it published. Bounded by [`MAX_JOB_FILE_GRANTS`].
    job_file_grants: Vec<JobFileGrant>,
    /// The current return shipping label of each job that has one, keyed by `JobId`.
    ///
    /// A job has one current label — `ExpirationDate` is "the expiration date of the
    /// current return shipping label" — so this holds at most one record per stored job
    /// and is bounded by [`MAX_STORED_JOBS`] without a ceiling of its own.
    return_shipping_labels: HashMap<String, StoredReturnShippingLabel>,
    /// How many addresses this store has ever minted an id for, how many jobs, how many
    /// clusters and how many long-term pricing contracts: the per-store sequence
    /// [`mint_id`] hashes, so two records never share an id.
    next_address_seq: u64,
    next_job_seq: u64,
    next_long_term_pricing_seq: u64,
    next_cluster_seq: u64,
    /// How many published-file grants this store has ever issued: the per-store sequence
    /// the grant token hashes, so two grants never share a token.
    next_job_file_grant_seq: u64,
    /// How many return shipping labels this store has ever started: the per-store sequence
    /// the label token hashes, so two labels never share a token.
    next_return_shipping_label_seq: u64,
    /// How many unlock codes this store has ever minted: the per-store sequence
    /// [`mint_unlock_code`] hashes, so two jobs never share an unlock code.
    next_unlock_code_seq: u64,
}

impl Default for ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState {
    /// A store holding no address, no job and no long-term pricing contract, and the
    /// on-device service version catalog this emulator supports — the one state the engine
    /// builds on first touch.
    fn default() -> Self {
        Self {
            addresses: HashMap::new(),
            address_order: Vec::new(),
            jobs: HashMap::new(),
            clusters: HashMap::new(),
            job_order: Vec::new(),
            long_term_pricing: HashMap::new(),
            long_term_pricing_order: Vec::new(),
            cluster_order: Vec::new(),
            service_versions: seed_service_versions(),
            job_file_grants: Vec::new(),
            return_shipping_labels: HashMap::new(),
            next_address_seq: 0,
            next_job_seq: 0,
            next_long_term_pricing_seq: 0,
            next_cluster_seq: 0,
            next_job_file_grant_seq: 0,
            next_return_shipping_label_seq: 0,
            next_unlock_code_seq: 0,
        }
    }
}

impl ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState {
    /// The supported versions of the on-device service `service_name`, newest first, or
    /// `None` when this store holds no such service.
    pub fn service_versions(&self, service_name: &str) -> Option<&[StoredServiceVersion]> {
        self.service_versions.get(service_name).map(Vec::as_slice)
    }

    /// The Amazon EC2-compatible images this store holds for the account: every distinct
    /// `AmiId` a stored job names as an `Ec2AmiResource`, each once, in the order the
    /// store first recorded it.
    ///
    /// `ListCompatibleImages` reports "the different Amazon EC2-compatible Amazon Machine
    /// Images (AMIs) that are owned by your Amazon Web Services account that would be
    /// supported for use on a Snow device"
    /// (`smithy_query(query="operation", operation="ListCompatibleImages")`), so this is a
    /// collection of the ACCOUNT's images, not a catalog of the service's own: a store no
    /// order has named an AMI in holds none, the way one in which no address has been
    /// created holds no address. An `Ec2AmiResource` is how an order puts one of the
    /// account's AMIs on a device — `CompatibleImage` is "compatible with the device's
    /// physical hardware requirements" and `Ec2AmiResource$AmiId` is "the ID of the AMI in
    /// Amazon EC2" — so the AMIs this emulator holds for the account are exactly the ones
    /// its stored orders name, which also makes this listing agree with what `CreateJob`
    /// accepted.
    ///
    /// Every stored job is read, cluster node jobs included: a node job carries its
    /// cluster's resources, and `job_order` holds every job this store has ever stored
    /// ([`Self::insert_job_order`]). An id named by more than one order is one image of the
    /// account, so it is reported once.
    pub fn compatible_images(&self) -> Vec<StoredCompatibleImage> {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut images: Vec<StoredCompatibleImage> = Vec::new();
        for job in self.jobs_in_creation_order() {
            let named = job
                .resources
                .as_ref()
                .and_then(|resources| resources.ec2_ami_resources.as_deref())
                .unwrap_or_default();
            for image in named {
                if seen.insert(image.ami_id.as_str()) {
                    images.push(StoredCompatibleImage {
                        ami_id: image.ami_id.clone(),
                        // An order identifies an AMI by its id alone, so the store holds
                        // no name for the image and the optional member reports none.
                        name: None,
                    });
                }
            }
        }
        images
    }

    /// Write onto every contract this store holds the change its term boundary has already
    /// made due at `now` ([`StoredLongTermPricing::settle_term_at`]): a contract bought with
    /// automatic renewal is renewed for another term, one bought without is expired.
    ///
    /// Asked by every operation that takes this store before it reads or writes a contract,
    /// so what a caller is told and what a later call writes are both about the term the
    /// clock has the contract in.
    pub fn settle_long_term_pricing_terms(&mut self, now: SystemTime) {
        for contract in self.long_term_pricing.values_mut() {
            contract.settle_term_at(now);
        }
    }

    /// Whether this store holds the long-term pricing contract `long_term_pricing_id`
    /// names.
    ///
    /// Asked of the stored contracts themselves, so the ids an order may be billed under
    /// are exactly the ones `CreateLongTermPricing` minted a contract for.
    pub fn holds_long_term_pricing(&self, long_term_pricing_id: &str) -> bool {
        self.long_term_pricing.contains_key(long_term_pricing_id)
    }

    /// The first address `named` names that this store holds no address for, as the refusal
    /// that id earns — or `None` when every address the request named is one this store
    /// holds.
    ///
    /// An address is a record of this service, created by `CreateAddress`, so an id is
    /// checked against the addresses the store holds rather than against the id's form,
    /// exactly as a long-term pricing contract is ([`Self::holds_long_term_pricing`]):
    /// "The specified resource can't be found. Check the information you provided in your
    /// last request, and try again" (`InvalidResourceException`) is what an order naming
    /// one answers, carrying the kind of resource at fault in `ResourceType`.
    ///
    /// Each handler lifts the shape into its own error enum and raises it where its own
    /// refusal ordering puts it, the way the resources an order names outside this service
    /// are resolved once in [`named_resources`] and refused per operation.
    pub fn first_unstored_address(
        &self,
        named: OrderAddresses<'_>,
    ) -> Option<InvalidResourceException> {
        named
            .named_ids()
            .find(|address_id| !self.addresses.contains_key(*address_id))
            .map(|address_id| InvalidResourceException {
                message: Some(format!(
                    "The address ID {address_id} can't be found. Check the information you \
                     provided in your last request, and try again."
                )),
                resource_type: Some("address".to_string()),
            })
    }

    /// Every long-term pricing contract this store holds in creation order — the order
    /// `ListLongTermPricing` pages over.
    ///
    /// Every contract is here whatever its term has done since: nothing in this API removes
    /// one, and a contract whose term has run out is one whose end date says so.
    pub fn long_term_pricing_in_creation_order(&self) -> Vec<&StoredLongTermPricing> {
        self.long_term_pricing_order
            .iter()
            .filter_map(|id| self.long_term_pricing.get(id))
            .collect()
    }

    /// The jobs ordered under the contract `long_term_pricing_id`, in creation order.
    ///
    /// A job carries the id it was ordered under, so the association is read off the stored
    /// jobs: an export order's main job and each of its job parts carry the order's id and
    /// are each one of these jobs, exactly as `ListJobs` reports them separately.
    pub fn jobs_billed_to_long_term_pricing(&self, long_term_pricing_id: &str) -> Vec<&StoredJob> {
        self.jobs_in_creation_order()
            .into_iter()
            .filter(|job| job.long_term_pricing_id.as_deref() == Some(long_term_pricing_id))
            .collect()
    }

    /// Whether this store can hold another address, i.e. is below
    /// [`MAX_STORED_ADDRESSES`].
    pub fn can_store_another_address(&self) -> bool {
        self.addresses.len() < MAX_STORED_ADDRESSES
    }

    /// Whether this store can hold `additional` more jobs without passing
    /// [`MAX_STORED_JOBS`].
    ///
    /// An order is stored whole or not at all — an export order is a main job plus its
    /// job parts — so the ceiling is asked about the whole order rather than one record.
    pub fn can_store_jobs(&self, additional: usize) -> bool {
        self.jobs.len() + additional <= MAX_STORED_JOBS
    }

    /// Mint the next `AddressId`: the 40-character form the modelled pattern
    /// `^ADID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` admits.
    pub fn mint_address_id(&mut self) -> String {
        self.next_address_seq += 1;
        mint_id("ADID", self.next_address_seq, SystemTime::now())
    }

    /// Mint the next `JobId`: the 39-character form the modelled pattern
    /// `^(M|J)ID[0-9a-f]{8}-…$` admits, in the `JID` form a job associated with a device
    /// carries — a plain job, or one job part of an export order.
    pub fn mint_job_id(&mut self) -> String {
        self.next_job_seq += 1;
        mint_id("JID", self.next_job_seq, SystemTime::now())
    }

    /// Whether this store can hold another long-term pricing contract, i.e. is below
    /// [`MAX_STORED_LONG_TERM_PRICING`].
    pub fn can_store_another_long_term_pricing(&self) -> bool {
        self.long_term_pricing.len() < MAX_STORED_LONG_TERM_PRICING
    }

    /// Mint the next `LongTermPricingId`: the 41-character form the modelled pattern
    /// `^LTPID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` admits.
    pub fn mint_long_term_pricing_id(&mut self) -> String {
        self.next_long_term_pricing_seq += 1;
        mint_id("LTPID", self.next_long_term_pricing_seq, SystemTime::now())
    }

    /// Store `pricing` under its minted id as the newest contract, or answer `false`,
    /// storing nothing, when storing it would take this store past
    /// [`MAX_STORED_LONG_TERM_PRICING`].
    ///
    /// The ceiling is enforced HERE, at the insertion point, so a handler cannot exceed it
    /// by forgetting to ask [`Self::can_store_another_long_term_pricing`] first. It bounds
    /// GROWTH, so it is asked only of a contract whose id the store does not hold yet. This
    /// is also the only place a contract is recorded, so the map and the creation order it
    /// is paged in cannot disagree about which contracts the store holds: a contract stored
    /// under an id the store already holds replaces that record and keeps its place in the
    /// order rather than appearing in it twice.
    pub fn insert_long_term_pricing(&mut self, pricing: StoredLongTermPricing) -> bool {
        if !self
            .long_term_pricing
            .contains_key(&pricing.long_term_pricing_id)
        {
            if !self.can_store_another_long_term_pricing() {
                return false;
            }
            self.long_term_pricing_order
                .push(pricing.long_term_pricing_id.clone());
        }
        self.long_term_pricing
            .insert(pricing.long_term_pricing_id.clone(), pricing);
        true
    }

    /// Mint the next job's unlock code: the form `GetJobUnlockCode` documents, minted
    /// once per job when the job is created and held on the job's record from then on.
    pub fn mint_unlock_code(&mut self) -> String {
        self.next_unlock_code_seq += 1;
        mint_unlock_code(self.next_unlock_code_seq, SystemTime::now())
    }

    /// Whether this store can hold another cluster, i.e. is below
    /// [`MAX_STORED_CLUSTERS`].
    pub fn can_store_another_cluster(&self) -> bool {
        self.clusters.len() < MAX_STORED_CLUSTERS
    }

    /// Mint the next `ClusterId`: the 39-character form the modelled pattern
    /// `^CID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` admits.
    pub fn mint_cluster_id(&mut self) -> String {
        self.next_cluster_seq += 1;
        mint_id("CID", self.next_cluster_seq, SystemTime::now())
    }

    /// Mint the next main job's `JobId`: the same 39-character form in the `MID` variant
    /// the modelled pattern's `M` alternative is for — the id of the main job an export
    /// order's job parts hang off, which is associated with no device.
    pub fn mint_main_job_id(&mut self) -> String {
        self.next_job_seq += 1;
        mint_id("MID", self.next_job_seq, SystemTime::now())
    }

    /// Store `address` under its minted id, or answer `false` when this store is at
    /// [`MAX_STORED_ADDRESSES`].
    ///
    /// The ceiling is enforced HERE, at the insertion point, so a handler cannot exceed
    /// it by forgetting to ask [`Self::can_store_another_address`] first. It bounds
    /// GROWTH, so it is asked only of an address whose id the store does not hold yet.
    ///
    /// This is also the only place an address is recorded, so the map and the creation
    /// order it is paged in cannot disagree about which addresses the store holds: an
    /// address stored under an id the store already holds replaces that record and keeps
    /// its place in the order rather than appearing in it twice.
    pub fn insert_address(&mut self, address: StoredAddress) -> bool {
        if !self.addresses.contains_key(&address.address_id) {
            if !self.can_store_another_address() {
                return false;
            }
            self.address_order.push(address.address_id.clone());
        }
        self.addresses.insert(address.address_id.clone(), address);
        true
    }

    /// Store a whole order as the newest jobs — one plain job, or an export order's main
    /// job followed by its job parts — or answer `false`, storing nothing, when this store
    /// cannot hold all of it without passing [`MAX_STORED_JOBS`].
    ///
    /// The ceiling is enforced HERE, at the insertion point, and over the whole order, so
    /// a create can neither exceed it by forgetting to ask [`Self::can_store_jobs`] first
    /// nor leave a main job behind with the parts it needs unstored.
    pub fn insert_job_order(&mut self, jobs: Vec<StoredJob>) -> bool {
        if !self.can_store_jobs(jobs.len()) {
            return false;
        }
        for job in jobs {
            self.job_order.push(job.job_id.clone());
            self.jobs.insert(job.job_id.clone(), job);
        }
        true
    }

    /// Store `cluster` under its minted id, or answer `false` when this store is at
    /// [`MAX_STORED_CLUSTERS`].
    ///
    /// The ceiling is enforced HERE, at the insertion point, so a handler cannot exceed it
    /// by forgetting to ask [`Self::can_store_another_cluster`] first.
    pub fn insert_cluster(&mut self, cluster: StoredCluster) -> bool {
        if !self.can_store_another_cluster() {
            return false;
        }
        self.cluster_order.push(cluster.cluster_id.clone());
        self.clusters.insert(cluster.cluster_id.clone(), cluster);
        true
    }

    /// Store `nodes` as the newest jobs and register them as node jobs of `cluster_id`,
    /// or answer `false`, storing nothing, when no such cluster is held or this store
    /// cannot hold all of them without passing [`MAX_STORED_JOBS`].
    ///
    /// Registering a node is what can give the cluster its quorum, so the cluster's
    /// quorum-driven state is brought back in line HERE rather than in a handler (see
    /// [`Self::refresh_cluster_quorum_state`]).
    ///
    /// Whether the cluster is still taking nodes at all is [`ClusterState::accepts_node_jobs`],
    /// which a caller holds the cluster to first: the refusal for a cluster past that window
    /// is a modelled error of the operation asking, so it is raised there rather than
    /// folded into the `false` this answers when the store cannot hold the nodes.
    pub fn add_cluster_node_jobs(&mut self, cluster_id: &str, nodes: Vec<StoredJob>) -> bool {
        if !self.clusters.contains_key(cluster_id) || !self.can_store_jobs(nodes.len()) {
            return false;
        }
        let node_ids: Vec<String> = nodes.iter().map(|node| node.job_id.clone()).collect();
        if !self.insert_job_order(nodes) {
            return false;
        }
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return false;
        };
        cluster.node_job_ids.extend(node_ids);
        self.refresh_cluster_quorum_state(cluster_id);
        true
    }

    /// The node jobs the cluster `cluster_id` will ship with, in the order they were
    /// created: every node registered against it whose own `JobState` still counts as one
    /// of the cluster's nodes ([`JobState::is_live_cluster_node`]). Empty when this store
    /// holds no cluster under that id.
    ///
    /// This — not the length of [`StoredCluster::node_job_ids`] — is what the five-node
    /// quorum is counted over: "Each cluster supports five nodes … The cluster does not
    /// ship until these five node jobs have been created" (`CreateCluster`), so a node the
    /// cluster will never ship is not one of the five.
    pub fn live_cluster_nodes(&self, cluster_id: &str) -> Vec<&StoredJob> {
        self.cluster_jobs_in_creation_order(cluster_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|job| job.job_state.is_live_cluster_node())
            .collect()
    }

    /// Whether the cluster `cluster_id` holds the [`CLUSTER_NODE_QUORUM`] live node jobs it
    /// ships with. A cluster this store does not hold holds no node, so it has no quorum.
    pub fn cluster_has_quorum(&self, cluster_id: &str) -> bool {
        self.live_cluster_nodes(cluster_id).len() >= CLUSTER_NODE_QUORUM
    }

    /// Bring the cluster `cluster_id`'s `ClusterState` back in line with the node jobs it
    /// now holds, in BOTH directions.
    ///
    /// A cluster holding its quorum is no longer awaiting it, so it reads `Pending` — it
    /// has every node it needs and waits to be prepared and shipped (`CreateCluster`: "The
    /// cluster does not ship until these five node jobs have been created"). A cluster
    /// short of the quorum is awaiting the nodes it still needs, so it reads
    /// `AwaitingQuorum`, which is the state from which the replacement nodes
    /// `ClusterLimitExceededException` describes ("create jobs until your cluster has
    /// exactly five nodes") can still be created: the quorum is a count of live nodes, so
    /// losing one puts the cluster back to awaiting it just as creating one can complete
    /// it.
    ///
    /// Only those two states turn on the quorum. A cluster that has reached `InUse`,
    /// `Complete` or `Cancelled` keeps the state it reached, so this never resurrects a
    /// cancelled cluster or walks a shipped one back.
    pub fn refresh_cluster_quorum_state(&mut self, cluster_id: &str) {
        let has_quorum = self.cluster_has_quorum(cluster_id);
        let Some(cluster) = self.clusters.get_mut(cluster_id) else {
            return;
        };
        if matches!(
            cluster.cluster_state,
            ClusterState::AwaitingQuorum | ClusterState::Pending
        ) {
            cluster.cluster_state = if has_quorum {
                ClusterState::Pending
            } else {
                ClusterState::AwaitingQuorum
            };
        }
    }

    /// The job parts of `job_id`, in the order they were created, or an empty slice's
    /// worth when that job has none (a job part, or any job that is not a main job).
    ///
    /// A part id a main job names always resolves: nothing in this API removes a job.
    pub fn job_parts(&self, job_id: &str) -> Vec<&StoredJob> {
        self.jobs
            .get(job_id)
            .map(|job| {
                job.job_part_ids
                    .iter()
                    .filter_map(|part_id| self.jobs.get(part_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every job of the cluster `cluster_id` in the order the cluster's nodes were
    /// created — the order `ListClusterJobs` pages over — or `None` when this store holds
    /// no cluster under that id.
    ///
    /// A cluster's jobs are exactly the node jobs registered against it, whether
    /// `CreateCluster` made them from `InitialClusterSize` or a later `CreateJob` named the
    /// cluster, so the listing is read off the cluster's own record rather than by scanning
    /// every stored job.
    ///
    /// Every id a cluster names resolves: nothing in this API removes a job, which is the
    /// same invariant [`Self::job_parts`] relies on.
    pub fn cluster_jobs_in_creation_order(&self, cluster_id: &str) -> Option<Vec<&StoredJob>> {
        let cluster = self.clusters.get(cluster_id)?;
        Some(
            cluster
                .node_job_ids
                .iter()
                .filter_map(|job_id| self.jobs.get(job_id))
                .collect(),
        )
    }

    /// Every stored job in creation order — the order `ListJobs` pages over.
    pub fn jobs_in_creation_order(&self) -> Vec<&StoredJob> {
        self.job_order
            .iter()
            .filter_map(|job_id| self.jobs.get(job_id))
            .collect()
    }

    /// Every stored address in creation order — the order an address listing pages over.
    ///
    /// Every address this store holds is here: nothing in this API removes one.
    pub fn addresses_in_creation_order(&self) -> Vec<&StoredAddress> {
        self.address_order
            .iter()
            .filter_map(|address_id| self.addresses.get(address_id))
            .collect()
    }

    /// The addresses this store holds that are pickup locations, in creation order — the
    /// locations `ListPickupLocations` pages over.
    ///
    /// A pickup location is an address of the account typed as one
    /// ([`StoredAddress::is_pickup_location`]), so a delivery address is not reported as a
    /// place a device can be collected from.
    pub fn pickup_locations(&self) -> Vec<&StoredAddress> {
        self.addresses_in_creation_order()
            .into_iter()
            .filter(|address| address.is_pickup_location())
            .collect()
    }

    /// Every stored cluster in creation order — the order `ListClusters` pages over.
    ///
    /// Every cluster this store holds is here whatever state it is in: nothing in this API
    /// removes a cluster, and a cancelled one is a cluster whose `ClusterState` says so.
    pub fn clusters_in_creation_order(&self) -> Vec<&StoredCluster> {
        self.cluster_order
            .iter()
            .filter_map(|cluster_id| self.clusters.get(cluster_id))
            .collect()
    }

    /// How many Snow devices this account currently has in use.
    ///
    /// One device per stored job that has a device of its own and whose state has not
    /// released it: a job that is the main job of an export order has none of its own
    /// ([`StoredJob::is_master`]), and a state that holds no device
    /// ([`JobState::holds_device`]) leaves the order with nothing allocated. Counted over
    /// the whole store, so a cluster's node jobs count exactly as the devices they ordered.
    pub fn snowball_devices_in_use(&self) -> usize {
        self.jobs
            .values()
            .filter(|job| !job.is_master() && job.job_state.holds_device())
            .count()
    }

    /// Issue a grant for one `file` of `job_id`, valid from `now` for that file's
    /// [`JobFile::availability`], and record it.
    ///
    /// One call issues one grant: the window each operation publishes runs from "this
    /// request", so each call is its own window with its own token rather than a re-report
    /// of an earlier one. A grant whose window has closed is dropped first — nothing can be
    /// fetched under it any more and the window was over at that instant whether or not
    /// anybody called — and when the store is still at [`MAX_JOB_FILE_GRANTS`] live grants
    /// the oldest, which is the nearest to expiring, makes room.
    pub fn issue_job_file_grant(
        &mut self,
        file: JobFile,
        job_id: &str,
        now: SystemTime,
    ) -> JobFileGrant {
        self.job_file_grants.retain(|grant| grant.is_active(now));
        // Oldest first, so the grants nearest their own expiry are the ones that make room.
        // One `drain` rather than repeated removals from the front: the store holds up to
        // [`MAX_JOB_FILE_GRANTS`] grants, and shifting the whole vector per evicted grant
        // would make a call that has to evict quadratic in that ceiling.
        let over_ceiling = (self.job_file_grants.len() + 1).saturating_sub(MAX_JOB_FILE_GRANTS);
        if over_ceiling > 0 {
            self.job_file_grants.drain(..over_ceiling);
        }
        self.next_job_file_grant_seq += 1;
        let grant = JobFileGrant {
            file,
            job_id: job_id.to_string(),
            token: mint_digest(file.token_label(), self.next_job_file_grant_seq, now),
            issued_at: now,
            expires_at: now + file.availability(),
        };
        self.job_file_grants.push(grant.clone());
        grant
    }

    /// The current return shipping label of `job_id`, or `None` when no label has been
    /// started for that job.
    pub fn return_shipping_label(&self, job_id: &str) -> Option<&StoredReturnShippingLabel> {
        self.return_shipping_labels.get(job_id)
    }

    /// Start the return shipping label of `job_id` at `now`, travelling at
    /// `shipping_option`, and record it as that job's current label.
    ///
    /// A job has ONE current label, so this REPLACES whatever label the job held — which a
    /// caller can only reach once the old one has expired, since a valid label is what
    /// `ReturnShippingLabelAlreadyExistsException` refuses a second create for. The label
    /// is not ready at the instant the call is served: the task that creates it runs for
    /// [`RETURN_SHIPPING_LABEL_PREPARATION`], and the label it produces is then usable for
    /// [`RETURN_SHIPPING_LABEL_VALIDITY`].
    pub fn start_return_shipping_label(
        &mut self,
        job_id: &str,
        shipping_option: Option<String>,
        now: SystemTime,
    ) -> StoredReturnShippingLabel {
        self.next_return_shipping_label_seq += 1;
        let ready_at = now + RETURN_SHIPPING_LABEL_PREPARATION;
        let label = StoredReturnShippingLabel {
            job_id: job_id.to_string(),
            token: mint_digest(
                "return-shipping-label",
                self.next_return_shipping_label_seq,
                now,
            ),
            shipping_option,
            requested_at: now,
            ready_at,
            expires_at: ready_at + RETURN_SHIPPING_LABEL_VALIDITY,
        };
        self.return_shipping_labels
            .insert(job_id.to_string(), label.clone());
        label
    }

    /// The grants issued for one `file` of `job_id` that are still inside their
    /// availability window at `now`, oldest first.
    pub fn active_job_file_grants(
        &self,
        file: JobFile,
        job_id: &str,
        now: SystemTime,
    ) -> Vec<&JobFileGrant> {
        self.job_file_grants
            .iter()
            .filter(|grant| grant.file == file && grant.job_id == job_id && grant.is_active(now))
            .collect()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState>)
}
