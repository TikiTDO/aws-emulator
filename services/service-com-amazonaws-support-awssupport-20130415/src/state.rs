// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-support-awssupport-20130415 service.
//!
//! The support-case lifecycle lives here: [`Case`] is the primary resource and
//! [`CommunicationRecord`] is its correspondence log. `CreateCase`,
//! `AddCommunicationToCase` and `ResolveCase` write it; `DescribeCases` and
//! `DescribeCommunications` only read it, so every value they report is computed
//! from what an earlier call persisted.
//!
//! The account's Premium Support subscription lives here too
//! ([`ComAmazonawsSupportAwssupport20130415State::support_plan`]): every operation of
//! this API is served only to an account whose plan qualifies, which
//! [`ComAmazonawsSupportAwssupport20130415State::support_api_allowed`] answers and
//! `crate::service` enforces once, ahead of dispatch.
//!
//! Every growth site is BOUNDED, because every write here is driven by a request:
//! [`MAX_STORED_CASES`] caps how many cases the store holds (a further create is
//! refused with the operation's own modelled `CaseCreationLimitExceeded`),
//! [`MAX_COMMUNICATIONS_PER_CASE`] caps one case's log, evicting oldest-first
//! inside the single writer [`Case::record_communication_with_attachments`], and
//! [`MAX_ATTACHMENT_BYTES`] caps the file content every case retains together. A
//! case's cc list is capped the same way, at [`MAX_CC_EMAIL_ADDRESSES`] inside
//! [`Case::merge_cc_email_addresses`], which refuses a request from its length
//! before doing any work proportional to it.
//!
//! A case is added to the store in ONE place, [`ComAmazonawsSupportAwssupport20130415State::open_case`]:
//! `CreateCase` serves the account opening its own case, and a service opening one
//! on the account's behalf while serving a submission of its own goes through the
//! same writer, so every client reads back one case id form, one `displayId`
//! sequence and one correspondence log.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use sha2::{Digest, Sha256};

use crate::ops::types::{AttachmentDetails, Communication};

/// The status of a case AWS has accepted but not resolved: what `CreateCase`
/// records. One of the values `CaseDetails$status` documents.
pub const STATUS_OPENED: &str = "opened";

/// The status of a resolved case: what `ResolveCase` records. One of the values
/// `CaseDetails$status` documents.
pub const STATUS_RESOLVED: &str = "resolved";

/// The default `issueType` AWS records when `CreateCase` is called without one
/// (`CreateCaseRequest$issueType`: "If you don't specify a value, the default is
/// technical.").
pub const DEFAULT_ISSUE_TYPE: &str = "technical";

/// How many communications `CaseDetails$recentCommunications` carries:
/// "The five most recent communications associated with the case."
pub const RECENT_COMMUNICATIONS: usize = 5;

/// The widest page `maxResults` may ask for on `DescribeCases` /
/// `DescribeCommunications` (the modelled `MaxResults` range is 10..100), and so
/// the page size used when a caller asks for no particular one.
pub const MAX_PAGE_SIZE: usize = 100;

/// The `ccEmailAddresses` a case may carry: the modelled `CcEmailAddressList`
/// length constraint is 0..10, so a request whose addresses would take a case
/// past it is refused rather than partly applied.
pub const MAX_CC_EMAIL_ADDRESSES: usize = 10;

/// How many cases this emulator holds. Emulator-chosen (AWS publishes no case
/// quota through this API), and reported with `CreateCase`'s own modelled
/// `CaseCreationLimitExceeded`, which says exactly this: "The case creation limit
/// for the account has been exceeded." Every case is created by a request, so
/// without this bound a client could grow the process without limit.
pub const MAX_STORED_CASES: usize = 128;

/// How many communications one case retains. Emulator-chosen for the same
/// reason: `AddCommunicationToCase` appends a caller-supplied body (modelled up
/// to 8000 characters) per call and models no quota error, so
/// [`Case::record_communication`] drops the oldest once a case holds more than
/// this. With [`MAX_STORED_CASES`] that bounds the whole store at roughly
/// 128 * 32 * 8000 bytes.
pub const MAX_COMMUNICATIONS_PER_CASE: usize = 32;

/// How many bytes of attachment content this store retains, across every case.
///
/// Emulator-chosen for the same reason as the bounds above: an attachment is
/// caller-supplied content and nothing in this API removes a case, so without
/// this bound a client could grow the process without limit. A submission whose
/// files would take the store past it is refused whole
/// ([`CaseNotOpened::AttachmentStoreFull`]) rather than stored truncated, because
/// a truncated document is not the document that was submitted.
pub const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;

/// The Amazon Web Services Support plans an account may call this API with.
///
/// Source: this service's own documentation, carried on every operation header in
/// `src/ops/` (`smithy_query(query="doc",shape="AWSSupport_20130415")`): "You must
/// have an Amazon Web Services Business Support+, Amazon Web Services Enterprise
/// Support, or Amazon Web Services Unified Operations plan to use the Amazon Web
/// Services Support API. If you're in an Amazon Web Services Region that doesn't
/// offer one of these Amazon Web Services Support plans, or if you haven't
/// transitioned to one of these plans, you can use the Amazon Web Services Support
/// API with a Business, Enterprise On-Ramp, or Enterprise Support plan." Every plan
/// the sentence does not name — Basic and Developer — does NOT qualify, and the
/// documentation says what the service answers then: "the
/// SubscriptionRequiredException error message appears".
///
/// Entries are in the form [`normalize_support_plan`] produces, so a caller compares
/// through [`plan_qualifies`] rather than against this list directly.
pub const QUALIFYING_SUPPORT_PLANS: &[&str] = &[
    "business",
    "businesssupport+",
    "enterprise",
    "enterpriseonramp",
    "enterprisesupport",
    "unifiedoperations",
];

/// The environment variable an operator uses to tell this emulated account which
/// Support plan it holds, since the Support API models no operation that changes one
/// (`smithy_query(query="digest")` lists none): a plan name, read once when this
/// service's store is created (`SUPPORT_PLAN="enterprise"`).
///
/// UNSET means the account holds no Premium Support subscription, which is what a new
/// Amazon Web Services account has — and then every operation of this API answers the
/// documented subscription-required refusal rather than running.
pub const SUPPORT_PLAN_ENV: &str = "SUPPORT_PLAN";

/// A plan name reduced to what identifies the plan: ASCII lowercase, with every
/// separator and the vendor prefix the documentation writes dropped. So
/// `"Enterprise On-Ramp"`, `"enterprise_on_ramp"` and `"AWS Enterprise On-Ramp"` are
/// one plan, while `"Business Support+"` keeps the `+` that names it.
pub fn normalize_support_plan(plan: &str) -> String {
    let squeezed: String = plan
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '+')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let identifying = squeezed
        .strip_prefix("amazonwebservices")
        .or_else(|| squeezed.strip_prefix("aws"))
        .unwrap_or(squeezed.as_str());
    identifying.to_string()
}

/// Whether `plan` names one of the [`QUALIFYING_SUPPORT_PLANS`], i.e. whether an
/// account holding it may call this API at all.
pub fn plan_qualifies(plan: &str) -> bool {
    QUALIFYING_SUPPORT_PLANS.contains(&normalize_support_plan(plan).as_str())
}

/// The realm segment of a case id (`case-<account>-<realm>-<year>-<16 hex>`).
///
/// AWS documents the SHAPE of a case id but publishes no region-to-realm
/// mapping, so the emulator emits one fixed realm code rather than inventing a
/// mapping: the segment is opaque to every client, which only round-trips the
/// whole id.
const CASE_ID_REALM: &str = "muen";

/// Per-service state for com-amazonaws-support-awssupport-20130415. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsSupportAwssupport20130415State {
    /// Every case this emulator holds, keyed by its `caseId`. Bounded at
    /// [`MAX_STORED_CASES`]; `CreateCase` consults [`Self::is_full`] first.
    pub cases: HashMap<String, Case>,
    /// Monotonic counter backing each case's numeric `displayId`.
    pub next_display_id: u64,
    /// The Amazon Web Services Support plan this account holds, as
    /// [`SUPPORT_PLAN_ENV`] named it at start-up — `None` when nothing named one,
    /// which is an account with no Premium Support subscription.
    ///
    /// It is a field rather than a constant because the subscription is per account
    /// and nothing in THIS API changes it: a plan is bought outside the operations
    /// this model declares, so the only way one reaches the emulator is out of band.
    /// One field serves every caller because this emulator serves ONE account — the
    /// canonical emulator account (`engine::emulator_account_id()`) that every case id
    /// and `submittedBy` is built from. [`Self::support_api_allowed`] is what the
    /// dispatch path reads.
    pub support_plan: Option<String>,
}

impl Default for ComAmazonawsSupportAwssupport20130415State {
    /// An account with no cases and the Support plan [`SUPPORT_PLAN_ENV`] names
    /// (none, unless it is set — what a new Amazon Web Services account has).
    fn default() -> Self {
        Self {
            cases: HashMap::new(),
            next_display_id: 0,
            support_plan: ::std::env::var(SUPPORT_PLAN_ENV).ok(),
        }
    }
}

impl ComAmazonawsSupportAwssupport20130415State {
    /// Whether this account may call the Support API at all: whether the plan it
    /// holds is one of the [`QUALIFYING_SUPPORT_PLANS`]. False for an account with
    /// no plan recorded, and false for one holding a plan that does not qualify
    /// (Basic, Developer) — the account-level gate that fronts every operation of
    /// this service.
    pub fn support_api_allowed(&self) -> bool {
        self.support_plan.as_deref().is_some_and(plan_qualifies)
    }

    /// Whether the store already holds [`MAX_STORED_CASES`] cases, so a further
    /// `CreateCase` must be refused instead of growing it.
    pub fn is_full(&self) -> bool {
        self.cases.len() >= MAX_STORED_CASES
    }

    /// Take the next numeric `displayId`. Monotonic, so every case gets its own
    /// and the derived case id is unique too.
    pub fn allocate_display_id(&mut self) -> u64 {
        self.next_display_id += 1;
        self.next_display_id
    }

    /// How many bytes of attachment content the store retains, across every
    /// case: what [`MAX_ATTACHMENT_BYTES`] bounds.
    pub fn retained_attachment_bytes(&self) -> usize {
        self.cases
            .values()
            .flat_map(|case| case.communications.iter())
            .flat_map(|communication| communication.attachments.iter())
            .map(|attachment| attachment.data.len())
            .sum()
    }

    /// Open the case `submission` describes, as created at `time_created`, and
    /// answer its `caseId`.
    ///
    /// The ONE place a case is added to the store, whether the account opened it
    /// itself through `CreateCase` or a service opened it on the account's behalf
    /// while serving a submission of its own: both reach a case through here, so
    /// one case id form, one `displayId` sequence and one correspondence log are
    /// what every client reads back.
    ///
    /// Every bound is applied BEFORE anything is allocated or inserted, so a
    /// refused submission leaves the store exactly as it was — it mints no case
    /// id, consumes no `displayId` and retains none of its content. The caller
    /// renders the refusal with its own modelled error.
    pub fn open_case(
        &mut self,
        submission: CaseSubmission,
        time_created: String,
        time_created_instant: i128,
    ) -> Result<String, CaseNotOpened> {
        if self.is_full() {
            return Err(CaseNotOpened::StoreFull);
        }
        if submission.cc_email_addresses.len() > MAX_CC_EMAIL_ADDRESSES {
            return Err(CaseNotOpened::TooManyCcEmailAddresses(
                submission.cc_email_addresses.len(),
            ));
        }
        let submitted: usize = submission
            .attachments
            .iter()
            .map(|attachment| attachment.data.len())
            .sum();
        let retained = self.retained_attachment_bytes();
        if retained + submitted > MAX_ATTACHMENT_BYTES {
            return Err(CaseNotOpened::AttachmentStoreFull {
                submitted,
                retained,
            });
        }

        let display_id = self.allocate_display_id();
        let case_id = new_case_id(display_id, &time_created);
        let submitted_by = submitted_by();
        let mut case = Case::new(
            case_id.clone(),
            display_id,
            submission.subject,
            submission.service_code,
            submission.category_code,
            submission.severity_code,
            submission.issue_type,
            submission.language,
            submitted_by.clone(),
            time_created.clone(),
            time_created_instant,
        );
        case.merge_cc_email_addresses(&submission.cc_email_addresses)
            .map_err(CaseNotOpened::TooManyCcEmailAddresses)?;
        // The files the submission carries are attached to the body that
        // describes the issue — the case's first communication — under the ids a
        // client hands to `DescribeAttachment`.
        let attachments = submission
            .attachments
            .into_iter()
            .enumerate()
            .map(|(index, file)| StoredAttachment {
                attachment_id: new_attachment_id(&case_id, index, &file.file_name),
                file_name: file.file_name,
                data: file.data,
            })
            .collect();
        case.record_communication_with_attachments(
            submission.communication_body,
            submitted_by,
            time_created,
            time_created_instant,
            attachments,
        );
        self.cases.insert(case_id.clone(), case);
        Ok(case_id)
    }
}

/// A case to open: what the submission opening it says about the issue, plus the
/// files it submits with it.
///
/// `CreateCase` fills this from its own request; a service opening a case on the
/// account's behalf fills it from the submission it is serving.
#[derive(Debug, Clone, Default)]
pub struct CaseSubmission {
    /// `CaseDetails$subject`: the title of the case.
    pub subject: String,
    /// The body that describes the issue, which becomes the case's first
    /// communication.
    pub communication_body: String,
    pub service_code: Option<String>,
    pub category_code: Option<String>,
    pub severity_code: Option<String>,
    /// `customer-service` or `technical`; [`DEFAULT_ISSUE_TYPE`] when none is
    /// named.
    pub issue_type: Option<String>,
    pub language: Option<String>,
    pub cc_email_addresses: Vec<String>,
    /// The files to attach to that first communication. Each carries its name
    /// and content; the store mints the `attachmentId`.
    pub attachments: Vec<SubmittedFile>,
}

/// One file submitted with a case: its name and its content, before the store
/// has minted the id a client retrieves it by.
#[derive(Debug, Clone)]
pub struct SubmittedFile {
    pub file_name: String,
    pub data: Vec<u8>,
}

/// Why a case was NOT opened. Each caller renders it with the modelled error its
/// own operation declares, so the decision lives in one place and the wording
/// stays each operation's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseNotOpened {
    /// The store already holds [`MAX_STORED_CASES`] cases.
    StoreFull,
    /// The cc list carries more addresses than a case may hold
    /// ([`MAX_CC_EMAIL_ADDRESSES`]); the total it named.
    TooManyCcEmailAddresses(usize),
    /// The files submitted with the case would take the store past
    /// [`MAX_ATTACHMENT_BYTES`]: how many bytes they carry, and how many the
    /// store already retains.
    AttachmentStoreFull { submitted: usize, retained: usize },
}

/// One support case, as `CreateCase` persisted it and `DescribeCases` reports it.
#[derive(Debug, Clone)]
pub struct Case {
    pub case_id: String,
    pub display_id: String,
    pub subject: String,
    /// One of the values `CaseDetails$status` documents — `opened` on creation,
    /// `resolved` once `ResolveCase` has run.
    pub status: String,
    pub service_code: Option<String>,
    pub category_code: Option<String>,
    pub severity_code: Option<String>,
    pub issue_type: Option<String>,
    pub submitted_by: String,
    /// ISO 8601 instant the case was created, as reported on the wire.
    pub time_created: String,
    /// The same instant in whole nanoseconds: what ordering and the
    /// `afterTime`/`beforeTime` window compare, so neither depends on how the
    /// instant happens to be rendered.
    pub time_created_instant: i128,
    pub cc_email_addresses: Vec<String>,
    pub language: Option<String>,
    /// The correspondence log, oldest FIRST: the `CreateCase` body, then each
    /// `AddCommunicationToCase` body. Readers present it newest first. A
    /// `VecDeque`, so the oldest-first eviction at
    /// [`MAX_COMMUNICATIONS_PER_CASE`] is structural; write it only through
    /// [`Case::record_communication`], which applies that bound.
    pub communications: VecDeque<CommunicationRecord>,
    /// The sequence number the next communication gets. Monotonic per case and
    /// never reused, so a pagination cursor built from it survives both appends
    /// and the oldest-first eviction above.
    pub next_communication_sequence: u64,
}

impl Case {
    /// A new case with an empty correspondence log. The first communication is
    /// added with [`Case::record_communication`], exactly like a later one.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        case_id: String,
        display_id: u64,
        subject: String,
        service_code: Option<String>,
        category_code: Option<String>,
        severity_code: Option<String>,
        issue_type: Option<String>,
        language: Option<String>,
        submitted_by: String,
        time_created: String,
        time_created_instant: i128,
    ) -> Self {
        Self {
            case_id,
            display_id: display_id.to_string(),
            subject,
            status: STATUS_OPENED.to_string(),
            service_code,
            category_code,
            severity_code,
            issue_type: Some(issue_type.unwrap_or_else(|| DEFAULT_ISSUE_TYPE.to_string())),
            submitted_by,
            time_created,
            time_created_instant,
            cc_email_addresses: Vec::new(),
            language,
            communications: VecDeque::new(),
            next_communication_sequence: 0,
        }
    }

    /// The key the case list is ordered by: newest instant first, ties broken by
    /// `caseId`, so an order (and the pagination cursor built from it) is
    /// deterministic across calls.
    pub fn order_key(&self) -> (std::cmp::Reverse<i128>, String) {
        (
            std::cmp::Reverse(self.time_created_instant),
            self.case_id.clone(),
        )
    }

    /// Whether this case is resolved, i.e. whether `DescribeCases` hides it
    /// unless the caller passed `includeResolvedCases`.
    pub fn is_resolved(&self) -> bool {
        self.status == STATUS_RESOLVED
    }

    /// Append a communication, within [`MAX_COMMUNICATIONS_PER_CASE`]: once the
    /// case holds more than that, the oldest are dropped. The one writer of the
    /// log, and the only place a sequence number is minted.
    pub fn record_communication(
        &mut self,
        body: String,
        submitted_by: String,
        time_created: String,
        time_created_instant: i128,
    ) {
        self.record_communication_with_attachments(
            body,
            submitted_by,
            time_created,
            time_created_instant,
            Vec::new(),
        );
    }

    /// Append a communication carrying `attachments`, the files submitted with
    /// it: [`Self::record_communication`] with content, and the writer it
    /// delegates to, so the log's bound and its sequence numbering are applied
    /// once however a communication reaches a case.
    pub fn record_communication_with_attachments(
        &mut self,
        body: String,
        submitted_by: String,
        time_created: String,
        time_created_instant: i128,
        attachments: Vec<StoredAttachment>,
    ) {
        let sequence = self.next_communication_sequence;
        self.next_communication_sequence += 1;
        self.communications.push_back(CommunicationRecord {
            case_id: self.case_id.clone(),
            sequence,
            body,
            submitted_by,
            time_created,
            time_created_instant,
            attachments,
        });
        while self.communications.len() > MAX_COMMUNICATIONS_PER_CASE {
            self.communications.pop_front();
        }
    }

    /// Record `addresses` as case cc recipients, ignoring ones it already holds.
    /// `Err(total)` when the request would take the case past
    /// [`MAX_CC_EMAIL_ADDRESSES`], and then NOTHING is recorded: a request is
    /// refused rather than partly applied.
    ///
    /// The refusal comes FIRST, from the lengths alone, so the work this does is
    /// bounded by [`MAX_CC_EMAIL_ADDRESSES`] rather than by the size of the request:
    /// a caller-supplied list longer than a case may carry cannot fit however many
    /// duplicates it holds, so it costs one comparison instead of a clone and a
    /// membership scan per address. (The modelled `CcEmailAddressList` 0..10 length
    /// constraint refuses such a list at the boundary before it ever reaches here;
    /// this keeps the bound true for any other caller of this writer, and makes a
    /// list of duplicates past the modelled maximum refused rather than collapsed
    /// into an acceptable one.)
    pub fn merge_cc_email_addresses(&mut self, addresses: &[String]) -> Result<(), usize> {
        if addresses.len() > MAX_CC_EMAIL_ADDRESSES {
            return Err(addresses.len());
        }
        let mut merged = self.cc_email_addresses.clone();
        for address in addresses {
            if !merged.iter().any(|known| known == address) {
                merged.push(address.clone());
            }
        }
        if merged.len() > MAX_CC_EMAIL_ADDRESSES {
            return Err(merged.len());
        }
        self.cc_email_addresses = merged;
        Ok(())
    }
}

/// One communication on a case: the message body plus who submitted it and when.
#[derive(Debug, Clone)]
pub struct CommunicationRecord {
    pub case_id: String,
    /// Monotonic per case and never reused: the pagination cursor
    /// `DescribeCommunications` hands out.
    pub sequence: u64,
    pub body: String,
    pub submitted_by: String,
    /// ISO 8601 instant the communication was created, as reported on the wire.
    pub time_created: String,
    /// The same instant in whole nanoseconds: what the
    /// `afterTime`/`beforeTime` window compares.
    pub time_created_instant: i128,
    /// The files attached to this communication, as the submission that opened
    /// or added it carried them. Empty for a communication submitted without
    /// any — which is every one `AddCommunicationToCase` records, since the
    /// attachment-set operations are still scaffolded.
    pub attachments: Vec<StoredAttachment>,
}

impl CommunicationRecord {
    /// The modelled `Communication` for this record. `attachments` (every
    /// attachment) and `attachmentSet` (those 5 MB or smaller) report the same
    /// stored files, because every file this store retains is smaller than that:
    /// the one submission that attaches content carries a document the model
    /// bounds at 4 MiB.
    pub fn to_modeled(&self) -> Communication {
        let details: Vec<AttachmentDetails> = self
            .attachments
            .iter()
            .map(StoredAttachment::to_details)
            .collect();
        Communication {
            case_id: Some(self.case_id.clone()),
            body: Some(self.body.clone()),
            submitted_by: Some(self.submitted_by.clone()),
            time_created: Some(self.time_created.clone()),
            attachments: Some(details.clone()),
            attachment_set: Some(details),
        }
    }
}

/// One file attached to a case communication: the content as it was submitted,
/// under the id a client hands to `DescribeAttachment` and the file's own name.
#[derive(Debug, Clone)]
pub struct StoredAttachment {
    pub attachment_id: String,
    pub file_name: String,
    pub data: Vec<u8>,
}

impl StoredAttachment {
    /// The modelled `AttachmentDetails` for this file — "the file name and ID of
    /// an attachment to a case communication" — which is what the case readers
    /// report; the content itself is served by `DescribeAttachment`.
    pub fn to_details(&self) -> AttachmentDetails {
        AttachmentDetails {
            attachment_id: Some(self.attachment_id.clone()),
            file_name: Some(self.file_name.clone()),
        }
    }
}

/// The `afterTime`/`beforeTime` date window a reader was asked for, as instants.
#[derive(Debug, Clone, Copy, Default)]
pub struct DateWindow {
    pub after: Option<i128>,
    pub before: Option<i128>,
}

impl DateWindow {
    /// Whether `instant` is inside the window: `afterTime` is the start of the
    /// search and `beforeTime` its end, and both bounds are inclusive.
    pub fn contains(&self, instant: i128) -> bool {
        self.after.is_none_or(|after| instant >= after)
            && self.before.is_none_or(|before| instant <= before)
    }
}

/// Parse an `afterTime`/`beforeTime` pair. `Err(member)` names the member whose
/// value is not an ISO 8601 instant, so each operation can refuse it with its
/// own error channel — the window semantics stay in this one place.
pub fn parse_date_window(
    after: Option<&str>,
    before: Option<&str>,
) -> Result<DateWindow, &'static str> {
    Ok(DateWindow {
        after: parse_bound(after, "afterTime")?,
        before: parse_bound(before, "beforeTime")?,
    })
}

fn parse_bound(value: Option<&str>, member: &'static str) -> Result<Option<i128>, &'static str> {
    match value {
        None => Ok(None),
        Some(text) => parse_instant(text).map(Some).ok_or(member),
    }
}

/// The current time, as the ISO 8601 string the `TimeCreated` members carry plus
/// that same string's instant in whole nanoseconds — derived from the rendered
/// text, so what is stored and what a client can parse back always agree.
/// `None` only if the clock cannot be rendered, which a handler answers with
/// `InternalServerError`.
pub fn now() -> Option<(String, i128)> {
    let text = DateTime::from(SystemTime::now()).fmt(Format::DateTime).ok()?;
    let instant = parse_instant(&text)?;
    Some((text, instant))
}

/// Parse an ISO 8601 instant into whole nanoseconds since the epoch. `None` when
/// the text is not an ISO 8601 instant.
pub fn parse_instant(text: &str) -> Option<i128> {
    let parsed = DateTime::from_str(text, Format::DateTime).ok()?;
    Some(i128::from(parsed.secs()) * 1_000_000_000 + i128::from(parsed.subsec_nanos()))
}

/// The case id AWS hands back from `CreateCase`:
/// `case-<12-digit account>-<realm>-<year>-<16 hex>`.
///
/// The hex segment is the leading 8 bytes of a digest over the account, the
/// monotonic `displayId` and the creation instant, so it is opaque to a client
/// and unique per case (the `displayId` alone already makes every input
/// distinct).
pub fn new_case_id(display_id: u64, time_created: &str) -> String {
    let account = ::engine::emulator_account_id();
    let year: String = time_created.chars().take(4).collect();
    let digest = Sha256::digest(format!("{account}:{display_id}:{time_created}").as_bytes());
    let suffix = hex::encode(&digest[..8]);
    format!("case-{account}-{CASE_ID_REALM}-{year}-{suffix}")
}

/// The id of one attachment on a case communication.
///
/// The model constrains `AttachmentId` not at all and AWS publishes no format
/// for it — a client only round-trips it into `DescribeAttachment` — so this
/// mints an opaque id the same way a case id's segment is minted: a digest over
/// the case the file is attached to, its position in that attachment set and its
/// file name, which makes every input distinct and so every id unique.
pub fn new_attachment_id(case_id: &str, index: usize, file_name: &str) -> String {
    let digest = Sha256::digest(format!("{case_id}:{index}:{file_name}").as_bytes());
    format!("attachment-{}", hex::encode(&digest[..8]))
}

/// The identity recorded as `submittedBy` on a case and on every communication:
/// the email address of the account that submitted it. This emulator serves the
/// one canonical emulator account, so the address is derived from its id rather
/// than from anything in the request.
pub fn submitted_by() -> String {
    format!("{}@example.com", ::engine::emulator_account_id())
}

/// Wrap a pagination cursor as the opaque `nextToken` a caller hands back.
pub fn encode_token(cursor: &str) -> String {
    hex::encode(cursor)
}

/// Recover the cursor from a `nextToken`. `None` for a token this service did
/// not mint, which a handler refuses as a bad request.
pub fn decode_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSupportAwssupport20130415State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSupportAwssupport20130415State>)
}
