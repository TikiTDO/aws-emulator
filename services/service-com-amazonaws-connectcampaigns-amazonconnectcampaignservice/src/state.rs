// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connectcampaigns-amazonconnectcampaignservice service.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    AnswerMachineDetectionConfig, DialerConfig, EncryptionConfig, InstanceOnboardingJobStatus,
    OutboundCallConfig,
};

/// This service's sigv4 signing name and ARN namespace (see `OPERATIONS.md`), used
/// for every ARN the service mints.
pub const ARN_NAMESPACE: &str = "connect-campaigns";

/// The region to stamp into an ARN when the request carries no sigv4 credential
/// scope to read one from (an unsigned, host-routed request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many campaigns one Amazon Connect instance may hold. AWS publishes a
/// per-instance campaign quota for Amazon Connect outbound campaigns (adjustable, 100
/// by default), and `CreateCampaign` models `ServiceQuotaExceededException` (HTTP 402)
/// for exceeding it, so the store is capped rather than grown without limit.
pub const CAMPAIGNS_PER_INSTANCE_QUOTA: usize = 100;

/// How many campaigns this emulator's store holds across every instance. AWS publishes
/// no such account-wide number: this is the emulator's own memory bound, because the
/// store lives in memory for the life of the process and only `DeleteCampaign` releases
/// an entry. Past it `CreateCampaign` answers the modelled
/// `ServiceQuotaExceededException` — the nearest modelled refusal — with a message that
/// says it is this store's limit rather than an AWS quota.
pub const CAMPAIGN_STORE_CAPACITY: usize = 10_000;

/// The states every [`CampaignState`] renders as, in the order the model lists them.
/// Kept beside the enum so a test can check it against the model's own
/// `CAMPAIGN_STATE_VALUES`.
pub const ALL_CAMPAIGN_STATES: &[CampaignState] = &[
    CampaignState::Initialized,
    CampaignState::Running,
    CampaignState::Paused,
    CampaignState::Stopped,
    CampaignState::Failed,
];

/// The run state of a campaign: the `CampaignState` enum of the Smithy model
/// (`Failed | Initialized | Paused | Running | Stopped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignState {
    Failed,
    Initialized,
    Paused,
    Running,
    Stopped,
}

impl CampaignState {
    /// The value this state takes on the wire — the model's enum value, so a
    /// response member and an `InvalidCampaignStateException.state` agree.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Failed => "Failed",
            Self::Initialized => "Initialized",
            Self::Paused => "Paused",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
        }
    }

    /// The spelling this state takes inside a state-conflict refusal's sentence: the
    /// model's enum value in UPPER CASE, which is how the recorded refusals of that
    /// class name the state a campaign is in.
    ///
    /// Derived from [`Self::as_wire`] rather than listed a second time, so a state
    /// cannot be named one way in a response member and another in a refusal.
    pub fn as_upper_wire(self) -> String {
        self.as_wire().to_uppercase()
    }

    /// Whether a campaign in this state takes dial requests.
    ///
    /// Only a RUNNING campaign is dialling, so only a running one has a run for a
    /// submitted dial request to join; the dial plane of a campaign in any other state
    /// is refused as the state conflict it is.
    pub fn takes_dial_requests(self) -> bool {
        matches!(self, Self::Running)
    }
}

/// A stored campaign: every member of the modelled `Campaign` shape plus the run
/// state the start/pause/stop operations move and `GetCampaignState` reports.
#[derive(Debug, Clone)]
pub struct StoredCampaign {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub connect_instance_id: String,
    pub dialer_config: DialerConfig,
    pub outbound_call_config: OutboundCallConfig,
    pub tags: HashMap<String, String>,
    pub state: CampaignState,
    /// When `CreateCampaign` stored it. No operation of this slice reports a creation
    /// time (the modelled `Campaign` shape carries none); it is kept because the
    /// instance-onboarding and update operations that are still 501 report one, and
    /// recording it at creation is the only moment it can be recorded.
    pub created_at: SystemTime,
    /// The store's creation counter at the moment this campaign was stored: the order
    /// `ListCampaigns` pages over. `created_at` cannot serve as that key, because its
    /// resolution is coarser than two consecutive creates.
    ///
    /// Assigned by [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::insert_campaign`];
    /// whatever a caller puts here is overwritten.
    pub sequence: u64,
    /// The dial requests this campaign has pending, keyed by the `clientToken` they
    /// were submitted under — which is what makes `PutDialRequestBatch` idempotent: a
    /// token this buffer already holds is answered with the id it was given then and
    /// nothing is written.
    ///
    /// A request leaves this buffer once its `expirationTime` has passed, which the
    /// clock alone owes ([`StoredCampaign::expire_dial_requests`]): an expired request
    /// is no longer dialable, so it neither occupies a buffer slot nor keeps answering
    /// for its token.
    pub dial_requests: BTreeMap<String, StoredDialRequest>,
}

impl StoredCampaign {
    /// Drop every dial request this campaign's buffer holds whose `expirationTime` has
    /// passed at `now`, reporting how many left.
    ///
    /// The buffer holds PENDING requests, so a request past its expiration is not in it.
    /// That removal is owed to the clock alone — it was due at the same instant whether
    /// or not anybody called — which is why a reporting path may materialise it.
    pub fn expire_dial_requests(&mut self, now: SystemTime) -> usize {
        let before = self.dial_requests.len();
        self.dial_requests
            .retain(|_, request| !deadline_passed(request.expires_at, now));
        before - self.dial_requests.len()
    }
}

/// One dial request a campaign's buffer holds: the id this service minted for it, and
/// what the request asked to dial exactly as it was submitted.
///
/// `phoneNumber` and `attributes` are `@sensitive` in the model and no operation of this
/// service reports them back; they are kept because they are what the pending request IS,
/// so the buffer says what each pending request is for and not merely that one is
/// pending.
#[derive(Debug, Clone)]
pub struct StoredDialRequest {
    /// The `DialRequestId` this service minted for the request, which
    /// `PutDialRequestBatch` reports in its `successfulRequests`.
    pub id: String,
    pub phone_number: String,
    pub attributes: HashMap<String, String>,
    /// The instant after which the request can no longer be dialled, as the request
    /// carried it.
    pub expires_at: ::aws_smithy_types::DateTime,
}

/// `at` as the `(seconds, nanoseconds)` pair an `::aws_smithy_types::DateTime` reports an
/// instant as, so a wall-clock instant and a modelled timestamp compare exactly rather
/// than through a lossy float.
fn instant_parts(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// Whether the instant `deadline` has passed at `now`, compared exactly through
/// [`instant_parts`] rather than rounded to whole seconds.
///
/// The one place a deadline a caller sent is weighed against the clock, so every reader
/// of a stored deadline draws the line at the same instant.
fn deadline_passed(deadline: ::aws_smithy_types::DateTime, now: SystemTime) -> bool {
    instant_parts(::aws_smithy_types::DateTime::from(now)) >= instant_parts(deadline)
}

/// Per-service state for com-amazonaws-connectcampaigns-amazonconnectcampaignservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState {
    /// Every campaign, keyed by its minted campaign id.
    pub campaigns: HashMap<String, StoredCampaign>,
    /// The onboarding job of each Amazon Connect instance this service has onboarded to
    /// Campaigns, keyed by that instance's id — one record per instance, written by
    /// `StartInstanceOnboardingJob` and bounded at [`MAX_ONBOARDED_INSTANCES`] by
    /// [`Self::start_instance_onboarding`].
    ///
    /// The one record of which instances are onboarded and of what their campaign data
    /// is encrypted with, so `GetInstanceOnboardingJobStatus` and every later reader of
    /// an instance's configuration cannot disagree about an instance.
    pub instance_onboarding: HashMap<String, StoredInstanceOnboarding>,
    /// Campaign ARN -> campaign id, so an ARN-keyed lookup resolves without scanning
    /// `campaigns`. Maintained by [`Self::insert_campaign`] and [`Self::remove_campaign`]
    /// — the only two writers of `campaigns` — so it cannot drift from them; its first
    /// readers are the tag operations (`TagResource`, `UntagResource`,
    /// `ListTagsForResource`), which take an ARN and are still 501.
    pub arn_index: HashMap<String, String>,
    /// Creation counter: the `sequence` the next stored campaign takes.
    pub next_sequence: u64,
    /// Dial-request counter: the position the next `DialRequestId` this store mints
    /// carries. Store-wide, so no two dial requests of any campaign share an id.
    pub next_dial_sequence: u64,
}

impl ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState {
    /// Store a campaign and index its ARN, stamping it with the next `sequence`.
    /// Returns the campaign as stored.
    pub fn insert_campaign(&mut self, mut campaign: StoredCampaign) -> StoredCampaign {
        campaign.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.arn_index
            .insert(campaign.arn.clone(), campaign.id.clone());
        self.campaigns.insert(campaign.id.clone(), campaign.clone());
        campaign
    }

    /// How many campaigns the named Amazon Connect instance holds — what
    /// [`CAMPAIGNS_PER_INSTANCE_QUOTA`] caps.
    pub fn campaigns_for_instance(&self, connect_instance_id: &str) -> usize {
        self.campaigns
            .values()
            .filter(|campaign| campaign.connect_instance_id == connect_instance_id)
            .count()
    }

    /// Drop a campaign and its ARN index entry. `None` when no such campaign.
    pub fn remove_campaign(&mut self, id: &str) -> Option<StoredCampaign> {
        let removed = self.campaigns.remove(id)?;
        self.arn_index.remove(&removed.arn);
        Some(removed)
    }

    /// Move one campaign's run state into `to`, from any of `allowed_from`, and report
    /// what that did.
    ///
    /// The one place a run-state transition is applied, so start, pause and stop cannot
    /// disagree about what a transition does: each names only the states it may move
    /// from, and the rule they all share lives here.
    ///
    /// A campaign ALREADY in `to` is accepted and nothing is written
    /// ([`TransitionApplied::Unchanged`]): a request asking a campaign to enter the state
    /// it already holds has nothing left to do, so a retry after a timeout and two
    /// callers racing to the same state each succeed. A campaign in any OTHER state
    /// outside `allowed_from` is refused, and nothing is written then either.
    pub fn transition_campaign(
        &mut self,
        id: &str,
        allowed_from: &[CampaignState],
        to: CampaignState,
    ) -> Result<TransitionApplied, TransitionRefused> {
        let campaign = self
            .campaigns
            .get_mut(id)
            .ok_or(TransitionRefused::NoSuchCampaign)?;
        if campaign.state == to {
            return Ok(TransitionApplied::Unchanged(to));
        }
        if !allowed_from.contains(&campaign.state) {
            return Err(TransitionRefused::InvalidState(campaign.state));
        }
        campaign.state = to;
        Ok(TransitionApplied::Moved(to))
    }

    /// Whether the campaign `id` names takes dial requests right now, or the refusal the
    /// caller owes instead.
    ///
    /// The one place the dialability rule of this service lives, so the dial plane and
    /// any later reader of it cannot disagree about which campaign dials. Nothing is
    /// written on either refusal, and the refusal names the state it judged so the
    /// caller's sentence can report it.
    pub fn dialable_campaign(&self, id: &str) -> Result<(), DialRejected> {
        let Some(stored) = self.campaigns.get(id) else {
            return Err(DialRejected::NoSuchCampaign);
        };
        if !stored.state.takes_dial_requests() {
            return Err(DialRejected::NotDialable(stored.state));
        }
        Ok(())
    }

    /// The position the next `DialRequestId` this store mints carries.
    ///
    /// Minted from the store's own counter, never from anything in the request. The text
    /// it is rendered into is within `DialRequestId`'s pattern `^[a-zA-Z0-9_\-.]*$` and
    /// its 0..256 length bound.
    fn next_dial_request_id(&mut self) -> String {
        let sequence = self.next_dial_sequence;
        self.next_dial_sequence = self.next_dial_sequence.saturating_add(1);
        format!("dial-{sequence:012}")
    }

    /// Take the dial request submitted under `client_token` into the buffer of the
    /// campaign `campaign_id` names, and report what became of it (see
    /// [`DialRequestOutcome`]). `None` when no campaign is stored under that id.
    ///
    /// A `client_token` the campaign has already taken stores nothing and reports the id
    /// it was given then, which is what makes a repeated batch idempotent.
    ///
    /// `expires_at` is weighed against `now` first: a request whose expiration has
    /// already passed can never be dialled, so it is not taken at all, mints no id and
    /// occupies no slot. The expiries the clock already owes the buffer are materialised
    /// at the same instant ([`StoredCampaign::expire_dial_requests`]), so a slot and a
    /// `clientToken` are both held only while their request is still pending.
    pub fn take_dial_request(
        &mut self,
        campaign_id: &str,
        client_token: &str,
        phone_number: &str,
        attributes: &HashMap<String, String>,
        expires_at: ::aws_smithy_types::DateTime,
        now: SystemTime,
    ) -> Option<DialRequestOutcome> {
        {
            let stored = self.campaigns.get_mut(campaign_id)?;
            stored.expire_dial_requests(now);
            if deadline_passed(expires_at, now) {
                return Some(DialRequestOutcome::Expired);
            }
            if let Some(taken) = stored.dial_requests.get(client_token) {
                return Some(DialRequestOutcome::Taken(taken.id.clone()));
            }
            if stored.dial_requests.len() >= MAX_DIAL_REQUESTS_PER_CAMPAIGN {
                return Some(DialRequestOutcome::BufferFull);
            }
        }
        // Only now, past every refusal, is an id minted — so a request that was not
        // taken consumes no id and leaves no gap in the sequence.
        let id = self.next_dial_request_id();
        let stored = self.campaigns.get_mut(campaign_id)?;
        stored.dial_requests.insert(
            client_token.to_string(),
            StoredDialRequest {
                id: id.clone(),
                phone_number: phone_number.to_string(),
                attributes: attributes.clone(),
                expires_at,
            },
        );
        Some(DialRequestOutcome::Taken(id))
    }

    /// Start the onboarding job of `connect_instance_id`, persisting `encryption_config`
    /// as that instance's, or report why no job was started.
    ///
    /// The one place this store takes an onboarding record, so the two refusals an
    /// onboarding call owes are decided once rather than per handler: an instance this
    /// service already holds a job for is not onboarded a second time, and the store is
    /// bounded. Nothing is written on either refusal.
    pub fn start_instance_onboarding(
        &mut self,
        connect_instance_id: &str,
        encryption_config: EncryptionConfig,
        now: SystemTime,
    ) -> Result<StoredInstanceOnboarding, OnboardingRefused> {
        if self.instance_onboarding.contains_key(connect_instance_id) {
            return Err(OnboardingRefused::AlreadyOnboarded);
        }
        if self.instance_onboarding.len() >= MAX_ONBOARDED_INSTANCES {
            return Err(OnboardingRefused::StoreFull);
        }
        let record = StoredInstanceOnboarding::starting(
            connect_instance_id.to_string(),
            encryption_config,
            now,
        );
        self.instance_onboarding
            .insert(record.connect_instance_id.clone(), record.clone());
        Ok(record)
    }

    /// Delete the onboarding job this service holds for `connect_instance_id`, and
    /// report what became of the request (see [`DeleteOnboardingOutcome`]).
    ///
    /// The one place an onboarding record is dropped, and the one place the rule that
    /// governs the drop lives: only a job in [`ONBOARDING_STATUS_FAILED`] may be
    /// deleted, so a job in any other status keeps its record and the status a later
    /// `GetInstanceOnboardingJobStatus` reports is untouched by a refused delete. The
    /// status is read off the record at `now`, the same way every other reader of it
    /// reads it, so the clock alone decides nothing differently here.
    pub fn delete_instance_onboarding(
        &mut self,
        connect_instance_id: &str,
        now: SystemTime,
    ) -> DeleteOnboardingOutcome {
        let Some(record) = self.instance_onboarding.get(connect_instance_id) else {
            return DeleteOnboardingOutcome::NoSuchJob;
        };
        if record.status_at(now) != ONBOARDING_STATUS_FAILED {
            return DeleteOnboardingOutcome::NotFailed;
        }
        self.instance_onboarding.remove(connect_instance_id);
        DeleteOnboardingOutcome::Deleted
    }

    /// The onboarding record this service holds for `connect_instance_id`, or `None`
    /// when no call has onboarded that instance.
    ///
    /// The one lookup of that record, so a reporting operation reads what
    /// [`Self::start_instance_onboarding`] wrote and nothing else.
    pub fn instance_onboarding(
        &self,
        connect_instance_id: &str,
    ) -> Option<&StoredInstanceOnboarding> {
        self.instance_onboarding.get(connect_instance_id)
    }

    /// Replace the stored name of one campaign and report what that did. `None` when no
    /// campaign is stored under that id, in which case nothing is written.
    ///
    /// The one place a campaign's name is amended, so the operation that renames a
    /// campaign and every later reader of it cannot disagree about what a rename does:
    /// only the name member moves, and the campaign's id, ARN, instance, configs, tags,
    /// run state and creation order are left exactly as they were.
    ///
    /// A campaign that ALREADY carries that name is reported as
    /// [`AmendApplied::Unchanged`] and nothing is written, which is what makes the rename
    /// idempotent: a retry after a timeout, or two callers asking for the same name, each
    /// succeed and leave one stored name behind.
    pub fn rename_campaign(&mut self, id: &str, name: &str) -> Option<AmendApplied> {
        let campaign = self.campaigns.get_mut(id)?;
        if campaign.name == name {
            return Some(AmendApplied::Unchanged);
        }
        campaign.name = name.to_string();
        Some(AmendApplied::Amended)
    }

    /// Amend the outbound-call config of one campaign with the members an amendment
    /// CARRIES, and report what that did. `None` when no campaign is stored under that
    /// id, in which case nothing is written.
    ///
    /// The one place that config is amended, so the operation that updates it and every
    /// later reader of it cannot disagree about what an update does: a member the
    /// amendment carries replaces the stored one, a member it omits leaves the stored one
    /// exactly as it was, and no other part of the campaign — its id, ARN, name,
    /// instance, dialer config, tags, run state or creation order — moves. The stored
    /// queue id is among what stays: the update request models no member for it, so no
    /// amendment can name one.
    ///
    /// An amendment every member of which already matches what is stored is reported as
    /// [`AmendApplied::Unchanged`] and nothing is written, which is what makes the update
    /// idempotent: a retry after a timeout, and an amendment that carries no member at
    /// all, each succeed and leave one stored config behind.
    pub fn amend_outbound_call_config(
        &mut self,
        id: &str,
        amendment: OutboundCallConfigAmendment<'_>,
    ) -> Option<AmendApplied> {
        let config = &mut self.campaigns.get_mut(id)?.outbound_call_config;
        let mut amended = false;

        if let Some(contact_flow_id) = amendment.connect_contact_flow_id {
            if config.connect_contact_flow_id != contact_flow_id {
                config.connect_contact_flow_id = contact_flow_id.to_string();
                amended = true;
            }
        }
        if let Some(source_phone_number) = amendment.connect_source_phone_number {
            if config.connect_source_phone_number.as_deref() != Some(source_phone_number) {
                config.connect_source_phone_number = Some(source_phone_number.to_string());
                amended = true;
            }
        }
        if let Some(detection) = amendment.answer_machine_detection_config {
            if !same_answer_machine_detection(config.answer_machine_detection_config.as_ref(), detection)
            {
                config.answer_machine_detection_config = Some(detection.clone());
                amended = true;
            }
        }

        Some(if amended {
            AmendApplied::Amended
        } else {
            AmendApplied::Unchanged
        })
    }

    /// Every campaign in creation order — the order `ListCampaigns` pages over.
    pub fn campaigns_in_creation_order(&self) -> Vec<&StoredCampaign> {
        let mut all: Vec<&StoredCampaign> = self.campaigns.values().collect();
        // `sequence` is the creation counter, so it orders creations exactly — unlike
        // `created_at`, whose resolution is coarser than two consecutive creates.
        all.sort_by(|a, b| a.sequence.cmp(&b.sequence).then_with(|| a.id.cmp(&b.id)));
        all
    }
}

/// Why a requested run-state transition did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionRefused {
    /// No campaign with that id is stored (`ResourceNotFoundException`, HTTP 404).
    NoSuchCampaign,
    /// The campaign is in a state this transition cannot be made from
    /// (`ConflictException`, HTTP 409, worded by [`campaign_invalid_state_message`]),
    /// carrying the state it is in.
    InvalidState(CampaignState),
}

/// What a requested run-state transition DID, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::transition_campaign`]
/// reports it.
///
/// Both variants are successes; they are kept apart because only one of them changed
/// anything, and a transition that did not happen is not audited as one that did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionApplied {
    /// The campaign moved into the state the operation asked for and now holds it.
    Moved(CampaignState),
    /// The campaign already held that state, so nothing was written.
    Unchanged(CampaignState),
}

/// What a requested amendment to one member of a stored campaign DID, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::rename_campaign`]
/// reports it.
///
/// Both variants are successes, kept apart for the same reason [`TransitionApplied`]'s
/// are: only one of them changed anything, and an amendment that wrote nothing is not
/// audited as one that did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmendApplied {
    /// The campaign's member now carries the value the request asked for.
    Amended,
    /// The campaign already carried that value, so nothing was written.
    Unchanged,
}

/// The members one amendment of a campaign's outbound-call config carries, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::amend_outbound_call_config`]
/// takes them.
///
/// A member left `None` here is one the request did not carry, and the stored value of it
/// is left alone — so the absent member and a member asking for a new value are kept
/// apart rather than collapsed into one "replace the whole config" step.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutboundCallConfigAmendment<'a> {
    pub connect_contact_flow_id: Option<&'a str>,
    pub connect_source_phone_number: Option<&'a str>,
    pub answer_machine_detection_config: Option<&'a AnswerMachineDetectionConfig>,
}

/// Whether a stored answering-machine-detection config already says what `wanted` says,
/// member by member.
///
/// The modelled shape carries no equality of its own, so the comparison lives here —
/// beside the amendment that needs it — rather than being open-coded per caller.
fn same_answer_machine_detection(
    stored: Option<&AnswerMachineDetectionConfig>,
    wanted: &AnswerMachineDetectionConfig,
) -> bool {
    stored.is_some_and(|stored| {
        stored.enable_answer_machine_detection == wanted.enable_answer_machine_detection
            && stored.await_answer_machine_prompt == wanted.await_answer_machine_prompt
    })
}

/// Why this store took no dial request for a campaign, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::dialable_campaign`]
/// reports it. Nothing is written in either case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialRejected {
    /// No campaign with that id is stored, so there is no buffer to take the batch into:
    /// the modelled `ResourceNotFoundException` (HTTP 404).
    NoSuchCampaign,
    /// The campaign is in a state that takes no dial requests (see
    /// [`CampaignState::takes_dial_requests`]), carrying that state: the modelled
    /// `ConflictException` (HTTP 409), worded by [`campaign_invalid_state_message`].
    NotDialable(CampaignState),
}

/// What became of one dial request of a batch, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::take_dial_request`]
/// reports it.
///
/// A per-request outcome, not a refusal of the call: the batch operation reports each of
/// these on its own, through `successfulRequests` or `failedRequests`, and the rest of
/// the batch is unaffected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialRequestOutcome {
    /// The request is in the campaign's buffer under this `DialRequestId` — which is the
    /// id it was given the first time its `clientToken` was seen.
    Taken(String),
    /// Its `expirationTime` had already passed, so it can never be dialled and was not
    /// taken: reported with the model's `InvalidInput` failure code.
    Expired,
    /// The campaign's buffer already holds [`MAX_DIAL_REQUESTS_PER_CAMPAIGN`] pending
    /// requests: reported with the model's `RequestThrottled` failure code.
    BufferFull,
}

/// The most dial requests one campaign's buffer in this emulator's store holds, past
/// which a further request of a batch is reported as throttled instead of taken.
///
/// AWS publishes no such number: this is the emulator's own memory bound, because the
/// buffer lives in memory and grows one record per accepted request. The model's own
/// `FailureCode` catalog names what a buffer that cannot take another answers with —
/// `RequestThrottled`, documented as "Request throttled due to large number of pending
/// dial requests" — so that is the code reported, set far above any plausible batch.
pub const MAX_DIAL_REQUESTS_PER_CAMPAIGN: usize = 10_000;

/// `DestinationPhoneNumber`'s own constraints as the trait map the framework's constraint
/// walk reads them from: `@length(min 0, max 20)` and `@pattern("^[\d\-+]*$")`, from
/// `smithy_query(query="shape", shape="DestinationPhoneNumber")`.
static DESTINATION_PHONE_NUMBER_TRAITS: ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    ::std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(::std::boxed::Box::new(
            ::aws_smithy_schema_server::constraints::LengthTrait::new(Some(0), Some(20)),
        ));
        traits.insert(::std::boxed::Box::new(
            ::aws_smithy_schema_server::constraints::PatternTrait::new(r"^[\d\-+]*$"),
        ));
        traits
    });

/// The schema a submitted destination number is judged against: one string carrying
/// `DestinationPhoneNumber`'s own constraints.
///
/// It is NOT the member schema the request was deserialized against, deliberately: those
/// constraints are enforced on this request alone rather than at the boundary, because a
/// destination this service cannot dial is reported on that request's own failure channel
/// and the rest of the batch is served (see [`dialable_phone_number`]).
static DESTINATION_PHONE_NUMBER_SCHEMA: ::std::sync::LazyLock<::aws_smithy_schema::Schema> =
    ::std::sync::LazyLock::new(|| {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.connectcampaigns#DialRequest$phoneNumber",
                "com.amazonaws.connectcampaigns",
                "phoneNumber",
            ),
            ::aws_smithy_schema::ShapeType::String,
            "phoneNumber",
            0,
        )
        .with_traits(&DESTINATION_PHONE_NUMBER_TRAITS)
    });

/// Whether `phone_number` is a destination a campaign can dial: a value the
/// `DestinationPhoneNumber` shape admits.
///
/// Judged by the framework's own leaf walk against
/// [`DESTINATION_PHONE_NUMBER_SCHEMA`], so a number is weighed by the very code the
/// boundary weighs a constrained string member with and the rules enforced are the
/// model's declared ones rather than a second, hand-rolled reading of them. The one
/// place this service decides whether a submitted number is dialable.
pub fn dialable_phone_number(phone_number: &str) -> bool {
    use ::aws_smithy_schema::serde::ShapeSerializer as _;

    let mut validator = ::aws_smithy_schema_server::constraints::ValidatingSerializer::new();
    validator.set_fail_fast(true);
    // Infallible: the validator accumulates violations instead of failing.
    let _ = validator.write_string(&DESTINATION_PHONE_NUMBER_SCHEMA, phone_number);
    validator.into_violations().is_empty()
}

/// What this service answers when a campaign is in a run state the change a request asks
/// for cannot be made from: a lifecycle transition the state machine does not admit, or
/// a dial request for a campaign that is not running.
///
/// Composed from the request and from service state — the id the caller addressed and
/// the state the campaign is in, spelled as [`CampaignState::as_upper_wire`] renders it.
/// The one text every refusal of this class shares, so the lifecycle operations and the
/// dial plane cannot word one state conflict four ways.
pub fn campaign_invalid_state_message(id: &str, state: CampaignState) -> String {
    format!(
        "Campaign: {id} is in invalid state: {}",
        state.as_upper_wire()
    )
}

/// The `InstanceOnboardingJobStatusCode` value an onboarding job that has not finished
/// yet reports, as the model's catalog declares it (see
/// `INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES` in [`crate::ops::types`]).
pub const ONBOARDING_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `InstanceOnboardingJobStatusCode` value a finished onboarding job reports, from
/// the same catalog.
pub const ONBOARDING_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The `InstanceOnboardingJobStatusCode` value a job that could not onboard the instance
/// reports, from the same catalog. It is the ONLY status an onboarding job may be
/// DELETED in (see
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::delete_instance_onboarding`]).
///
/// A job carries it exactly while its record names the
/// `InstanceOnboardingJobFailureCode` it ended on
/// ([`StoredInstanceOnboarding::failure_code`]); every code that catalog declares names
/// a dependency of the onboarding outside this service — a KMS key, an IAM role, an
/// EventBridge rule — so nothing this emulator's onboarding does can end a job here,
/// and no operation of this crate writes that member yet.
pub const ONBOARDING_STATUS_FAILED: &str = "FAILED";

/// What this service answers when the onboarding job a request asks it to delete is in
/// any status but [`ONBOARDING_STATUS_FAILED`], so there is no failed job to clear.
///
/// A FIXED service string stating the rule it enforces: it names no instance and carries
/// nothing from the request, so every refusal of this class reads the same.
pub const ONBOARDING_JOB_NOT_FAILED_MESSAGE: &str =
    "Instance onboarding job is not in failed status. Only failed job can be deleted.";

/// How long after the `StartInstanceOnboardingJob` call that started it an onboarding
/// job is due to report [`ONBOARDING_STATUS_SUCCEEDED`].
///
/// AWS documents no duration for the job, so this span is the emulator's own: long
/// enough that a caller which polls straight after the start call still observes the
/// transient status a real job has, short enough that a poller reaches the finished one
/// without waiting on anything. Nothing sleeps for it — the record carries the instant
/// the job is due and [`StoredInstanceOnboarding::status_at`] reads the status off the
/// clock.
pub const ONBOARDING_JOB_DURATION: ::std::time::Duration = ::std::time::Duration::from_secs(3);

/// The most Amazon Connect instances this emulator's store holds onboarding records
/// for, after which `StartInstanceOnboardingJob` refuses to take another.
///
/// The store grows one record per newly onboarded instance and the instance id is the
/// caller's, so without a ceiling a caller could repeat the call under fresh ids until
/// the process runs out of memory. AWS publishes no such limit, so this is the
/// emulator's own, set far above any plausible workload.
pub const MAX_ONBOARDED_INSTANCES: usize = 1_000;

/// The onboarding job of one Amazon Connect instance: the encryption config the
/// `StartInstanceOnboardingJob` call persisted for it, and the instant that job is due
/// to have finished.
#[derive(Debug, Clone)]
pub struct StoredInstanceOnboarding {
    pub connect_instance_id: String,
    /// What this instance's campaign data is encrypted with, exactly as the call that
    /// started the job supplied it.
    pub encryption_config: EncryptionConfig,
    /// The instant this job reaches [`ONBOARDING_STATUS_SUCCEEDED`]: the start instant
    /// plus [`ONBOARDING_JOB_DURATION`]. Until then the job is still in progress.
    pub succeeds_at: SystemTime,
    /// The `InstanceOnboardingJobFailureCode` this job ended on, or `None` for a job
    /// that has not failed — which is every job this emulator's onboarding starts, for
    /// the reason [`ONBOARDING_STATUS_FAILED`] states.
    ///
    /// It is what makes a record's job a FAILED one: [`Self::status_at`] reports that
    /// status exactly while this member is set, and [`Self::to_job_status`] reports the
    /// code beside it, so a job's status and its failure code cannot disagree.
    pub failure_code: Option<String>,
}

impl StoredInstanceOnboarding {
    /// A job just started for `connect_instance_id` at `now`, due to have finished one
    /// [`ONBOARDING_JOB_DURATION`] later.
    pub fn starting(
        connect_instance_id: String,
        encryption_config: EncryptionConfig,
        now: SystemTime,
    ) -> Self {
        Self {
            connect_instance_id,
            encryption_config,
            succeeds_at: now + ONBOARDING_JOB_DURATION,
            failure_code: None,
        }
    }

    /// The job's status at `now`: failed when the record names the code it failed with,
    /// and otherwise finished once the clock has passed the due instant and in progress
    /// until then.
    ///
    /// A pure read of the stored record, so reporting a job's status writes nothing and
    /// two callers reading at the same moment see the same value.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if self.failure_code.is_some() {
            ONBOARDING_STATUS_FAILED
        } else if now < self.succeeds_at {
            ONBOARDING_STATUS_IN_PROGRESS
        } else {
            ONBOARDING_STATUS_SUCCEEDED
        }
    }

    /// This record as the modelled `InstanceOnboardingJobStatus` the onboarding
    /// operations report, so the start and the status read cannot word one job two ways.
    ///
    /// `failureCode` is the code the record names, which is absent for every job this
    /// emulator's onboarding starts: the onboarding it performs is writing the record,
    /// which either happened or was refused before any job started.
    pub fn to_job_status(&self, now: SystemTime) -> InstanceOnboardingJobStatus {
        InstanceOnboardingJobStatus {
            connect_instance_id: self.connect_instance_id.clone(),
            status: self.status_at(now).to_string(),
            failure_code: self.failure_code.clone(),
        }
    }
}

/// Why a requested onboarding job was not started, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::start_instance_onboarding`]
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingRefused {
    /// The instance already holds an onboarding job, so there is no onboarding left to
    /// start for it: the modelled `ConflictException` (HTTP 409).
    AlreadyOnboarded,
    /// This emulator's store already holds [`MAX_ONBOARDED_INSTANCES`] records, so it
    /// takes no further instance.
    StoreFull,
}

/// What became of a request to delete one instance's onboarding job, as
/// [`ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState::delete_instance_onboarding`]
/// reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOnboardingOutcome {
    /// The record was in the failed status and is gone, so the instance holds no
    /// onboarding job any more.
    Deleted,
    /// This service holds no onboarding record for that instance, so there is no job to
    /// delete: the modelled `ResourceNotFoundException` (HTTP 404) carrying
    /// [`onboarding_record_absent_message`].
    NoSuchJob,
    /// The job is in a status other than [`ONBOARDING_STATUS_FAILED`], so it stays: the
    /// modelled `InvalidStateException` (HTTP 409) carrying
    /// [`ONBOARDING_JOB_NOT_FAILED_MESSAGE`].
    NotFailed,
}

/// What this service answers when the Amazon Connect instance a request names is
/// already onboarded to Campaigns, so its onboarding job cannot be started again.
///
/// The sentence the model documents for `ConflictException` — "The request could not be
/// processed because of conflict in the current state of the resource" — which is what
/// this refusal is: the state of the instance, and not the request, is what stops the
/// call. A FIXED service string carrying nothing from the request, so every refusal of
/// this class reads the same.
pub const INSTANCE_ALREADY_ONBOARDED_MESSAGE: &str =
    "The request could not be processed because of conflict in the current state of the resource.";

/// What this service answers when the Amazon Connect instance a request asks it to
/// onboard is not one the account holds: the id names no instance to onboard.
///
/// Composed from the request, so the refusal names the instance the caller asked about —
/// the same shape as [`campaign_not_found_message`], which is how this service words a
/// resource its request addresses by id. `CreateCampaign`, whose own resource is the
/// campaign and which only REQUIRES a usable instance, carries
/// [`INSTANCE_NOT_USABLE_MESSAGE`] for the same registry outcome, because each operation
/// answers it in the words recorded for that operation.
pub fn connect_instance_not_found_message(connect_instance_id: &str) -> String {
    format!("ConnectInstanceId not found {connect_instance_id}.")
}

/// What this service answers when an Amazon Connect instance has no onboarding job to
/// report on: it was never onboarded, or its job has since been deleted.
///
/// The onboarding RECORD is the resource that is missing here, not the instance, and the
/// record is scoped to an account — so the sentence names the caller's account id and the
/// instance the request named, and carries no trailing period. The one text the
/// operations whose own resource IS that record share, so they cannot disagree about a
/// request they both refuse.
pub fn onboarding_record_absent_message(connect_instance_id: &str) -> String {
    format!(
        "Instance onboarding record does not exist for aws account id {} and instance id {connect_instance_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The region a request was signed for, which is the region of every ARN the
/// handler serving it mints. Falls back to [`DEFAULT_REGION`] for a request that
/// carries no sigv4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A fresh campaign id.
///
/// The model shapes `CampaignId` as a non-whitespace string of at most 256 characters
/// (`len 0..256, pattern ^[\S]*$`), so a 32-character lowercase hex id satisfies it. The
/// bytes are never derived from the request: this process's OS-seeded
/// [`std::collections::hash_map::RandomState`] keys, the clock and a process-wide mint
/// counter are hashed together, so two creates carrying identical bodies still get
/// different ids and an id cannot be guessed from what was sent. (The same entropy
/// recipe as `services/service-com-amazonaws-vpclattice-mercurycontrolplane`'s
/// `fresh_entropy`.)
pub fn fresh_campaign_id() -> String {
    use ::sha2::{Digest, Sha256};
    use ::std::hash::{BuildHasher, Hasher};
    use ::std::sync::atomic::{AtomicU64, Ordering};

    /// How many ids this process has minted, so two ids minted in the same clock tick
    /// still differ.
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let os_seeded = ::std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let digest = Sha256::digest(format!("{nanos}:{minted}:{os_seeded}").as_bytes());
    ::hex::encode(&digest[..16])
}

/// The codec every `ListCampaigns` `nextToken` is written with: the standard base64
/// alphabet, so the token a caller is handed back is the opaque base64 value this
/// service's pagination token is, and a token that is not base64 at all fails to
/// DECODE before anything is resolved from it.
///
/// Padding is accepted but not required on the way in, so a token whose trailing `=`
/// a client dropped still decodes — the same latitude a decoder that pads on demand
/// gives.
const PAGE_TOKEN_CODEC: ::base64::engine::GeneralPurpose = ::base64::engine::GeneralPurpose::new(
    &::base64::alphabet::STANDARD,
    ::base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(::base64::engine::DecodePaddingMode::Indifferent),
);

/// How many hex digits the position inside a `nextToken` is written with, so every
/// token this service mints is the same length whatever position it names.
const PAGE_TOKEN_POSITION_DIGITS: usize = 16;

/// The `ListCampaigns` `nextToken` that resumes a walk at creation counter `sequence`.
///
/// A token names a POSITION in the creation order, not a campaign: deleting the campaign
/// a page was going to start at leaves the token resumable, and the token carries no
/// campaign id for a caller to read out of it. It is handed out base64-encoded, which
/// is the form [`page_token_sequence`] decodes before it resolves anything.
pub fn page_token(sequence: u64) -> String {
    use ::base64::Engine as _;

    PAGE_TOKEN_CODEC.encode(format!("{sequence:0width$x}", width = PAGE_TOKEN_POSITION_DIGITS))
}

/// Why a `nextToken` a caller sent names no position to resume a walk from.
///
/// Two outcomes, because a pagination token is resolved in two steps and each is
/// refused in its own words: a value that is not base64 does not survive the DECODE,
/// and a value that decodes but names no position this service issued does not
/// RESOLVE. [`Self::message`] is the one place either sentence lives, so every paged
/// list of this service refuses alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageTokenRejected {
    /// The token is not a base64 value, carrying the decoder's own account of why.
    Undecodable(String),
    /// The token decodes, but it is not one [`page_token`] minted.
    Unissued,
}

impl PageTokenRejected {
    /// The sentence this rejection is refused with.
    pub fn message(&self) -> String {
        match self {
            Self::Undecodable(why) => why.clone(),
            // This emulator's own wording, not a recorded one: a token only this
            // emulator can mint has no recorded refusal to reproduce. It carries
            // nothing from the request — a token a caller supplied is not echoed back
            // into the refusal — and it is deliberately not a decode failure's
            // sentence, which is the distinction the refusal exists to draw.
            Self::Unissued => "nextToken is not a page token this service issued".to_string(),
        }
    }
}

/// How a base64 decoder reports a value carrying a character its alphabet does not
/// hold: it stops at that character and names it by the hex code of its byte, so the
/// sentence is composed from the token the request carried.
pub fn illegal_base64_character_message(byte: u8) -> String {
    format!("Illegal base64 character {byte:x}")
}

/// How a base64 decoder reports a value whose last 4-byte unit carries too few bits to
/// decode — a dangling character left over after the whole units.
pub const DANGLING_BASE64_UNIT_MESSAGE: &str = "Last unit does not have enough valid bits";

/// How a base64 decoder reports a value whose last 4-byte unit is not one: padding
/// where no padding belongs, or too little of it.
pub const MALFORMED_BASE64_UNIT_MESSAGE: &str = "Input byte array has wrong 4-byte ending unit";

/// The padding character of the standard base64 alphabet, which closes a value's last
/// unit rather than carrying data — so it is not an illegal character, and a value that
/// misplaces it is refused as a malformed unit instead.
const BASE64_PAD: u8 = b'=';

/// The bytes `token` decodes to, or why it is not a base64 value at all.
///
/// Every account of WHY comes from the codec's own error rather than from a second,
/// hand-rolled pass over the token: the decoder stops at the first byte its alphabet does
/// not hold, and that is the byte the refusal names.
fn decode_page_token(token: &str) -> Result<Vec<u8>, PageTokenRejected> {
    use ::base64::Engine as _;

    match PAGE_TOKEN_CODEC.decode(token) {
        Ok(bytes) => Ok(bytes),
        Err(::base64::DecodeError::InvalidByte(_, byte)) if byte != BASE64_PAD => Err(
            PageTokenRejected::Undecodable(illegal_base64_character_message(byte)),
        ),
        Err(::base64::DecodeError::InvalidLength(_)) => Err(PageTokenRejected::Undecodable(
            DANGLING_BASE64_UNIT_MESSAGE.to_string(),
        )),
        Err(::base64::DecodeError::InvalidByte(..) | ::base64::DecodeError::InvalidPadding) => Err(
            PageTokenRejected::Undecodable(MALFORMED_BASE64_UNIT_MESSAGE.to_string()),
        ),
        // The value decodes; only its last unit carries bits a decoder that pads on
        // demand discards. Whatever those bytes are, they are not a token this service
        // minted, so it is refused as one it never issued rather than as a decode
        // failure.
        Err(::base64::DecodeError::InvalidLastSymbol(..)) => Err(PageTokenRejected::Unissued),
    }
}

/// The creation counter a `nextToken` resumes at, or why that token names no position.
///
/// The token is DECODED first — a value that is not base64 is refused as the decode
/// failure it is — and only then resolved against the form [`page_token`] mints.
pub fn page_token_sequence(token: &str) -> Result<u64, PageTokenRejected> {
    let decoded = decode_page_token(token)?;
    let position = String::from_utf8(decoded).map_err(|_| PageTokenRejected::Unissued)?;
    if position.len() != PAGE_TOKEN_POSITION_DIGITS {
        return Err(PageTokenRejected::Unissued);
    }
    u64::from_str_radix(&position, 16).map_err(|_| PageTokenRejected::Unissued)
}

/// Record one state-changing campaign operation: when (the record's own timestamp), who
/// asked (the access key id the request presented), what changed and how it ended.
///
/// The one place this service writes an audit record, so create, delete and every
/// run-state transition record the same fields. Only wire metadata and server-minted
/// identifiers go in: never a campaign's name, its tags or any credential material.
pub fn audit_campaign_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    campaign_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        campaign_id,
        "campaign changed"
    );
}

/// Record one state-changing Connect-instance operation — starting an instance's
/// onboarding job — with the same fields as [`audit_campaign_change`]: when, who asked,
/// which instance and how the call ended.
///
/// Called on EVERY path such a call returns through, the accepted one and each refusal,
/// named by the modelled error the refusal answers with — so an attempt to change what
/// this service holds for an instance is as visible when it was refused as when it
/// succeeded. Only wire metadata and the instance id go in: never an encryption key,
/// an ARN a caller supplied or any credential material.
pub fn audit_onboarding_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    connect_instance_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        connect_instance_id,
        "connect instance onboarding changed"
    );
}

/// The ARN of a campaign: `arn:aws:connect-campaigns:<region>:<account>:campaign/<id>`.
pub fn campaign_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("campaign/{id}"))
}

/// What this service answers when a well-formed campaign id names no campaign, for the
/// operations that RESOLVE the campaign resource that id names — the describe and the
/// delete, addressed to `/campaigns/{id}` itself.
///
/// Composed from the request, so the refusal names the id the caller asked for. The one
/// text those operations share, so two of them cannot disagree about a lookup they both
/// refuse. The operations addressed to the campaign's RUN STATE word the same miss
/// differently and take [`campaign_absent_message`] instead.
pub fn campaign_not_found_message(id: &str) -> String {
    format!("CampaignId not found {id}.")
}

/// What this service answers when a well-formed campaign id names no campaign, for the
/// operations addressed to the campaign's RUN STATE (`GetCampaignState`,
/// `StartCampaign`, `PauseCampaign`, `StopCampaign`).
///
/// Also composed from the request, and deliberately NOT the sentence a campaign-resource
/// lookup answers ([`campaign_not_found_message`]): the service words the two classes
/// differently, so they are kept apart rather than unified. The one text those
/// operations share.
pub fn campaign_absent_message(id: &str) -> String {
    format!("Campaign: {id} doesn't exist")
}

/// The member one batch state read names its campaign ids in, as the clause below spells
/// it: the Smithy member name, lowerCamel-ed by the framework's own path spelling.
const CAMPAIGN_IDS_MEMBER: &str = "campaignIds";

/// The most campaign ids one batch state read may name.
///
/// The cap the SERVICE enforces, which is why it lives here and not in the model: a batch
/// of up to this many well-formed ids is PROCESSED — every id answered on its own,
/// resolved or not — and only a batch past it is refused. This service's Smithy model
/// therefore declares `GetCampaignStateBatchRequest$campaignIds` with a `@length` minimum
/// of one and no maximum, so the boundary validator still refuses the empty list while
/// the cap is applied by the handler, which is the only layer that holds the submitted
/// ids and can state them in the refusal ([`campaign_ids_over_cap_message`]).
pub const CAMPAIGN_IDS_PER_BATCH: usize = 500;

/// The longest `CampaignId` this service's model admits (`@length(min 0, max 256)`), which
/// the boundary validator applies to every entry of a batch before the handler runs.
const CAMPAIGN_ID_MAX_LENGTH: usize = 256;

/// How long a rendering of a submitted batch one clause quotes in FULL.
///
/// A MODEL constant, not a request-sized one: the longest list a batch one id past
/// [`CAMPAIGN_IDS_PER_BATCH`] can render as, every id at [`CAMPAIGN_ID_MAX_LENGTH`] and
/// separated by `, ` inside the brackets. So the refusal quotes in full every batch that is
/// an attempt at the cap — which is what AWS's own clause states — and a batch many times
/// larger than the cap is quoted as the shared module elides any value, rather than turning
/// a request into a response sized by it.
const MAX_QUOTED_CAMPAIGN_IDS: usize =
    (CAMPAIGN_IDS_PER_BATCH + 1) * (CAMPAIGN_ID_MAX_LENGTH + ", ".len()) + "[]".len();

/// What this service answers when one batch state read names more campaign ids than
/// [`CAMPAIGN_IDS_PER_BATCH`].
///
/// The envelope, the member path and the requirement come from the framework's own
/// constraint renderer, so a cap the service enforces is stated exactly as one the model
/// declares. The value half is the list the caller submitted, rendered the way a list
/// renders ([`service_common::validation::rendered_list`]) and held to
/// [`MAX_QUOTED_CAMPAIGN_IDS`] by the same elision the shared module applies to any value
/// ([`service_common::validation::quoted_echo_up_to`]).
pub fn campaign_ids_over_cap_message(campaign_ids: &[String]) -> String {
    let elements: Vec<&str> = campaign_ids.iter().map(String::as_str).collect();
    let submitted = ::service_common::validation::rendered_list(&elements);
    let violation = ::aws_smithy_schema_server::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(CAMPAIGN_IDS_MEMBER),
        kind: ::aws_smithy_schema_server::ViolationKind::MaxLength(CAMPAIGN_IDS_PER_BATCH as i64),
        value: Some(
            ::service_common::validation::quoted_echo_up_to(&submitted, MAX_QUOTED_CAMPAIGN_IDS)
                .into_owned(),
        ),
        sensitive: false,
    };
    // One violation always renders an envelope; the fallback keeps this infallible rather
    // than answering a refusal with no sentence at all.
    ::aws_smithy_schema_server::envelope(&[violation]).unwrap_or_default()
}

/// Why the Amazon Connect instance a request names is not a campaign target, as
/// [`connect_instance_refusal`] reports it: the shared gate's two outcomes
/// ([`::service_common::connect_instance_registry`]), which both ConnectCampaigns API
/// versions read the registry for and each answers with the error ITS own model names.
pub use ::service_common::connect_instance_registry::ConnectInstanceRefusal;

/// What this service answers when the Amazon Connect instance a request names is not one
/// Campaigns can be used with: the account holds no instance under that id, or the
/// instance it holds is not enabled for Campaigns.
///
/// A FIXED service string — it names no instance and carries nothing from the request —
/// so every refusal of this class reads the same and the response is deterministic. The
/// sentence itself is the shared gate's
/// ([`::service_common::connect_instance_registry::INSTANCE_NOT_USABLE_MESSAGE`]), stated
/// once for both API versions, and named here because this service's own
/// `ValidationException` (HTTP 400) is what carries it.
pub const INSTANCE_NOT_USABLE_MESSAGE: &str =
    ::service_common::connect_instance_registry::INSTANCE_NOT_USABLE_MESSAGE;

/// What this service answers when the Amazon Connect instance registry — another
/// service's store, reached through the shared engine — cannot be read at all, so the
/// instance question has no answer here. The shared gate's sentence for that outcome.
pub const INSTANCE_REGISTRY_UNAVAILABLE_MESSAGE: &str =
    ::service_common::connect_instance_registry::REGISTRY_UNAVAILABLE_MESSAGE;

/// Whether the Connect instance `connect_instance_id` names is one this service's
/// operations may be used with: `None` when it is, and otherwise which refusal the
/// caller owes.
///
/// The shared gate ([`::service_common::connect_instance_registry`]) over the registry
/// owner's own read: Amazon Connect mints and drops every instance id, so the answer comes
/// from THAT service's store through the one shared engine
/// (`::service_com_amazonaws_connect_amazonconnectservice::state::registry_holds_instance`)
/// rather than being kept a second time here — an instance created or deleted over the
/// Connect API is a campaign target, or not, with no propagation step.
///
/// The one place this service decides that gate, so the operations that require a usable
/// instance cannot disagree about a request they all refuse.
///
/// LOCK ORDER: the registry read takes the Connect store's lock, holds no other while it
/// does, and hands back an owned value rather than a guard — so this is called with NO
/// guard of this service's own store held. Keeping the cross-service read one-directional
/// and guard-free is what makes a lock cycle between the two stores unreachable.
pub fn connect_instance_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    connect_instance_id: &str,
) -> Option<ConnectInstanceRefusal> {
    ::service_common::connect_instance_registry::connect_instance_refusal(
        ::service_com_amazonaws_connect_amazonconnectservice::state::registry_holds_instance(
            engine,
            connect_instance_id,
        ),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>)
}
