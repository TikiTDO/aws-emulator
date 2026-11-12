// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-b2bi-b2bi service.
//!
//! The service's primary resources — Profile, Capability, Partnership and Transformer —
//! are persisted here, under the single shared engine, so a create is observable by every
//! later Get/List/Delete. Each collection keeps BOTH a `HashMap` keyed by the resource's
//! generated id (the lookup every Get/Delete does) and a `Vec` of ids in insertion
//! order (the stable enumeration the `@paginated` List operations page through). The runs
//! `StartTransformerJob` performed live here too ([`TransformerJob`]), which is what
//! `GetTransformerJob` reports.
//!
//! It also owns the wording of the `ResourceNotFoundException` each resource type is
//! reported absent with, so every handler that resolves an id states it the same way; the
//! ONE way this service reaches an Amazon S3 object ([`read_edi_object`],
//! [`write_object`]); and the ONE authorization decision its operations take
//! ([`authorize`]), against the IAM identity policies of the principal that signed the
//! request.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime};

/// Whether a document is the file format a request declared it to be — the check the
/// test-harness operations (`TestConversion`, `TestMapping`) share.
///
/// Declared here, rather than beside `state` in `src/lib.rs`, because `lib.rs` is
/// generated and cannot take a hand-written `mod`.
#[path = "documents.rs"]
pub mod documents;

use crate::ops::types::{
    AccessDeniedException, CapabilityConfiguration, CapabilityOptions, EdiType, InputConversion,
    Mapping, OutputConversion, S3Location, SampleDocuments, Tag,
};

/// The region an emulated ARN carries when the request's SigV4 scope does not name one
/// (the emulator's documented default — see `engine::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Wording for the `InternalServerException` a handler answers when the engine it was
/// constructed from is not the shared `InMemoryEngine` (so no store is reachable).
pub const STATE_UNAVAILABLE: &str = "the b2bi service store is unavailable";

/// The value of the modelled `Logging` enum that turns a profile's logging on.
pub const LOGGING_ENABLED: &str = "ENABLED";

/// This service's own code, as the modelled `ServiceQuotaExceededException$serviceCode`
/// reports it and as Service Quotas publishes this service's quotas under
/// (`smithy_query(query="service_info")` -> `sdkId: b2bi`).
pub const SERVICE_CODE: &str = "b2bi";

/// The quota the profile count is charged against, as
/// `ServiceQuotaExceededException$quotaCode` names it and as the Service Quotas
/// emulator publishes it for [`SERVICE_CODE`].
pub const PROFILE_QUOTA_CODE: &str = "ProfilesPerAccount";

/// AWS's DOCUMENTED profile limit, from `CreateProfile`: "You can have up to five
/// customer profiles, each representing a distinct private network."
///
/// A create past it is the modelled `ServiceQuotaExceededException` (HTTP 402), and
/// deleting a profile releases a slot — so the limit is a live count of stored
/// profiles, never a high-water mark. AWS publishes this limit in Service Quotas
/// (ServiceCode `b2bi`) as adjustable; this emulator enforces the documented default,
/// and does not apply an approved increase to it.
pub const MAX_PROFILES_PER_ACCOUNT: usize = 5;

/// The `TransformerStatus` a freshly created transformer carries.
///
/// `inactive`: the model's own doc for `CreateTransformerResponse$status` says "Returns
/// the state of the newly created transformer. The transformer can be either active or
/// inactive. For the transformer to be used in a capability, its status must active" —
/// i.e. a new transformer is not usable until a caller moves it, so creation cannot be
/// what makes it `active`. The move is an explicit `UpdateTransformer`, not a transition
/// the clock owes: `inactive` is a resting state, not a transient one.
pub const TRANSFORMER_STATUS_INACTIVE: &str = "inactive";

/// The other value of the modelled `TransformerStatus` enum (`active` | `inactive`,
/// `smithy_query(query="shape",shape="TransformerStatus")`): the state a transformer has
/// to be in to be used — by a capability, and by a transformer job
/// (`StartTransformerJob`) — and the TERMINAL state of a transformer's own lifecycle: the
/// model documents the single transition `inactive` -> `active`
/// (`UpdateTransformerRequest$status`), so an active record takes no further update (see
/// `UpdateTransformer`).
pub const TRANSFORMER_STATUS_ACTIVE: &str = "active";

/// How many hex digits follow the resource-type prefix in a generated id.
///
/// 17, matching the `profileId` / `capabilityId` / `partnershipId` / `tradingPartnerId`
/// values in this service's own Smithy model examples (`p-<17 hex>`), and well inside
/// the modelled `len 1..64` / `^[a-zA-Z0-9_-]+$` constraint on every one of those ids.
const ID_HEX_DIGITS: usize = 17;

/// The page size a List operation uses when the caller names no `maxResults`: the top of
/// the modelled `MaxResults` range (`range 1..100`).
pub const MAX_PAGE_SIZE: usize = 100;

/// The most records a store this service keeps no AWS-published limit for holds — the
/// capability and partnership stores. The profile store's ceiling is AWS's own
/// [`MAX_PROFILES_PER_ACCOUNT`], which is far below this one.
///
/// Bounds the memory one emulator process can be made to hold through repeated creates:
/// a create is reachable without a verified caller, so without a ceiling every call
/// would add a resident record for good. Each create refuses past this bound with the
/// modelled `ServiceQuotaExceededException` rather than growing further, which also
/// bounds the per-request work the List operations do under the state lock (their filter
/// and token-resume scans walk at most this many ids).
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// How long the create an `@idempotencyToken` started reports as still IN FLIGHT.
///
/// AWS answers a duplicate of an in-flight request with `ConflictException` and replays
/// the original resource only once that request has completed (recorded for
/// `CreateTransformer` by trace `b2bi.create-transformer-idempotent-token-concurrent-dedup`:
/// of N concurrent creates sharing one token exactly one resource is minted, the losing
/// racers are refused with `ConflictException`, and a replay issued AFTER the original
/// request completed answers with the id it minted). A create is served here in
/// microseconds, so "while the first request runs" would be a window no concurrent client
/// could ever observe: the create a token started therefore reports in flight for this
/// span, and a caller racing the original sees that transient state.
///
/// The span is the one this emulator holds every transient state it owes the clock for, so
/// a caller acting the instant it has the first answer still observes it — a window shorter
/// than the gap between two of a client's own calls is no transient state at all, and the
/// refusal that belongs to it would be served or not depending on how loaded the host is.
/// The other half of that recording is reached by waiting it out: once the span has passed
/// the same token replays the resource it minted. Nothing sleeps for it — the due instant
/// is stored with the remembered token and compared when the next call reads it
/// ([`ComAmazonawsB2biB2biState::replay`]).
pub const IDEMPOTENT_CREATE_IN_FLIGHT: Duration = Duration::from_secs(3);

/// How long after `StartTransformerJob` a transformer job reports `running`.
///
/// The job's work is done by the call that started it, but AWS answers that call with a
/// job in the non-terminal `running` status and reveals the outcome to a later
/// `GetTransformerJob`; this is the span this emulator holds `running` for, so a caller
/// polling straight after the start observes the status AWS returns. It is never zero and
/// nothing sleeps for it: the due instant is stored on the job and compared when read.
pub const TRANSFORMER_JOB_RUNNING_FOR: Duration = Duration::from_millis(250);

/// The most `@idempotencyToken` replays remembered at once.
///
/// Each distinct caller-chosen `clientToken` adds one entry, so the map is capped and
/// the oldest entry is evicted first
/// ([`ComAmazonawsB2biB2biState::record_idempotency`]); deleting a resource also drops
/// the entries that pointed at it
/// ([`ComAmazonawsB2biB2biState::forget_idempotency_for`]), so the map has a release
/// path as well as a ceiling.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 10_000;

/// The most tags one resource holds at once.
///
/// 200, the maximum of the modelled `TagList` (`len 0..200`) — the very shape
/// `ListTagsForResourceResponse$Tags` reports a resource's tags through, so a resource
/// holding more than this could not be read back inside its own modelled bound. It is
/// also what bounds tag residency: `TagResource` is reachable without a verified caller,
/// so without a ceiling repeated calls with fresh keys would grow one resource's record
/// for good. A call that would carry a resource past it is refused with the modelled
/// `ValidationException`.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// A customer profile: the private network a partnership is created under.
#[derive(Debug, Clone)]
pub struct Profile {
    pub profile_id: String,
    pub profile_arn: String,
    pub name: String,
    pub email: Option<String>,
    pub phone: String,
    pub business_name: String,
    pub logging: String,
    pub log_group_name: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    /// Digest of the create request that minted this profile — see
    /// [`request_digest`] and [`Transformer::request_digest`].
    pub request_digest: String,
}

/// A capability: the EDI processing a partnership references by id.
#[derive(Debug, Clone)]
pub struct Capability {
    pub capability_id: String,
    pub capability_arn: String,
    pub name: String,
    pub r#type: String,
    pub configuration: CapabilityConfiguration,
    /// `Option`, not `Vec`: the modelled member is optional, so an absent
    /// `instructionsDocuments` must read back absent rather than as an empty list.
    pub instructions_documents: Option<Vec<S3Location>>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    /// Digest of the create request that minted this capability — see
    /// [`request_digest`] and [`Transformer::request_digest`].
    pub request_digest: String,
}

/// A transformer: the EDI <-> JSON/XML translation a capability references by id.
///
/// Every member is persisted as the create supplied it — including the four members the
/// model marks `@deprecated` (`fileFormat`, `mappingTemplate`, `ediType`,
/// `sampleDocument`) — so a later read cannot disagree with what was created.
#[derive(Debug, Clone)]
pub struct Transformer {
    pub transformer_id: String,
    pub transformer_arn: String,
    pub name: String,
    pub status: String,
    pub file_format: Option<String>,
    pub mapping_template: Option<String>,
    pub edi_type: Option<EdiType>,
    pub sample_document: Option<String>,
    pub input_conversion: Option<InputConversion>,
    pub mapping: Option<Mapping>,
    pub output_conversion: Option<OutputConversion>,
    pub sample_documents: Option<SampleDocuments>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    /// Digest of the create request that minted this transformer, so a replay of its
    /// `@idempotencyToken` can tell "the same request again" (answer with this record)
    /// from "a different request under a token already used" (the modelled conflict).
    /// Carried on the record rather than in a second map, so it cannot drift from the
    /// resource it describes.
    pub request_digest: String,
}

/// The `TransformerJobStatus` a job reports while the clock still owes its outcome:
/// `running`, one of the three values the model declares (`failed | running | succeeded`).
pub const TRANSFORMER_JOB_RUNNING: &str = "running";

/// The `TransformerJobStatus` of a run that produced its output file.
pub const TRANSFORMER_JOB_SUCCEEDED: &str = "succeeded";

/// The `TransformerJobStatus` of a run that could not: `GetTransformerJob$message`
/// carries the reason, which is what that member documents ("Returns an optional error
/// message, which gets populated when the job is not run successfully").
pub const TRANSFORMER_JOB_FAILED: &str = "failed";

/// The most transformer jobs this emulator remembers at once, oldest evicted first.
///
/// A job is reachable without a stored resource of its own, so without a ceiling every
/// `StartTransformerJob` would add a resident record for good. AWS keeps a job for 30 days
/// (`StartTransformerJob`: "The system stores transformer jobs for 30 days"), a span this
/// emulator's process lifetime never reaches, so the bound is a count rather than an age.
pub const MAX_TRANSFORMER_JOBS: usize = 1_000;

/// What one `StartTransformerJob` run did, as `GetTransformerJob` reports it.
///
/// The run itself is performed by the call that starts it — the input object is read, the
/// transformation evaluated and the output file written there — and its OUTCOME is what is
/// persisted here, beside the instant the job stops reporting `running`
/// ([`TRANSFORMER_JOB_RUNNING_FOR`]). Nothing about the record changes after that instant:
/// the reader only chooses which of the two states the stored job is in.
#[derive(Debug, Clone)]
pub struct TransformerJob {
    pub transformer_job_id: String,
    /// The transformer this job ran, so a `GetTransformerJob` naming a different
    /// `transformerId` does not resolve it.
    pub transformer_id: String,
    /// A digest of what the `StartTransformerJob` request that started this run asked for,
    /// so a repeat of its `@idempotencyToken` can tell the SAME request (a retry, answered
    /// with this job) from a different one (a token already used, refused with
    /// `ConflictException`) — the same test `CreateTransformer` applies to its own token.
    pub request_digest: String,
    /// The terminal status the run reached: [`TRANSFORMER_JOB_SUCCEEDED`] or
    /// [`TRANSFORMER_JOB_FAILED`].
    pub terminal_status: String,
    /// The output files the run wrote, reported once the job is no longer `running`.
    pub output_files: Vec<S3Location>,
    /// Why the run failed, for a run that did.
    pub message: Option<String>,
    /// When the job stops reporting `running` and reports its terminal status.
    pub settles_at: Instant,
}

impl TransformerJob {
    /// The status a read at `now` reports: `running` until the outcome is due, then the
    /// terminal status the run reached.
    pub fn status_at(&self, now: Instant) -> &str {
        if now < self.settles_at {
            TRANSFORMER_JOB_RUNNING
        } else {
            &self.terminal_status
        }
    }

    /// The output files a read at `now` reports: none while the job is still `running`,
    /// because a caller may not follow a path the job has not yet reported.
    pub fn output_files_at(&self, now: Instant) -> Option<Vec<S3Location>> {
        if now < self.settles_at || self.output_files.is_empty() {
            return None;
        }
        Some(self.output_files.clone())
    }

    /// The `message` a read at `now` reports: only a settled, failed run has one.
    pub fn message_at(&self, now: Instant) -> Option<String> {
        if now < self.settles_at {
            return None;
        }
        self.message.clone()
    }
}

/// What a remembered `@idempotencyToken` minted, and when that create settled.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The id of the resource the first call carrying this token created.
    pub resource_id: String,
    /// When the create stops reporting as in flight — see
    /// [`IDEMPOTENT_CREATE_IN_FLIGHT`].
    pub settles_at: Instant,
}

/// What a create carrying an already-remembered `@idempotencyToken` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenReplay {
    /// The create this token started has not settled: a duplicate is the racer that lost
    /// it, which AWS refuses with `ConflictException`.
    InFlight,
    /// The create settled, so a duplicate answers with the resource it minted.
    Completed(String),
}

/// A partnership: one trading partner, under a profile, bound to a set of capabilities.
#[derive(Debug, Clone)]
pub struct Partnership {
    pub partnership_id: String,
    pub partnership_arn: String,
    pub profile_id: String,
    pub name: String,
    pub email: String,
    pub phone: Option<String>,
    pub capabilities: Vec<String>,
    pub capability_options: Option<CapabilityOptions>,
    pub trading_partner_id: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: Option<::aws_smithy_types::DateTime>,
    pub tags: Vec<Tag>,
    /// Digest of the create request that minted this partnership — see
    /// [`request_digest`] and [`Transformer::request_digest`].
    pub request_digest: String,
}

/// Per-service state for com-amazonaws-b2bi-b2bi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsB2biB2biState {
    pub profiles: HashMap<String, Profile>,
    /// `profiles` in creation order, so `ListProfiles` paging is stable across calls.
    pub profile_order: Vec<String>,
    /// `profileId`s minted but not yet stored.
    ///
    /// `CreateProfile` provisions a logging-ENABLED profile's log group with no lock
    /// held, so it reserves its id here first: while the reservation stands no
    /// concurrent create can mint the same id, and the reservation counts against
    /// [`MAX_PROFILES_PER_ACCOUNT`] so it cannot take the account's last slot either.
    /// An entry lives for one in-flight create and is removed whichever way it ends.
    pub reserved_profile_ids: HashSet<String>,
    pub capabilities: HashMap<String, Capability>,
    /// `capabilities` in creation order, for `ListCapabilities` paging.
    pub capability_order: Vec<String>,
    pub partnerships: HashMap<String, Partnership>,
    /// `partnerships` in creation order, for `ListPartnerships` paging.
    pub partnership_order: Vec<String>,
    /// The transformers this service holds, keyed by `transformerId`: the set an `edi`
    /// capability's `transformerId` is resolved against. See [`Transformer`].
    pub transformers: HashMap<String, Transformer>,
    /// `transformers` in creation order, for `ListTransformers` paging.
    pub transformer_order: Vec<String>,
    /// The transformer runs `StartTransformerJob` performed, keyed by
    /// `transformerJobId`: the set `GetTransformerJob` resolves against. Bounded by
    /// [`MAX_TRANSFORMER_JOBS`]. See [`TransformerJob`].
    pub transformer_jobs: HashMap<String, TransformerJob>,
    /// `transformer_jobs` in start order, so the oldest job is the one evicted once the
    /// store is full.
    pub transformer_job_order: VecDeque<String>,
    /// `@idempotencyToken` replay: [`idempotency_key`] -> what the first call with that
    /// token created, and when that create settled. Bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`].
    pub idempotency: HashMap<String, IdempotentCreate>,
    /// The order [`idempotency`](Self::idempotency)'s keys were recorded in, so the
    /// oldest is the one evicted once the map is full.
    pub idempotency_order: VecDeque<String>,
}

impl ComAmazonawsB2biB2biState {
    /// Whether another profile fits inside AWS's documented
    /// [`MAX_PROFILES_PER_ACCOUNT`] limit, counting the ids in-flight creates have
    /// reserved ([`reserved_profile_ids`](Self::reserved_profile_ids)). That limit is far
    /// below [`MAX_RECORDS_PER_STORE`], so it is also what bounds this store's residency.
    pub fn accepts_another_profile(&self) -> bool {
        self.profiles.len() + self.reserved_profile_ids.len() < MAX_PROFILES_PER_ACCOUNT
    }

    /// Whether the capability store is below [`MAX_RECORDS_PER_STORE`].
    pub fn accepts_another_capability(&self) -> bool {
        self.capabilities.len() < MAX_RECORDS_PER_STORE
    }

    /// Whether the partnership store is below [`MAX_RECORDS_PER_STORE`].
    pub fn accepts_another_partnership(&self) -> bool {
        self.partnerships.len() < MAX_RECORDS_PER_STORE
    }

    /// Whether the transformer store is below [`MAX_RECORDS_PER_STORE`].
    pub fn accepts_another_transformer(&self) -> bool {
        self.transformers.len() < MAX_RECORDS_PER_STORE
    }

    /// The first id in `capability_ids` this store holds no capability for, in the order
    /// the caller listed them — or `None` when every one of them resolves.
    ///
    /// The ONE place a list of `capabilityId` references is resolved, so every operation
    /// that binds capabilities to a partnership (the create and the update) refuses the
    /// same unknown reference the same way. The walk is one hash lookup per element and
    /// stops at the first element that does not resolve, which is the id the refusal
    /// names.
    pub fn first_unknown_capability<'ids>(
        &self,
        capability_ids: impl IntoIterator<Item = &'ids str>,
    ) -> Option<&'ids str> {
        capability_ids
            .into_iter()
            .find(|capability_id| !self.capabilities.contains_key(*capability_id))
    }

    /// Remember that `key` minted `id`, evicting the oldest remembered token once
    /// [`MAX_IDEMPOTENCY_ENTRIES`] are held — so a caller cannot grow this map without
    /// limit by sending a fresh `clientToken` on every request.
    ///
    /// The record reports the create as in flight for [`IDEMPOTENT_CREATE_IN_FLIGHT`]
    /// from here, which is what [`replay`](Self::replay) reads.
    pub fn record_idempotency(&mut self, key: String, id: &str) {
        let record = IdempotentCreate {
            resource_id: id.to_string(),
            settles_at: Instant::now() + IDEMPOTENT_CREATE_IN_FLIGHT,
        };
        if self.idempotency.insert(key.clone(), record).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// What a create carrying the token `key` answers at `now`: nothing when the token is
    /// not remembered, [`TokenReplay::InFlight`] while the create it started is still in
    /// flight, and the id it minted once that create has settled.
    pub fn replay(&self, key: &str, now: Instant) -> Option<TokenReplay> {
        self.idempotency.get(key).map(|record| {
            if now < record.settles_at {
                TokenReplay::InFlight
            } else {
                TokenReplay::Completed(record.resource_id.clone())
            }
        })
    }

    /// Remember `job` as a run of its transformer, evicting the oldest remembered job
    /// once [`MAX_TRANSFORMER_JOBS`] are held.
    pub fn record_transformer_job(&mut self, job: TransformerJob) {
        let job_id = job.transformer_job_id.clone();
        if self.transformer_jobs.insert(job_id.clone(), job).is_none() {
            self.transformer_job_order.push_back(job_id);
        }
        while self.transformer_job_order.len() > MAX_TRANSFORMER_JOBS {
            if let Some(oldest) = self.transformer_job_order.pop_front() {
                self.transformer_jobs.remove(&oldest);
            }
        }
    }

    /// The tags stored on the resource `arn` names, or `None` when this service holds
    /// no resource with that ARN.
    ///
    /// Every taggable resource this service persists — profile, capability,
    /// partnership, transformer — keeps the tags it was created with on its own record,
    /// so the tag
    /// list is looked up by the very ARN the create stored and handed back. An ARN that
    /// differs from a stored one in any field (its region or its account, say) names no
    /// resource here, which is what AWS reports as `ResourceNotFoundException`: "the
    /// resource exists in a region other than the region specified in the API call".
    ///
    /// The scan is linear over at most four times [`MAX_RECORDS_PER_STORE`] records,
    /// the same bound the List operations' scans carry.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&Vec<Tag>> {
        self.profiles
            .values()
            .find(|profile| profile.profile_arn == arn)
            .map(|profile| &profile.tags)
            .or_else(|| {
                self.capabilities
                    .values()
                    .find(|capability| capability.capability_arn == arn)
                    .map(|capability| &capability.tags)
            })
            .or_else(|| {
                self.partnerships
                    .values()
                    .find(|partnership| partnership.partnership_arn == arn)
                    .map(|partnership| &partnership.tags)
            })
            .or_else(|| {
                self.transformers
                    .values()
                    .find(|transformer| transformer.transformer_arn == arn)
                    .map(|transformer| &transformer.tags)
            })
    }

    /// The tags stored on the resource `arn` names, for a handler that CHANGES them
    /// (`TagResource`), or `None` when this service holds no resource with that ARN.
    ///
    /// The same resolution as [`tags_by_arn`](Self::tags_by_arn) — the tag list lives on
    /// the resource's own record, found by the very ARN its create stored — so a tag
    /// written here is the one the next `ListTagsForResource` reports, and an ARN that
    /// differs from a stored one in any field names no resource here.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut Vec<Tag>> {
        if let Some(profile) = self
            .profiles
            .values_mut()
            .find(|profile| profile.profile_arn == arn)
        {
            return Some(&mut profile.tags);
        }
        if let Some(capability) = self
            .capabilities
            .values_mut()
            .find(|capability| capability.capability_arn == arn)
        {
            return Some(&mut capability.tags);
        }
        if let Some(partnership) = self
            .partnerships
            .values_mut()
            .find(|partnership| partnership.partnership_arn == arn)
        {
            return Some(&mut partnership.tags);
        }
        self.transformers
            .values_mut()
            .find(|transformer| transformer.transformer_arn == arn)
            .map(|transformer| &mut transformer.tags)
    }

    /// Drop every remembered token that minted `id`, called when `id`'s resource is
    /// deleted: a replay of that token mints a fresh resource anyway, so keeping the
    /// entry would hold memory for a resource that no longer exists.
    pub fn forget_idempotency_for(&mut self, id: &str) {
        self.idempotency.retain(|_, record| record.resource_id != id);
        let remembered = &self.idempotency;
        self.idempotency_order
            .retain(|key| remembered.contains_key(key));
    }
}

/// The `ConflictException` message for a create whose `clientToken` names a create that
/// has not settled yet: the caller is the racer that lost the in-flight race, and the
/// request it duplicated is the one that mints the resource.
///
/// ONE fixed sentence, the same for every create this service offers and carrying nothing
/// of the token or of the resource type: an `@idempotencyToken` is a value the CALLER
/// generated, so echoing it tells the caller nothing it does not already know, and a retry
/// wrapper that keys on this refusal matches the sentence rather than parsing it.
pub const CLIENT_TOKEN_IN_FLIGHT: &str =
    "This clientToken has been used for a previous request that is still being processed.";

/// The digest of what a create request asked for, as the `@idempotencyToken` replay check
/// compares it: `rendered` is the request's own members, rendered by the handler that owns
/// them (destructured there, so a member the model adds cannot be silently left out).
///
/// Hashed rather than kept verbatim because one request can carry hundreds of kilobytes (a
/// `mappingTemplate` is `len 0..350000`), so a record holds a fixed number of bytes
/// whatever the request did. A `Debug` rendering is enough precisely because a digest is
/// never compared with one from anywhere else: it is only ever compared with a digest THIS
/// process computed, over records in this process's own memory, so it has to distinguish
/// two requests rather than be stable across builds.
pub fn request_digest(rendered: &str) -> String {
    use ::sha2::Digest;

    let mut digest = ::sha2::Sha256::new();
    digest.update(rendered.as_bytes());
    ::hex::encode(digest.finalize())
}

/// The `ConflictException` message for a SETTLED `clientToken` used again for a DIFFERENT
/// request.
///
/// An `@idempotencyToken` makes a retry safe, so one token names ONE request: a second
/// request under a token this store already minted a resource for can neither be served as
/// a replay (the parameters differ) nor mint a second resource, which is what
/// `ConflictException` (HTTP 409) is modelled for.
pub fn token_already_used(resource_type: &str, id: &str) -> String {
    format!("this clientToken already created {resource_type} {id} with different parameters")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsB2biB2biState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsB2biB2biState>)
}

/// The `ResourceNotFoundException` message the two operations that address the PROFILE
/// RECORD itself report an absent `profileId` with — `GetProfile` and `DeleteProfile`.
///
/// Built from the request's own identifier, in the form recorded from AWS for those two
/// paths: a client parses the id back out of the message, so the template is part of the
/// contract and must not be an emulator-local phrase. The service states a DIFFERENT
/// template on every other path that resolves a `profileId`
/// ([`profile_reference_not_found`]) — two code paths of the real service, each recorded
/// with its own wording, so the template is chosen per path rather than per service.
///
/// Source: one recorded unit (`b2bi.get-profile-not-found`) carries BOTH forms — an
/// earlier beat reads an unknown profile through `GetProfile` and requires this template,
/// a later one mutates a deleted profile through `UpdateProfile` and requires the other.
pub fn profile_not_found(profile_id: &str) -> String {
    format!("Profile ID {profile_id} not found.")
}

/// The `ResourceNotFoundException` message every OTHER path that resolves a `profileId`
/// reports it absent with — `UpdateProfile`, which resolves the record before writing to
/// it, `CreatePartnership`, which resolves the profile its partnership is created under,
/// and `ListPartnerships`, whose page is narrowed to it.
///
/// The recorded form for those paths names the MEMBER the id arrived in, so it is a
/// different template from [`profile_not_found`] — the same per-path split a transformer's
/// two forms carry ([`transformer_not_found`] and [`transformer_reference_not_found`]) and
/// a capability's ([`capability_not_found`] and [`capability_update_not_found`]).
pub fn profile_reference_not_found(profile_id: &str) -> String {
    format!("Profile with profileId {profile_id} not found.")
}

/// The `ResourceNotFoundException` message for a `capabilityId` the reader, the delete and
/// the operations that resolve a capability as a REFERENCE report absent —
/// `GetCapability`, `DeleteCapability`, `CreatePartnership`, `UpdatePartnership`.
pub fn capability_not_found(capability_id: &str) -> String {
    format!("Capability ID {capability_id} not found.")
}

/// The `ResourceNotFoundException` message `UpdateCapability` reports an unknown
/// `capabilityId` with.
///
/// The recorded form for that path names the MEMBER the id arrived in, unlike the template
/// every other capability path states ([`capability_not_found`]) — the same per-path split
/// a transformer's two forms carry ([`transformer_not_found`] and
/// [`transformer_reference_not_found`]), and the same wording the partnership paths use
/// ([`partnership_not_found`]).
pub fn capability_update_not_found(capability_id: &str) -> String {
    format!("Capability with capabilityId {capability_id} not found.")
}

/// The `ResourceNotFoundException` message for an unknown `partnershipId`.
pub fn partnership_not_found(partnership_id: &str) -> String {
    format!("Partnership with partnershipId {partnership_id} not found.")
}

/// The `ResourceNotFoundException` message for a `transformerId` an operation resolves as
/// its OWN identifier — `GetTransformer`, `UpdateTransformer`, `DeleteTransformer`,
/// `StartTransformerJob`.
///
/// The recorded form for that path states the fixed phrase first and the id last, unlike
/// the three above. A transformer has TWO recorded templates, one per path an id arrives
/// on; the one for a reference another resource holds is
/// [`transformer_reference_not_found`].
pub fn transformer_not_found(transformer_id: &str) -> String {
    format!("Transformer not found with ID {transformer_id}")
}

/// The `ResourceNotFoundException` message for a `transformerId` ANOTHER resource's
/// configuration references and this store holds no transformer for — the reference
/// `CreateCapability` resolves before it mints anything.
///
/// The recorded form for that path names the member the reference arrived in, so it is a
/// different template from [`transformer_not_found`].
pub fn transformer_reference_not_found(transformer_id: &str) -> String {
    format!("Transformer with transformerId {transformer_id} not found")
}

/// The `ResourceNotFoundException` message for a `transformerJobId` this service holds no
/// run for — including a job AWS has already dropped, which it does 30 days after the run
/// ("if 30 days have elapsed since your transformer job was started, the system deletes it
/// … you receive a 404 response", `GetTransformerJob`).
///
/// The recorded form words the refusal around the JOB and the CUSTOMER the job would have
/// belonged to — a job id is unique to an account, so the account is what scopes the
/// lookup — and says nothing of the transformer the request paired it with.
pub fn transformer_job_not_found(transformer_job_id: &str, account_id: &str) -> String {
    format!("Found no transformer job {transformer_job_id} for customer {account_id}")
}

/// The CloudWatch Logs log group a logging-ENABLED profile reports as `logGroupName`,
/// in the `b2bi/<profileId>-Logs` form this service's own model example carries.
///
/// The group with this name is PROVISIONED in CloudWatch Logs when the profile is
/// created (`CreateProfile`), so the name a caller reads back resolves there.
pub fn log_group_name(profile_id: &str) -> String {
    format!("b2bi/{profile_id}-Logs")
}

/// The key a `clientToken` is remembered under, scoped by resource type.
///
/// Scoped because the three creates each carry their own `@idempotencyToken` and a
/// caller may reuse one token value across them; an unscoped map would hand
/// `CreatePartnership` the profile that `CreateProfile` created under the same token.
pub fn idempotency_key(resource_type: &str, client_token: &str) -> String {
    format!("{resource_type}#{client_token}")
}

/// Whether `candidate` satisfies the AWS ARN grammar
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`: six colon-delimited
/// fields, the first literally `arn`, with a non-empty partition, service and resource.
/// `region` and `account-id` are the two fields AWS itself leaves empty on the ARNs of
/// global services, so neither is required to be filled in; the resource field may itself
/// contain colons and slashes, so it is whatever follows the fifth colon.
///
/// A SYNTAX test, which the tagging operations ask BEFORE they resolve anything, because
/// the two failures are worded differently: a value that is not an ARN at all is reported
/// with the value ([`invalid_arn`]), while an ARN that parses and addresses no resource of
/// this account is the fixed sentence ([`INVALID_INPUT_ARN`]). Both are the modelled
/// `ValidationException`.
pub fn is_arn(candidate: &str) -> bool {
    let fields: Vec<&str> = candidate.splitn(6, ':').collect();
    fields.len() == 6
        && fields[0] == "arn"
        && !fields[1].is_empty()
        && !fields[2].is_empty()
        && !fields[5].is_empty()
}

/// The `ValidationException` message for a `ResourceARN` that does not satisfy the ARN
/// grammar: the recorded form names the value that was rejected, which is what tells the
/// caller WHICH of its inputs is not an ARN.
pub fn invalid_arn(candidate: &str) -> String {
    format!("Invalid ARN {candidate}")
}

/// The `ValidationException` message every tagging operation refuses a `ResourceARN` that
/// PARSES as an ARN and addresses no b2bi resource of this account with.
///
/// ONE fixed sentence, carrying nothing of the value it rejected — the recorded form. A
/// well-formed ARN is resolved rather than merely parsed, and one that resolves to nothing
/// is an invalid input ARN rather than a resource reported missing; a value that is not an
/// ARN at all never reaches this point and is reported by [`invalid_arn`] instead.
pub const INVALID_INPUT_ARN: &str = "Invalid input ARN!";

/// The `ValidationException` message a capability whose configuration names an Amazon S3
/// bucket this account does not own is refused with.
///
/// Fixed text: it names neither the bucket nor which of the two locations failed, so a
/// caller learns that the input and/or the output bucket is not theirs and nothing more.
pub const BUCKETS_NOT_OWNED: &str = "Customer does not own input and/or output buckets.";

/// The `ResourceNotFoundException` message for an Amazon S3 object a request names and
/// object storage does not hold.
///
/// States the `s3://<bucket>/<key>` URI the request addressed and that it is not there,
/// and NOTHING of the error code S3 answered with: the dependency's own code is not part
/// of what this service tells its caller.
pub fn s3_object_not_found(bucket: &str, key: &str) -> String {
    format!("The S3 object s3://{bucket}/{key} does not exist")
}

/// The current wall-clock instant, as the modelled `CreatedDate`/`ModifiedDate` timestamp.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
}

/// The two timestamps a resource record is STORED with, as `(createdAt, modifiedAt)`.
///
/// `modifiedAt` reports when the record was last written, and creating it IS a write: a
/// resource nobody has updated since reports a `modifiedAt` equal to its `createdAt`
/// rather than no `modifiedAt` at all. One [`now`] for both, so the pair cannot disagree
/// by the time the call takes.
pub fn created_timestamps() -> (
    ::aws_smithy_types::DateTime,
    Option<::aws_smithy_types::DateTime>,
) {
    let at = now();
    (at, Some(at))
}

/// The ARN a request's own identifier names, for authorization.
///
/// Built from the REQUEST rather than from the store, because a request is authorized
/// before its identifier is resolved: a caller with no permission on a resource learns
/// that it is not authorized, not whether that resource exists.
pub fn resource_arn(region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_CODE, region, &format!("{kind}/{id}"))
}

/// The ARN of the account's whole collection of one resource type in `region`, which is
/// what a request addressing the collection rather than one member of it is authorized
/// against: a create (it names no resource until it mints one) and a List (it names none
/// at all) act on every resource of that type the account holds, spelled with IAM's
/// wildcard id — the same form a policy scoping a statement to every resource of the type
/// is written with.
pub fn collection_arn(region: &str, kind: &str) -> String {
    resource_arn(region, kind, ANY_RESOURCE)
}

/// The `ServiceQuotaExceededException` message for a create refused because the account
/// has reached its quota of `resource_type` resources (capitalised as the sentence names
/// it: `Profile`, `Capability`, `Partnership`, `Transformer`).
///
/// The only variable part is the account the quota belongs to. The numeric limit is NOT in
/// the sentence: a quota is adjustable per account, so what the limit currently is travels
/// in the exception's own `serviceCode`/`quotaCode` members, which name the quota a caller
/// reads its value (and its adjusted value) from.
pub fn quota_reached(resource_type: &str) -> String {
    format!(
        "{resource_type} limit reached for AWS account {}",
        ::engine::emulator_account_id()
    )
}

/// The tag-key prefix AWS reserves for its own tags across every service: a caller may
/// not create or edit a key that starts with it, in any casing.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` claims the reserved prefix. Compared case-INSENSITIVELY, as AWS reserves
/// the namespace rather than the spelling.
///
/// The comparison is over BYTES: a tag key is modelled `len 1..128` with no pattern, so it
/// can be any UTF-8 text, and slicing such a string by a byte offset would panic when that
/// offset falls inside a multi-byte character. Taking the bytes answers `false` for those
/// keys instead, which is correct — a key whose first four bytes are not `aws:` does not
/// claim the namespace.
fn claims_reserved_prefix(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Why a tag write was refused, with nothing written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagWriteRefused {
    /// The write would leave the resource holding this many tags, above
    /// [`MAX_TAGS_PER_RESOURCE`].
    TooManyTags(usize),
    /// A key in the request starts with [`RESERVED_TAG_KEY_PREFIX`].
    ReservedKeyPrefix,
}

/// Merge `incoming` into `held`: a key already present takes the incoming value, a key
/// that is new is appended, and every other held tag is left where it is.
///
/// The AWS tagging contract for every resource type — `TagResource` "attaches a
/// key-value pair to a resource", so tagging an already-tagged resource updates that
/// key rather than duplicating it or dropping the others.
///
/// The two conditions that make a write impossible are checked HERE, before anything is
/// written, so no caller of this helper can grow a resource's tag set past what the
/// model allows itself to report back, and `held` is left exactly as it was on a
/// refusal. Each tag's own key and value lengths are modelled (`TagKey len 1..128`,
/// `TagValue len 0..256`) and so are already enforced by the boundary validator; the
/// tags a CREATE call supplies are stored by that create handler and bounded by the
/// same modelled list length.
pub fn merge_tags(held: &mut Vec<Tag>, incoming: Vec<Tag>) -> Result<(), TagWriteRefused> {
    if incoming.iter().any(|tag| claims_reserved_prefix(&tag.key)) {
        return Err(TagWriteRefused::ReservedKeyPrefix);
    }

    // The keys that are genuinely NEW: a key already held is updated in place, and a key
    // repeated inside one request is one key.
    let mut added: Vec<&str> = Vec::new();
    for tag in &incoming {
        if !held.iter().any(|existing| existing.key == tag.key)
            && !added.contains(&tag.key.as_str())
        {
            added.push(tag.key.as_str());
        }
    }
    let would_hold = held.len() + added.len();
    if would_hold > MAX_TAGS_PER_RESOURCE {
        return Err(TagWriteRefused::TooManyTags(would_hold));
    }

    for tag in incoming {
        match held.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => held.push(tag),
        }
    }
    Ok(())
}

/// Whether `region` is shaped like an AWS region code (`us-east-1`, `ap-southeast-4`):
/// lower-case letters, digits and hyphens, and short enough that an ARN carrying it
/// stays inside the modelled `ResourceArn len 1..255`.
fn is_region_shaped(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The region this request was signed for, for the ARNs the response carries.
///
/// The value comes from the caller's own SigV4 credential scope, so it is checked
/// against the region grammar before it is interpolated into an ARN: a scope naming
/// something that is not a region (or nothing at all) falls back to [`DEFAULT_REGION`]
/// rather than letting caller-chosen text reshape the ARN's colon-delimited fields.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh, system-generated resource id of the form `<prefix>-<17 hex digits>`.
///
/// Unpredictable (a per-process-seeded hasher over a process-wide sequence number and
/// the clock) so ids are not guessable, and `taken` is consulted by the caller to
/// guarantee the id is not already in use.
pub fn new_resource_id(prefix: &str) -> String {
    format!("{prefix}-{}", random_hex(ID_HEX_DIGITS))
}

/// How many characters a `transformerJobId` carries: the modelled `TransformerJobId` is
/// `len 25..25`, `pattern ^[a-zA-Z0-9_-]+$`, so a job id is exactly 25 characters and hex
/// digits satisfy the pattern.
const TRANSFORMER_JOB_ID_DIGITS: usize = 25;

/// A fresh, system-generated `transformerJobId`.
///
/// Unpredictable for the same reason a resource id is, and exactly
/// [`TRANSFORMER_JOB_ID_DIGITS`] characters long, which is what the model requires of
/// this id.
pub fn new_transformer_job_id() -> String {
    random_hex(TRANSFORMER_JOB_ID_DIGITS)
}

/// `count` unpredictable lowercase hex digits: a per-process-seeded hasher over a
/// process-wide sequence number and the clock, so two ids minted in one clock tick still
/// differ and neither is guessable.
///
/// Two 64-bit digests are what it has to draw from, so [`MAX_RANDOM_HEX_DIGITS`] is the
/// most it can answer with. A caller asking for more is a programming error — it would get
/// a shorter id than the length its model requires — so it is asserted rather than
/// silently truncated.
fn random_hex(count: usize) -> String {
    assert!(
        count <= MAX_RANDOM_HEX_DIGITS,
        "random_hex can answer with at most {MAX_RANDOM_HEX_DIGITS} digits, asked for {count}"
    );
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Distinguishes two ids minted inside one clock tick.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    /// Odd 64-bit constant (the golden-ratio mix used by `hashbrown`) so the low and
    /// high halves of the digest see different inputs.
    const MIX: u64 = 0x9e37_79b9_7f4a_7c15;

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();

    let seed = ::std::collections::hash_map::RandomState::new();
    let mut high = seed.build_hasher();
    high.write_u64(sequence);
    high.write_u64(nanos);
    let mut low = seed.build_hasher();
    low.write_u64(nanos);
    low.write_u64(sequence.wrapping_mul(MIX));

    let digits = format!("{:016x}{:016x}", high.finish(), low.finish());
    digits[..count].to_string()
}

/// The most digits [`random_hex`] can answer with: the two 64-bit digests it draws from,
/// as hex.
const MAX_RANDOM_HEX_DIGITS: usize = 32;

/// Why a `nextToken` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// The `ValidationException` message every paginated operation of this service refuses an
/// unusable `nextToken` with.
///
/// Fixed text: it names neither the member nor the token the caller sent, so a client that
/// tells an expired or garbage cursor apart from another `ValidationException` by the
/// message reads the same sentence from every one of them. Declared here, beside
/// [`paginate`], so the paginated operations cannot drift apart in how they word it.
pub const INVALID_PAGE_TOKEN_MESSAGE: &str = "Invalid pagination token";

/// The opaque `nextToken` that resumes an enumeration at `id`.
fn encode_page_token(id: &str) -> String {
    ::hex::encode(id)
}

/// One page of `ids`, plus the token that resumes after it.
///
/// `next_token` is the token a previous page returned; `max_results` is the caller's
/// page size (already held to the modelled `1..100` range by the boundary validator).
/// A token that does not decode is refused; one that decodes to an id no longer in
/// `ids` (its resource was deleted between pages) ends the enumeration with an empty
/// page, rather than refusing a token this service really did issue.
pub fn paginate(
    ids: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<String>, Option<String>), InvalidPageToken> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let resume_at = ::hex::decode(token)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or(InvalidPageToken)?;
            match ids.iter().position(|id| *id == resume_at) {
                Some(index) => index,
                None => return Ok((Vec::new(), None)),
            }
        }
    };

    let page_size = max_results
        .map(|requested| (requested.max(1) as usize).min(MAX_PAGE_SIZE))
        .unwrap_or(MAX_PAGE_SIZE);
    let end = start.saturating_add(page_size).min(ids.len());
    let page = ids[start..end].to_vec();
    let next = ids.get(end).map(|id| encode_page_token(id));
    Ok((page, next))
}

// ---------------------------------------------------------------------------
// Amazon S3: where every EDI document this service reads and writes lives.
// ---------------------------------------------------------------------------

/// The most bytes this service reads out of ONE Amazon S3 object.
///
/// AWS's own bound for the EDI file the test operations take: "The input file has a file
/// size limit of 250 KB" (`TestParsing` / `TestConversion` documentation). It also bounds
/// the buffer one request can make this emulator hold.
pub const MAX_EDI_OBJECT_BYTES: usize = 250 * 1024;

/// An Amazon S3 client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and this crate names no
/// other service's crate. `region` is the request's own signing region, so an object is
/// read and written where the caller's own S3 requests go.
pub fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so every request addresses
        // its bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly. (Same configuration as `CreateProfile`'s CloudWatch Logs client.)
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether the account this emulator serves owns the Amazon S3 bucket `bucket`.
///
/// Asked of the emulator's own S3 service, which serves ONE account and therefore holds
/// exactly the buckets that account owns: a bucket S3 reports it does not hold is a bucket
/// the caller does not own. An empty name owns nothing and is not asked about.
///
/// Any failure that is NOT S3 reporting the bucket absent (the S3 service not mounted, a
/// transport fault) answers `true`: a refusal has to be grounded in S3 saying the bucket
/// is not there, never in this emulator failing to ask. That distinction is why this reads
/// `HeadBucket`'s own modelled `NotFound` rather than going through [`bucket_region`],
/// which asks the other question of the same client — WHICH Region holds the bucket — and
/// collapses every non-Region answer into `None`.
pub async fn owns_bucket(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> bool {
    if bucket.is_empty() {
        return false;
    }
    match s3_client(ctx, region)
        .head_bucket()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(_) => true,
        Err(error) => {
            let absent = error
                .as_service_error()
                .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found);
            if !absent {
                ::tracing::warn!(
                    %bucket,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the bucket could not be resolved against Amazon S3"
                );
            }
            !absent
        }
    }
}

/// The Region an Amazon S3 bucket is in when its location carries no constraint:
/// "Buckets in Region us-east-1 have a LocationConstraint of null"
/// (`GetBucketLocationOutput$LocationConstraint`), so an absent constraint names that one
/// Region rather than "anywhere".
pub const UNCONSTRAINED_BUCKET_REGION: &str = "us-east-1";

/// The Region the bucket `bucket` is in, as Amazon S3 reports it to a caller signing for
/// `region` — or `None` when no bucket of that name is reachable there.
///
/// A bucket lives in exactly ONE Region, and `GetBucketLocation` is where S3 states which
/// (an absent `LocationConstraint` being [`UNCONSTRAINED_BUCKET_REGION`]). `None` covers
/// every answer that is not a Region — a bucket this account does not hold, or one S3 will
/// not locate for this caller — because none of those put the bucket in the caller's own
/// Region either.
pub async fn bucket_region(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
) -> Option<String> {
    let located = s3_client(ctx, region)
        .get_bucket_location()
        .bucket(bucket)
        .send()
        .await
        .ok()?;
    let constraint = located
        .location_constraint()
        .map(|constraint| constraint.as_str().to_string())
        .filter(|constraint| !constraint.is_empty());
    Some(constraint.unwrap_or_else(|| UNCONSTRAINED_BUCKET_REGION.to_string()))
}

/// Why an `S3Location` could not be read, split the way the modelled errors are: a
/// location that is not a location at all, or an object this service will not read, is the
/// caller's mistake (`ValidationException`), an object that is not there is the
/// `ResourceNotFoundException` every one of these operations models, and a call that failed
/// is a fault of this service (500).
#[derive(Debug, Clone)]
pub enum ObjectError {
    /// The location names no bucket or no key, so it addresses no object.
    Incomplete(String),
    /// Nothing is stored under that bucket and key.
    NotFound(String),
    /// The object is there, and is not one this service reads: past the EDI file size
    /// limit, or not text at all.
    Rejected(String),
    /// S3 was asked and the call itself failed.
    Unreadable(String),
}

impl ObjectError {
    /// What a refusal built from this failure says.
    pub fn message(&self) -> &str {
        match self {
            Self::Incomplete(message)
            | Self::NotFound(message)
            | Self::Rejected(message)
            | Self::Unreadable(message) => message,
        }
    }
}

/// The bucket and key an `S3Location` addresses, or why it addresses nothing.
pub fn object_path(location: &S3Location, member: &str) -> Result<(String, String), ObjectError> {
    let bucket = location.bucket_name.as_deref().unwrap_or_default();
    let key = location.key.as_deref().unwrap_or_default();
    if bucket.is_empty() || key.is_empty() {
        return Err(ObjectError::Incomplete(format!(
            "{member} must name both an Amazon S3 bucketName and a key"
        )));
    }
    Ok((bucket.to_string(), key.to_string()))
}

/// Read the object `location` names, as text, through the emulator's own S3 service.
///
/// Refuses an object past [`MAX_EDI_OBJECT_BYTES`] and one whose bytes are not UTF-8
/// text: an EDI interchange is text, so bytes that are not text hold no interchange.
pub async fn read_edi_object(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
    member: &str,
) -> Result<String, ObjectError> {
    let (bucket, key) = object_path(location, member)?;
    let object = s3_client(ctx, region)
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .send()
        .await
        .map_err(|error| {
            use ::aws_sdk_s3::error::ProvideErrorMetadata;
            let detail = ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string();
            match error.code() {
                // A name S3 can address no bucket by (`InvalidBucketName`) holds no object
                // either, so it is the same verdict as a bucket or key that is not there.
                Some("NoSuchKey" | "NoSuchBucket" | "NotFound" | "InvalidBucketName") => {
                    ObjectError::NotFound(s3_object_not_found(&bucket, &key))
                }
                _ => ObjectError::Unreadable(format!(
                    "the object at s3://{bucket}/{key} could not be read: {detail}"
                )),
            }
        })?;
    // The clamp is applied to the size S3 REPORTS, before a byte of the object is
    // buffered, so an oversized object costs no allocation at all; the check after the
    // body is read is the backstop for an object whose reported size was wrong.
    if object
        .content_length()
        .is_some_and(|length| length > MAX_EDI_OBJECT_BYTES as i64)
    {
        return Err(ObjectError::Rejected(format!(
            "the object at s3://{bucket}/{key} is {} bytes, more than the \
             {MAX_EDI_OBJECT_BYTES}-byte limit on an EDI file",
            object.content_length().unwrap_or_default()
        )));
    }
    let body = object.body.collect().await.map_err(|error| {
        ObjectError::Unreadable(format!(
            "the object at s3://{bucket}/{key} could not be read: {error}"
        ))
    })?;
    let bytes = body.into_bytes();
    if bytes.len() > MAX_EDI_OBJECT_BYTES {
        return Err(ObjectError::Rejected(format!(
            "the object at s3://{bucket}/{key} is {} bytes, more than the \
             {MAX_EDI_OBJECT_BYTES}-byte limit on an EDI file",
            bytes.len()
        )));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        ObjectError::Rejected(format!(
            "the object at s3://{bucket}/{key} is not UTF-8 text, so it holds no EDI \
             interchange"
        ))
    })
}

/// Write `body` to `bucket`/`key` through the emulator's own S3 service, answering the
/// reason it could not be written.
pub async fn write_object(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
    content_type: &str,
    body: String,
) -> Result<(), String> {
    s3_client(ctx, region)
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            body.into_bytes(),
        ))
        .send()
        .await
        .map(drop)
        .map_err(|error| {
            format!(
                "the output file s3://{bucket}/{key} could not be written: {}",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })
}

// ---------------------------------------------------------------------------
// Authorization: the IAM identity policies of the principal that signed the request.
// ---------------------------------------------------------------------------

/// The service prefix every one of this service's IAM actions carries
/// (`b2bi:CreateProfile`): its `arnNamespace` / `sigv4Name`
/// (`smithy_query(query="service_info")` -> `arnNamespace: b2bi`), which is the same
/// string as [`SERVICE_CODE`].
pub const IAM_ACTION_PREFIX: &str = SERVICE_CODE;

/// The wildcard id IAM writes "every resource" with, which this service uses two ways:
/// as the id of a whole collection of one resource type ([`collection_arn`]), and as the
/// resource an operation bound to no resource type at all is authorized against — a dry
/// run over content the request carries (`TestMapping`, `TestParsing`, `TestConversion`,
/// `GenerateMapping`, `CreateStarterMappingTemplate`), which touches nothing the account
/// holds and so has no ARN to name.
pub const ANY_RESOURCE: &str = "*";

/// The STS session store in the SAME shared engine, which is where the identity an access
/// key id speaks for lives.
///
/// This is the STS service's OWN accessor, called rather than copied, so b2bi resolves the
/// caller from the very store `sts:AssumeRole` writes and `sts:GetCallerIdentity` reads.
/// `None` means the engine carries no such store — a router that mounts b2bi without STS,
/// where no session can have been minted at all.
pub fn sts_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::ComAmazonawsStsAwssecuritytokenservicev20110615State>>
{
    ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::service_state(engine)
}

/// The IAM store in the SAME shared engine, which is where the account's roles and their
/// identity policies live. The IAM service's OWN accessor, for the same reason
/// [`sts_state`] is.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State>>
{
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// Authorize this request against the IAM identity policies of the principal the signing
/// credentials name, as the service prefix [`IAM_ACTION_PREFIX`] and `operation` name the
/// action and `resource` names what it acts on: the ARN of the resource the request
/// addresses, the [`collection_arn`] of the resource type a create or a List acts on, or
/// [`ANY_RESOURCE`] for an action bound to no resource type at all.
///
/// The DECISION is [`::service_common::iam_authorization`], the tree's one evaluation of
/// IAM policy documents against a request, so b2bi and every other service handed the same
/// document answer alike: an explicit `Deny` wins, otherwise an `Allow` about the request
/// grants it, and with neither the request is refused by IAM's default deny. What this
/// function owns is the two things only this service can collect — WHO the request speaks
/// for (the STS session the access key id names) and WHICH documents are in force on it
/// (the role's inline policies and the default versions of its attached managed policies,
/// read out of the IAM store, so adding a `Deny` to a role takes effect on the next
/// request and removing it lifts it again).
///
/// Four callers are served rather than evaluated, because there is no identity policy that
/// bounds them:
///
/// * a request whose access key id names no STS session — the ambient identity, the
///   long-term key this emulator publishes at start, which is the account's own
///   administrator (`sts_state::ambient_identity`), the position account root credentials
///   are in;
/// * a session that is not an assumed role (an `sts:GetSessionToken` or
///   `sts:GetFederationToken` session, or an IAM user's own long-term key): what bounds
///   such a caller is the user's own identity policies or the inline session policy the
///   mint supplied, and THIS SERVICE DOES NOT EVALUATE EITHER — a `Deny` on an IAM user's
///   inline policy therefore has no effect here. Stated rather than hidden: the recorded
///   boundary this issue answers is an assumed-role session's, and widening the decision
///   to a user principal belongs with the shared collection step
///   `services/service-com-amazonaws-rum-rum/src/state/authorization.rs` already performs
///   for both principal forms;
/// * a router that mounts b2bi without the STS or IAM service, where no session and no role
///   can exist at all;
/// * a role carrying an AWS-MANAGED policy, whose document the account does not hold
///   (`iam_store::AWS_MANAGED_POLICY_ACCOUNT`): this emulator cannot read what that policy
///   grants, and refusing on evidence it does not have would deny access AWS permits. An
///   explicit `Deny` in a document it CAN read still refuses, and an attachment naming a
///   CUSTOMER-managed policy the account does not hold grants nothing, because the account
///   is the authority on its own policies. (The same reading
///   `services/service-com-amazonaws-rum-rum/src/state/authorization.rs` gives it.)
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let Some(sts) = sts_state(engine) else {
        return Ok(());
    };
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = {
        let mut sessions = sts.lock();
        ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state::resolve_caller(
            &mut sessions,
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        )
    };
    // An expired session is refused by the layer that verifies the signature, not here.
    let Ok(identity) = identity else {
        return Ok(());
    };
    let Some(role_name) = assumed_role_name(&identity.arn) else {
        return Ok(());
    };
    let Some(iam) = iam_state(engine) else {
        return Ok(());
    };
    let policies = role_identity_policies(&iam.lock(), role_name);

    let action = format!("{IAM_ACTION_PREFIX}:{operation}");
    // Identity-based documents name no principal, so the principal spellings a
    // resource-based policy would be matched against are not part of this decision.
    let request = ::service_common::iam_authorization::Request::new(&action, resource, &[]);
    let decided =
        ::service_common::iam_authorization::identity_decision(&policies.documents, &request);
    match decided {
        ::service_common::iam_authorization::Decision::Allowed => Ok(()),
        // An explicit `Deny` in a document this emulator CAN read refuses the request
        // whatever else the role carries, which is why it is answered before the
        // unreadable attachment below.
        ::service_common::iam_authorization::Decision::Denied => Err(access_denied(
            &identity.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        )),
        // Nothing the store holds grants the action — but a grant it cannot READ may: see
        // this function's docs.
        ::service_common::iam_authorization::Decision::NotStated
            if policies.unreadable_attachment =>
        {
            Ok(())
        }
        ::service_common::iam_authorization::Decision::NotStated => Err(access_denied(
            &identity.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        )),
    }
}

/// The modelled `AccessDeniedException` for a refused request, worded the way IAM words an
/// authorization failure: the principal, the action, the resource it was asked for and
/// which of IAM's two rules refused it.
///
/// The resource clause names whatever the request was judged against — the ARN of the
/// resource it addresses, or the wildcard ARN of the collection a create or a List acts on
/// ([`collection_arn`]), which is what tells a caller the SCOPE the deny applied to. Only
/// an action bound to no resource type at all ([`ANY_RESOURCE`]) states no clause: there
/// is nothing to name.
fn access_denied(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    let on_resource = if resource == ANY_RESOURCE {
        String::new()
    } else {
        format!(" on resource: {resource}")
    };
    AccessDeniedException {
        message: format!("User: {principal} is not authorized to perform: {action}{on_resource} {reason}"),
    }
}

/// The role name an assumed-role session's ARN names
/// (`arn:aws:sts::<account>:assumed-role/<RoleName>/<SessionName>`), or `None` for every
/// other principal form.
pub fn assumed_role_name(identity_arn: &str) -> Option<&str> {
    let resource = identity_arn.splitn(6, ':').nth(5)?;
    let path = resource.strip_prefix("assumed-role/")?;
    let role_name = path.split('/').next()?;
    (!role_name.is_empty()).then_some(role_name)
}

/// The identity policies in force on one role.
#[derive(Debug, Default)]
pub struct RolePolicies {
    /// Every document the IAM store holds for the role: its inline policies and the
    /// DEFAULT version of each managed policy attached to it — the two kinds of identity
    /// policy IAM evaluates for a role.
    pub documents: Vec<String>,
    /// Set when the role carries an AWS-MANAGED policy, whose document the account does
    /// not hold, so what it grants cannot be read here. See [`authorize`].
    pub unreadable_attachment: bool,
}

/// The identity policies attached to the role `role_name`.
///
/// An unknown role holds no policy, which IAM's default deny refuses.
fn role_identity_policies(
    iam: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
    role_name: &str,
) -> RolePolicies {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

    let role_key = iam_store::name_key(role_name);
    let mut policies = RolePolicies {
        documents: iam
            .role_inline_policies
            .get(&role_key)
            .into_iter()
            .flat_map(|held| held.values())
            .map(|policy| policy.policy_document.clone())
            .collect(),
        unreadable_attachment: false,
    };
    for arn in iam
        .role_attached_policies
        .get(&role_key)
        .into_iter()
        .flatten()
    {
        let document = iam.policy_by_arn(arn).and_then(|managed| {
            managed
                .policy
                .default_version_id
                .as_deref()
                .and_then(|version_id| managed.versions.get(version_id))
                .or_else(|| {
                    managed
                        .versions
                        .values()
                        .find(|version| version.is_default_version)
                })
                .and_then(|version| version.document.clone())
        });
        match document {
            Some(document) => policies.documents.push(document),
            // Only an AWS-MANAGED policy is a grant this emulator cannot read; an
            // attachment naming a customer-managed policy the account does not hold grants
            // nothing, because the account is the authority on its own policies.
            None if iam_store::parse_policy_arn(arn).is_some_and(|policy| {
                policy.account == iam_store::AWS_MANAGED_POLICY_ACCOUNT
            }) =>
            {
                policies.unreadable_attachment = true;
            }
            None => {}
        }
    }
    policies
}
