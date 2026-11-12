// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-devicefarm-devicefarm-20150623 service.
//!
//! The persisted records live here, together with the small helpers every Project
//! handler shares (ARN minting and classification, the request's signing region, and
//! the record -> modelled-shape conversion). `src/ops/mod.rs` is generated from the
//! model and cannot carry a hand-written module, so this editable file is the service's
//! one home for code more than one handler needs.

use std::collections::BTreeMap;

use crate::ops::types::{
    DevicePool, EnvironmentVariable, NetworkProfile, Project, Rule, Upload, VpcConfig,
};

/// The EC2 resources a project's `vpcConfig` names, resolved against EC2 — the one rule
/// `CreateProject` and `UpdateProject` both apply before they write.
mod vpc_config;

pub use vpc_config::unresolved_vpc_config;

/// The execution timeout a project gets when `CreateProject` is called without
/// `defaultJobTimeoutMinutes`.
///
/// The value is the model's own documented default for `Project$defaultJobTimeoutMinutes`
/// ("The default number of minutes (at the project level) a test run executes before it
/// times out. The default value is 150 minutes."), so it is a documented duration rather
/// than a chosen one.
pub const DEFAULT_JOB_TIMEOUT_MINUTES: i32 = 150;

/// The resource type a project ARN carries: `arn:aws:devicefarm:<region>:<account>:project:<id>`.
pub const PROJECT_RESOURCE_TYPE: &str = "project";

/// The resource type a network-profile ARN carries:
/// `arn:aws:devicefarm:<region>:<account>:networkprofile:<project-id>/<profile-id>`.
pub const NETWORK_PROFILE_RESOURCE_TYPE: &str = "networkprofile";

/// The resource type a device-pool ARN carries:
/// `arn:aws:devicefarm:<region>:<account>:devicepool:<project-id>/<pool-id>`.
pub const DEVICE_POOL_RESOURCE_TYPE: &str = "devicepool";

/// The only type a device pool created through the API can have.
///
/// `DevicePoolType` models two values, and the model's own member documentation splits
/// them by who owns the pool: a `CURATED` pool is "created and managed by AWS Device
/// Farm", a `PRIVATE` one "created and managed by the device pool developer" — so a pool a
/// caller creates is reported as `PRIVATE`.
pub const DEVICE_POOL_TYPE_PRIVATE: &str = "PRIVATE";

/// The resource type an upload ARN carries:
/// `arn:aws:devicefarm:<region>:<account>:upload:<project-id>/<upload-id>`.
pub const UPLOAD_RESOURCE_TYPE: &str = "upload";

/// The status a freshly created upload has: the file name and type are registered, and the
/// content has not been stored against them yet.
///
/// `UploadStatus` models INITIALIZED, PROCESSING, SUCCEEDED and FAILED; `CreateUpload`
/// registers the upload before any content exists for it, so INITIALIZED is the status it
/// starts in.
pub const UPLOAD_STATUS_INITIALIZED: &str = "INITIALIZED";

/// The status an upload has while the content stored against it is being read.
pub const UPLOAD_STATUS_PROCESSING: &str = "PROCESSING";

/// The terminal status of an upload whose content was read as the package its type names.
pub const UPLOAD_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The terminal status of an upload whose content could not be read as the package its
/// type names.
pub const UPLOAD_STATUS_FAILED: &str = "FAILED";

/// How long an upload stays PROCESSING before its terminal status is reported.
///
/// Neither the model nor AWS documents how long Device Farm takes to process an upload, so
/// this span is the emulator's OWN: it only has to be non-zero, so a caller that reads the
/// upload straight after storing its content still sees PROCESSING, exactly as a caller of
/// the real service can.
pub const UPLOAD_PROCESSING_SPAN: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The segment that names THIS data plane under the emulator's reserved data-plane path
/// namespace (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`): a content
/// locator is `/_aws/<this>/<project-id>/<upload-id>`.
pub const UPLOAD_CONTENT_PLANE: &str = "devicefarm-uploads";

/// The category an upload created through the API has.
///
/// `UploadCategory` models two values, documented as "CURATED: An upload managed by AWS
/// Device Farm" and "PRIVATE: An upload managed by the AWS Device Farm customer" — so an
/// upload a caller creates in its own project is PRIVATE.
pub const UPLOAD_CATEGORY_PRIVATE: &str = "PRIVATE";

/// The only type a network profile created through the API can have.
///
/// `NetworkProfileType` models two values: a `CURATED` profile is one the service itself
/// publishes, and a profile a caller creates inside its own project is its own — so the
/// created profile is reported as `PRIVATE` whatever type the request asked for.
pub const NETWORK_PROFILE_TYPE_PRIVATE: &str = "PRIVATE";

/// The throughput a network profile gets for a direction the request does not shape.
///
/// `CreateNetworkProfile`'s bandwidth members document the range "an integer from 0 to
/// 104857600", and the top of that range is an unshaped link: a profile created without a
/// bandwidth imposes no throughput limit. The value is therefore the modelled maximum
/// rather than a figure of this emulator's own choosing, while which end of the range an
/// omitted member lands on is the emulator's reading of "unshaped".
pub const DEFAULT_NETWORK_PROFILE_BANDWIDTH_BITS: i64 = 104_857_600;

/// The delay and jitter a network profile gets for a direction the request does not shape:
/// none. Both members document milliseconds "from 0 to 2000", and an unshaped link adds
/// neither — the same reading as [`DEFAULT_NETWORK_PROFILE_BANDWIDTH_BITS`], at the other
/// end of the range because delay and jitter are impairments rather than capacity.
pub const DEFAULT_NETWORK_PROFILE_DELAY_MS: i64 = 0;

/// How many projects one `ListProjects` page carries.
///
/// Device Farm's `ListProjects` models no `maxResults`, and AWS documents no page size
/// for it, so this bound is CHOSEN: it only has to be a fixed positive number so a
/// truncated page hands back a `nextToken` and a resumed call continues from it.
pub const PROJECTS_PAGE_SIZE: usize = 50;

/// How many projects the store holds before `CreateProject` refuses with the modelled
/// `LimitExceededException`.
///
/// Device Farm publishes an adjustable per-account quota on projects, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the emulator's
/// OWN bound on the store rather than a claim about any account's limit. It is set far
/// above any workflow an emulator serves, and it is what stops the state growing without
/// end: every record it holds is released by `DeleteProject`.
pub const PROJECT_QUOTA: usize = 1_000;

/// How many device pools the store holds before `CreateDevicePool` refuses with the
/// modelled `LimitExceededException`.
///
/// Like [`PROJECT_QUOTA`], this is the emulator's OWN bound on the store rather than a
/// claim about any account's published quota, set far above any workflow an emulator
/// serves: it is what stops repeated calls growing the state without end.
pub const DEVICE_POOL_QUOTA: usize = 1_000;

/// How many network profiles the store holds before `CreateNetworkProfile` refuses with
/// the modelled `LimitExceededException`.
///
/// Like [`PROJECT_QUOTA`], this is the emulator's OWN bound on the store rather than a
/// claim about any account's published quota, set far above any workflow an emulator
/// serves: it is what stops repeated calls growing the state without end.
pub const NETWORK_PROFILE_QUOTA: usize = 1_000;

/// How many uploads the store holds before `CreateUpload` refuses with the modelled
/// `LimitExceededException`.
///
/// Like [`PROJECT_QUOTA`], this is the emulator's OWN bound on the store rather than a
/// claim about any account's published quota, set far above any workflow an emulator
/// serves: it is what stops repeated calls growing the state without end.
pub const UPLOAD_QUOTA: usize = 1_000;

/// One stored project: the single source of truth every Project response is built from.
///
/// The ARN and `created` instant are minted once, at create time, and never recomputed —
/// so every later `GetProject`/`ListProjects` reports byte-identical values.
#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub arn: String,
    pub name: String,
    pub default_job_timeout_minutes: i32,
    pub created: ::aws_smithy_types::DateTime,
    pub vpc_config: Option<VpcConfig>,
    pub environment_variables: Vec<EnvironmentVariable>,
    pub execution_role_arn: Option<String>,
}

impl ProjectRecord {
    /// The modelled `Project` for this record. Every member is read off the record, so a
    /// response can never carry a value the store does not hold.
    pub fn to_project(&self) -> Project {
        Project {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            default_job_timeout_minutes: Some(self.default_job_timeout_minutes),
            created: Some(self.created),
            vpc_config: self.vpc_config.clone(),
            // An absent list and an empty list are different answers on the wire: a
            // project created without environment variables reports none at all.
            environment_variables: if self.environment_variables.is_empty() {
                None
            } else {
                Some(self.environment_variables.clone())
            },
            execution_role_arn: self.execution_role_arn.clone(),
        }
    }
}

/// One stored network profile: the single source of truth every `NetworkProfile` response
/// is built from.
///
/// Every shaping member is stored RESOLVED — the value the request asked for, or the
/// default the service applies for a direction it did not shape — so no response has to
/// re-derive one, and `type` is stored as the service classified the profile rather than
/// as the request named it.
#[derive(Debug, Clone)]
pub struct NetworkProfileRecord {
    pub arn: String,
    /// The project the profile belongs to. Kept so `ListNetworkProfiles` can select the
    /// profiles of one project without parsing the ARN back apart.
    pub project_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub r#type: String,
    pub uplink_bandwidth_bits: i64,
    pub downlink_bandwidth_bits: i64,
    pub uplink_delay_ms: i64,
    pub downlink_delay_ms: i64,
    pub uplink_jitter_ms: i64,
    pub downlink_jitter_ms: i64,
    pub uplink_loss_percent: i32,
    pub downlink_loss_percent: i32,
}

impl NetworkProfileRecord {
    /// The modelled `NetworkProfile` for this record. Every member is read off the
    /// record, so a response can never carry a value the store does not hold.
    pub fn to_network_profile(&self) -> NetworkProfile {
        NetworkProfile {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            r#type: Some(self.r#type.clone()),
            uplink_bandwidth_bits: Some(self.uplink_bandwidth_bits),
            downlink_bandwidth_bits: Some(self.downlink_bandwidth_bits),
            uplink_delay_ms: Some(self.uplink_delay_ms),
            downlink_delay_ms: Some(self.downlink_delay_ms),
            uplink_jitter_ms: Some(self.uplink_jitter_ms),
            downlink_jitter_ms: Some(self.downlink_jitter_ms),
            uplink_loss_percent: self.uplink_loss_percent,
            downlink_loss_percent: self.downlink_loss_percent,
        }
    }
}

/// One stored device pool: the single source of truth every `DevicePool` response is
/// built from.
///
/// The pool's rules are the caller's own selection criteria, so they are stored exactly as
/// the request supplied them and reported back unchanged; `type` is stored as the service
/// classified the pool rather than as anything the request asked for, and `max_devices` is
/// absent when the request set no ceiling.
#[derive(Debug, Clone)]
pub struct DevicePoolRecord {
    pub arn: String,
    /// The project the pool was created in. A device pool OUTLIVES that project (deleting
    /// a project releases the project record only), so this is the owner the pool was
    /// minted under and never a guarantee the project is still there.
    pub project_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub r#type: String,
    pub rules: Vec<Rule>,
    pub max_devices: Option<i32>,
}

impl DevicePoolRecord {
    /// The modelled `DevicePool` for this record. Every member is read off the record, so a
    /// response can never carry a value the store does not hold.
    pub fn to_device_pool(&self) -> DevicePool {
        DevicePool {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            r#type: Some(self.r#type.clone()),
            // `rules` is a required member of the create request, so a stored pool always
            // has the list the caller supplied \u2014 an empty one included.
            rules: Some(self.rules.clone()),
            max_devices: self.max_devices,
        }
    }
}

/// One stored upload: the single source of truth every `Upload` response is built from.
///
/// The ARN, the `created` instant and the content locator are minted once, at create time,
/// and never recomputed — so every later `GetUpload` reports byte-identical values.
#[derive(Debug, Clone)]
pub struct UploadRecord {
    pub arn: String,
    /// The project the upload belongs to. Kept so an upload can be selected by its owning
    /// project without parsing the ARN back apart.
    pub project_arn: String,
    pub name: String,
    pub created: ::aws_smithy_types::DateTime,
    pub r#type: String,
    pub status: String,
    pub url: Option<String>,
    pub metadata: Option<String>,
    pub content_type: Option<String>,
    pub message: Option<String>,
    pub category: String,
    /// The outcome the content stored against this upload was read to, and the instant it
    /// becomes the upload's reported status. `None` for an upload no content has been
    /// stored against, and cleared once the outcome has been applied.
    ///
    /// Crate-private: it is written by `CreateUpload` (as `None`) and by
    /// [`UploadRecord::begin_processing`], and read only by [`UploadRecord::settle`], so no
    /// response can report a status the clock has not made due.
    pub(crate) processing: Option<PendingProcessing>,
}

/// The terminal status an upload's processing reached, waiting for the instant it is
/// reported from.
#[derive(Debug, Clone)]
pub(crate) struct PendingProcessing {
    /// When the outcome becomes the upload's reported status, per
    /// [`UPLOAD_PROCESSING_SPAN`].
    due: ::std::time::Instant,
    outcome: ProcessingOutcome,
}

/// How the content stored against an upload was read: the terminal status it puts the
/// upload in, and what the service has to say about it.
#[derive(Debug, Clone)]
pub struct ProcessingOutcome {
    /// One of [`UPLOAD_STATUS_SUCCEEDED`] or [`UPLOAD_STATUS_FAILED`].
    pub status: &'static str,
    /// "A message about the upload's result" (`Upload$message`) — carried by an upload
    /// whose processing has something to report, which is a failure.
    pub message: Option<String>,
    /// "The upload's metadata" (`Upload$metadata`), the document Device Farm reports about
    /// the content it read: what the processing found, in JSON.
    pub metadata: Option<String>,
}

impl UploadRecord {
    /// The modelled `Upload` for this record. Every member is read off the record, so a
    /// response can never carry a value the store does not hold.
    pub fn to_upload(&self) -> Upload {
        Upload {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            created: Some(self.created),
            r#type: Some(self.r#type.clone()),
            status: Some(self.status.clone()),
            url: self.url.clone(),
            metadata: self.metadata.clone(),
            content_type: self.content_type.clone(),
            message: self.message.clone(),
            category: Some(self.category.clone()),
        }
    }

    /// Take the content stored against this upload into processing: the upload reports
    /// PROCESSING, and `outcome` becomes its status once [`UPLOAD_PROCESSING_SPAN`] has
    /// passed.
    ///
    /// Storing content again restarts the processing of the upload, because the outcome is
    /// the outcome of the content it now holds.
    pub fn begin_processing(&mut self, outcome: ProcessingOutcome, now: ::std::time::Instant) {
        self.status = UPLOAD_STATUS_PROCESSING.to_string();
        self.message = None;
        self.metadata = None;
        self.processing = Some(PendingProcessing {
            due: now + UPLOAD_PROCESSING_SPAN,
            outcome,
        });
    }

    /// Apply the processing outcome the clock has already made due, if any.
    ///
    /// A reporting operation calls this before it reads the record: the transition was owed
    /// at `now` whether or not anybody asked, so applying it is materialising a change the
    /// clock made, never a change the read itself caused.
    pub fn settle(&mut self, now: ::std::time::Instant) {
        let Some(pending) = &self.processing else {
            return;
        };
        if now < pending.due {
            return;
        }
        let outcome = pending.outcome.clone();
        self.status = outcome.status.to_string();
        self.message = outcome.message;
        self.metadata = outcome.metadata;
        self.processing = None;
    }
}

/// Per-service state for com-amazonaws-devicefarm-devicefarm-20150623. A handler reaches it through [`service_state`].
///
/// `projects` is keyed by project ARN, so Get/Update/Delete are O(log n) and
/// `ListProjects` has one deterministic (ARN-ordered) page sequence. `network_profiles`
/// is keyed by network-profile ARN for the same reasons.
#[derive(Debug, Default)]
pub struct ComAmazonawsDevicefarmDevicefarm20150623State {
    pub projects: BTreeMap<String, ProjectRecord>,
    pub network_profiles: BTreeMap<String, NetworkProfileRecord>,
    /// Keyed by device-pool ARN, for the same reasons as `projects`.
    pub device_pools: BTreeMap<String, DevicePoolRecord>,
    /// Keyed by upload ARN, for the same reasons as `projects`.
    pub uploads: BTreeMap<String, UploadRecord>,
    /// The sequence the next minted project id comes from. Private, and only ever
    /// advanced by [`Self::mint_project_arn`], so no two projects can share an id and a
    /// released id is never handed out again.
    next_project_id: u64,
    /// The sequence the next minted network-profile id comes from, with the same
    /// guarantee as `next_project_id`.
    next_network_profile_id: u64,
    /// The sequence the next minted device-pool id comes from, with the same guarantee as
    /// `next_project_id`.
    next_device_pool_id: u64,
    /// The sequence the next minted upload id comes from, with the same guarantee as
    /// `next_project_id`.
    next_upload_id: u64,
}

impl ComAmazonawsDevicefarmDevicefarm20150623State {
    /// Mint a fresh, unique project ARN in `region`, advancing the store's sequence.
    ///
    /// The resource portion is `project:<id>`, the form a Device Farm project ARN takes;
    /// the account segment and the ARN frame come from the core's `emulator_arn`. The id
    /// itself is opaque to a client — the model constrains a project ARN only to
    /// `^arn:aws:devicefarm:.+$` (length 32..1011) — so the store's own never-reused
    /// sequence supplies it, which is what makes a minted ARN unique without any outside
    /// source of identity. The ARN is stored on the record and never recomputed.
    pub fn mint_project_arn(&mut self, region: &str) -> String {
        self.next_project_id = self.next_project_id.wrapping_add(1);
        ::emulator_server::emulator_arn(
            "devicefarm",
            region,
            &format!(
                "{PROJECT_RESOURCE_TYPE}:{:016x}-{}",
                self.next_project_id, RESOURCE_ID_SUFFIX
            ),
        )
    }

    /// Mint a fresh, unique network-profile ARN in `region` under `project_arn`,
    /// advancing the store's sequence.
    ///
    /// A Device Farm resource that lives inside a project carries both ids:
    /// `networkprofile:<project-id>/<profile-id>`, so the owning project is readable off
    /// the ARN exactly as it is in the service's own ARNs. The project id is the one the
    /// project ARN already holds, and the profile id comes from the store's never-reused
    /// sequence. The ARN is stored on the record and never recomputed.
    pub fn mint_network_profile_arn(&mut self, region: &str, project_arn: &str) -> String {
        self.next_network_profile_id = self.next_network_profile_id.wrapping_add(1);
        project_child_arn(
            region,
            project_arn,
            NETWORK_PROFILE_RESOURCE_TYPE,
            self.next_network_profile_id,
        )
    }

    /// Mint a fresh, unique device-pool ARN in `region` under `project_arn`, advancing the
    /// store's sequence.
    ///
    /// A device pool is a resource that lives inside a project, so its ARN carries both
    /// ids (`devicepool:<project-id>/<pool-id>`) exactly as a network profile's does. The
    /// ARN is stored on the record and never recomputed.
    pub fn mint_device_pool_arn(&mut self, region: &str, project_arn: &str) -> String {
        self.next_device_pool_id = self.next_device_pool_id.wrapping_add(1);
        project_child_arn(
            region,
            project_arn,
            DEVICE_POOL_RESOURCE_TYPE,
            self.next_device_pool_id,
        )
    }

    /// Mint a fresh, unique upload ARN in `region` under `project_arn`, advancing the
    /// store's sequence.
    ///
    /// An upload lives inside a project, so its ARN carries both ids
    /// (`upload:<project-id>/<upload-id>`) exactly as a network profile's does. The ARN is
    /// stored on the record and never recomputed.
    pub fn mint_upload_arn(&mut self, region: &str, project_arn: &str) -> String {
        self.next_upload_id = self.next_upload_id.wrapping_add(1);
        let project_id = devicefarm_resource_id(project_arn, PROJECT_RESOURCE_TYPE).unwrap_or("");
        ::emulator_server::emulator_arn(
            "devicefarm",
            region,
            &format!(
                "{UPLOAD_RESOURCE_TYPE}:{project_id}/{:016x}-{}",
                self.next_upload_id, RESOURCE_ID_SUFFIX
            ),
        )
    }

    /// The upload whose published content locator carries `resource_id`
    /// (`<project-id>/<upload-id>`), for writing.
    ///
    /// A resource id comes from the store's own never-reused sequence, so it names at most
    /// one upload however many regions the store holds uploads in.
    pub fn upload_of_resource_id_mut(&mut self, resource_id: &str) -> Option<&mut UploadRecord> {
        self.uploads.values_mut().find(|record| {
            devicefarm_resource_id(&record.arn, UPLOAD_RESOURCE_TYPE) == Some(resource_id)
        })
    }

    /// Whether the store holds an upload whose published content locator carries
    /// `resource_id`.
    pub fn holds_upload_of_resource_id(&self, resource_id: &str) -> bool {
        self.uploads.values().any(|record| {
            devicefarm_resource_id(&record.arn, UPLOAD_RESOURCE_TYPE) == Some(resource_id)
        })
    }
}

/// The ARN of a resource of type `resource_type` that lives inside `project_arn`, with the
/// store-supplied `id`.
///
/// A Device Farm resource created inside a project carries both ids in the ARN's resource
/// field, `<type>:<project-id>/<resource-id>`, so the owning project is readable off the
/// ARN exactly as it is in the service's own ARNs. The account segment and the ARN frame
/// come from the core's `emulator_arn`.
fn project_child_arn(region: &str, project_arn: &str, resource_type: &str, id: u64) -> String {
    let project_id = devicefarm_resource_id(project_arn, PROJECT_RESOURCE_TYPE).unwrap_or("");
    ::emulator_server::emulator_arn(
        "devicefarm",
        region,
        &format!("{resource_type}:{project_id}/{id:016x}-{RESOURCE_ID_SUFFIX}"),
    )
}

/// The fixed tail every minted resource id carries, so an ARN is recognisable as one this
/// emulator minted and comfortably clears the modelled minimum ARN length of 32.
const RESOURCE_ID_SUFFIX: &str = "emulated";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDevicefarmDevicefarm20150623State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDevicefarmDevicefarm20150623State>)
}

/// The region this request was signed for, which is the region the resources it creates
/// live in.
///
/// Read from the SigV4 credential scope (`Credential=<akid>/<date>/<region>/<service>/aws4_request`)
/// — the only region a request carries. An unsigned request falls back to the emulator's
/// default region, as `emulator_arn` documents.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request's resources are placed in.
const DEFAULT_REGION: &str = "us-east-1";

/// The resource id of `arn`, if `arn` names a Device Farm resource of type
/// `resource_type`.
///
/// Applies the ARN grammar: `arn:<partition>:<service>:<region>:<account>:<type>:<id>`,
/// with a non-empty resource id. A resource that lives inside a project carries both ids
/// in that one field (`<project-id>/<profile-id>`), which is why the id is returned whole
/// rather than split further.
pub fn devicefarm_resource_id<'a>(arn: &'a str, resource_type: &str) -> Option<&'a str> {
    let mut fields = arn.split(':');
    match (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) {
        (
            Some("arn"),
            Some(_partition),
            Some("devicefarm"),
            Some(_region),
            Some(_account),
            Some(found_type),
            Some(id),
            None,
        ) if found_type == resource_type && !id.is_empty() => Some(id),
        _ => None,
    }
}

/// Whether `arn` names a Device Farm resource of type `resource_type`.
///
/// Device Farm's ARN members are modelled only as `^arn:aws:devicefarm:.+$`, so an ARN
/// naming some OTHER Device Farm resource type (an upload, a run, a device pool) reaches a
/// handler and must be refused as an invalid argument rather than reported as a missing
/// resource of the type the operation expects.
pub fn is_devicefarm_arn_of_type(arn: &str, resource_type: &str) -> bool {
    devicefarm_resource_id(arn, resource_type).is_some()
}

/// The message a Device Farm `NotFoundException` carries when the ARN a request supplied
/// resolves to no resource of the type that member names.
///
/// Device Farm words that absence `No <resource type> found for arn <arn>`: the text around
/// the two variable parts is fixed, the resource is named by the very token its own ARN
/// carries in the resource field (`networkprofile`, `upload`, `devicepool` — one word, as
/// the ARN spells it, never the English phrase), and `arn` is echoed verbatim from the
/// request's own ARN member rather than re-derived from the store. A project is the one
/// type the service words differently ([`project_not_found_message`]), so pass a resource
/// type here and never a phrase of this crate's own making.
pub fn arn_not_found_message(resource_type: &str, arn: &str) -> String {
    format!("No {resource_type} found for arn {arn}")
}

/// The message a Device Farm `NotFoundException` carries for an ARN that resolves to no
/// project.
///
/// The service words a missing project `No project was found for arn <arn>` — the same
/// sentence as [`arn_not_found_message`] with `was` in it, which is why a project does not
/// go through that one.
pub fn project_not_found_message(arn: &str) -> String {
    format!("No {PROJECT_RESOURCE_TYPE} was found for arn {arn}")
}

/// Whether `arn` names a project.
pub fn is_project_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, PROJECT_RESOURCE_TYPE)
}

/// Whether `arn` names a network profile.
pub fn is_network_profile_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, NETWORK_PROFILE_RESOURCE_TYPE)
}

/// Whether `arn` names a device pool.
pub fn is_device_pool_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, DEVICE_POOL_RESOURCE_TYPE)
}

/// Whether `arn` names an upload.
pub fn is_upload_arn(arn: &str) -> bool {
    is_devicefarm_arn_of_type(arn, UPLOAD_RESOURCE_TYPE)
}

/// Where the content of the upload `arn` names is stored against it, as the modelled `url`
/// member reports it — on `host`, the endpoint the creating request reached
/// (`service_common::data_plane_addressing::reached_endpoint_host`).
///
/// AWS answers a presigned Amazon S3 URL here. This emulator has no object data plane, so
/// the locator names THIS upload on this emulator's own endpoint, under the workspace's
/// reserved data-plane path namespace so it collides with no modelled route, and this
/// service SERVES it (`crate::service`): the content a caller stores against the locator is
/// what the upload's processing then reads, so the URL the response publishes can actually
/// be used.
pub fn upload_content_url(host: &str, arn: &str) -> Option<String> {
    let resource_id = devicefarm_resource_id(arn, UPLOAD_RESOURCE_TYPE)?;
    Some(format!(
        "https://{host}/{namespace}/{UPLOAD_CONTENT_PLANE}/{resource_id}",
        namespace = ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE,
    ))
}

/// The upload resource id a request path names, when the path is a content locator this
/// service publishes ([`upload_content_url`]).
///
/// The path is read back through the workspace's one reader of data-plane addressing, so
/// the namespace and the plane segment are judged exactly as every other plane judges
/// them. Reading it is a syntax check only: whether it names an upload the store holds is
/// the store's own judgement.
pub fn upload_content_resource_id(path: &str) -> Option<&str> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};

    let (namespace, under_namespace) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, under_plane) = first_segment(under_namespace)?;
    if plane != UPLOAD_CONTENT_PLANE {
        return None;
    }
    let resource_id = under_plane.strip_prefix('/')?;
    let mut segments = resource_id.split('/');
    match (segments.next(), segments.next(), segments.next()) {
        (Some(project_id), Some(upload_id), None)
            if !project_id.is_empty() && !upload_id.is_empty() =>
        {
            Some(resource_id)
        }
        _ => None,
    }
}

/// What the content stored against an upload of a given `UploadType` is read as.
///
/// `UploadType` (read with `smithy_query(query="shape",shape="UploadType")`) names three
/// kinds of upload: the two platform APP packages, whose content is that platform's package
/// format; the `*_TEST_PACKAGE` types, whose content is the test archive Device Farm
/// unpacks; and the rest — `EXTERNAL_DATA` (data a test reads), `WEB_APP` and the
/// `*_TEST_SPEC` types (a test spec document) — whose content Device Farm does not read as
/// an archive at all.
///
/// One classification serves the judgement the content plane makes of the bytes; what file
/// name a type may carry is its own rule ([`upload_name_rule`]), because a type whose
/// content Device Farm does not unpack still has a documented file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadContentFormat {
    /// An Android application package: a zip container carrying an Android manifest.
    AndroidApp,
    /// An iOS application archive: a zip container carrying an app bundle under `Payload/`.
    IosApp,
    /// A zip container, whatever it carries — the form every Device Farm test package
    /// takes.
    ZipArchive,
    /// Content the service does not read: it is accepted as whatever was uploaded.
    Opaque,
}

/// The format an upload of `upload_type` is read as.
pub fn upload_content_format(upload_type: &str) -> UploadContentFormat {
    match upload_type {
        "ANDROID_APP" => UploadContentFormat::AndroidApp,
        "IOS_APP" => UploadContentFormat::IosApp,
        _ if upload_type.ends_with("_TEST_PACKAGE") => UploadContentFormat::ZipArchive,
        _ => UploadContentFormat::Opaque,
    }
}

/// The file-name extensions an upload of `upload_type` may carry, and the sentence a name
/// carrying none of them is refused with — or `None` for a type whose file name the service
/// does not constrain.
///
/// Every rule and every sentence is the model's own documentation, PER TYPE.
/// `CreateUpload$name` states two: "If you are uploading an iOS app, the file name must end
/// with the .ipa extension. If you are uploading an Android app, the file name must end
/// with the .apk extension." `UpdateUpload$name` states the third, for the file AWS's
/// custom-test-environment workflow uploads: "The test spec file name must end with the
/// .yaml or .yml file extension."
///
/// The same sentence continues "For all others, the file name must end with the .zip file
/// extension", and that clause is deliberately NOT enforced: the behaviour recorded from
/// AWS for this emulator has a `CreateUpload` of a non-app type SERVED under a file name
/// that does not end `.zip`, so refusing it would refuse a call the service accepts. The
/// documented clause is a convention of the console workflow rather than a constraint the
/// API applies, and a recorded response outranks the prose where the two disagree.
pub fn upload_name_rule(upload_type: &str) -> Option<(&'static [&'static str], &'static str)> {
    match upload_type {
        "ANDROID_APP" => Some((
            &[".apk"],
            "The file name must end with the .apk extension",
        )),
        "IOS_APP" => Some((
            &[".ipa"],
            "The file name must end with the .ipa extension",
        )),
        _ if upload_type.ends_with("_TEST_SPEC") => Some((
            &[".yaml", ".yml"],
            "The test spec file name must end with the .yaml or .yml file extension",
        )),
        _ => None,
    }
}

/// The separator a file name may not contain, per `CreateUpload$name` ("The name should
/// not contain any forward slashes (/)"): a name is one file's, not a path.
pub const UPLOAD_NAME_FORBIDDEN_SEPARATOR: char = '/';

/// Why `name` is not a usable file name for an upload of `upload_type`, or `None` when it
/// is.
///
/// Every sentence is the model's own documentation (see [`upload_name_rule`]); the model
/// fixes no message text for `ArgumentException`, so the documented sentence is what the
/// refusal states. The check lives here so `CreateUpload` and `UpdateUpload` — the two
/// operations that set a name — word it one way.
pub fn upload_name_refusal(name: &str, upload_type: &str) -> Option<String> {
    if name.contains(UPLOAD_NAME_FORBIDDEN_SEPARATOR) {
        return Some(format!(
            "The name should not contain any forward slashes ({UPLOAD_NAME_FORBIDDEN_SEPARATOR})"
        ));
    }
    let (extensions, sentence) = upload_name_rule(upload_type)?;
    if extensions.iter().any(|extension| name.ends_with(extension)) {
        return None;
    }
    Some(sentence.to_string())
}

/// The current wall-clock instant as a modelled timestamp, for the `created`-style
/// members a handler mints once and then stores.
///
/// Lives here, beside the other helpers more than one handler needs, because
/// `src/ops/mod.rs` is generated and cannot carry a hand-written module.
///
/// A clock before the Unix epoch yields the epoch rather than a panic: a handler must
/// never panic, and the emulator's own creation times are what this feeds.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}
