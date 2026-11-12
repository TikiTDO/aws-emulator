// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backupsearch-cryobackupsearchservice service.
//!
//! The service's primary resource is the SEARCH JOB: `StartSearchJob` mints one,
//! `GetSearchJob`/`ListSearchJobs`/`ListSearchJobResults`/`ListSearchJobBackups` report
//! it, `StopSearchJob` ends it, and `TagResource`/`ListTagsForResource` reach its tags
//! through its ARN.
//!
//! Its second resource is the EXPORT JOB ([`ExportJob`]): `StartSearchResultExportJob`
//! mints one for a search job whose results have settled, and
//! `GetSearchResultExportJob`/`ListSearchResultExportJobs` report exactly that record.
//! Every one of those answers is computed from the records held here.
//!
//! BOTH are taggable resources of this service in their own right — each is minted by an
//! operation taking a `TagMap` and each ARN is an `@arnReference` to this service — so
//! the tag APIs resolve their `ResourceArn` against every record held here
//! ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::tags_by_arn`]), not against
//! one resource type.
//!
//! ## The clock, not the reader, moves a status
//!
//! A search job AWS returns is `CREATING` — a non-terminal status — so the service owes
//! it a later status without anybody calling again, and then another: a scan is set up
//! (`CREATING`), runs (`RUNNING`) and ends (a terminal status). Those owed transitions
//! are stored as DUE INSTANTS ([`SearchJob::owed_statuses`]) and materialised by
//! [`ComAmazonawsBackupsearchCryobackupsearchserviceState::settle`], which every handler
//! calls first: a read therefore reports what the clock has already made due and never
//! advances a job because it was polled.
//!
//! The clock drives one more thing a caller can observe: a `ClientToken`'s validity
//! window ([`CLIENT_TOKEN_VALIDITY`], [`ClientToken`]). A token takes part in idempotency
//! only while that window is open, so the same `settle` that materialises a due status
//! drops a token the clock has carried past it — and the next call under that token is a
//! new request rather than a repeat or a conflict.
//!
//! WHICH status that is follows from the job's resolved scope, not from the operation
//! that started it: [`scan_outcome`] turns the recovery points a scope resolved to
//! ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::recovery_points_in_scope`])
//! into the terminal status the scan is owed, and the `StatusMessage` that status comes
//! with travels with it — including the one a scan that got through only PART of the scope
//! it covered has to explain itself with.
//!
//! ## Two layers: backups, and the items inside them
//!
//! A scan resolves a `SearchScope` to recovery points — the backups AWS Backup holds for
//! this account, read from that component's own store ([`indexed_recovery_points`]), so a
//! backup a client created through Backup's API is searchable on the next call — and then
//! gets through the ITEMS their backup indexes hold ([`IndexedRecoveryPoint`]) — the
//! objects of a backed-up
//! bucket, the files of a backed-up volume. That inner layer is what a job's `ItemFilters`
//! select from ([`item_matches_filters`]), what `TotalItemsToScanCount` and
//! `ItemsScannedCount` count ([`scanned_items`]) and what `ListSearchJobResults` pages, so
//! every one of those answers is counted from the records this store holds rather than
//! assumed by a handler.
//!
//! A covered recovery point only reaches that inner layer when its backup index was in a
//! state a search could read ([`index_searchable`]): one that was not takes no part in the
//! search — nothing of it is scanned, counted or matched — and is DISPLAYED, with the
//! status and status message saying why, by the listing that pages
//! [`SearchJob::backups`]. A scan that left one of its covered recovery points out that
//! way did not get through the scope it covered, so the terminal status it settles to
//! carries the `StatusMessage` saying so ([`scan_outcome`]).
//!
//! ## An export produces an object, in another service
//!
//! An export job is not finished by its own record: `StartSearchResultExportJob` exports a
//! search job's results "to a designated S3 bucket", so the results are WRITTEN into the
//! bucket the export names, and the status the clock owes the export is what that write came
//! to ([`export_delivery`]). The destination therefore lives where the object store holds
//! it, and a bucket nothing holds fails the export rather than being reported as exported
//! to.

/// Writing a search job's results into the destination bucket an export names, which is
/// the work the export performs.
mod export_delivery;
pub use export_delivery::*;

/// How a refusal another service answered is reported to this service's caller: the
/// downstream reason inside the envelope naming the call it was relayed from.
mod relayed_refusal;
pub use relayed_refusal::*;

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    BackupCreationTimeFilter, ConflictException, CurrentSearchProgress, EBSItemFilter,
    EBSResultItem, ExportSpecification, ItemFilters, LongCondition,
    ResultItem, S3ItemFilter, S3ResultItem, SearchJobBackupsResult, SearchScope,
    SearchScopeSummary, ServiceQuotaExceededException, StringCondition, TimeCondition,
};

/// Where the recovery points a `SearchScope` is resolved against come from: the AWS
/// Backup component of this emulator, which is what holds this account's backups.
mod backup_plane;
pub use backup_plane::*;

/// The HTTP status a successful call of this service that has NO content to report
/// answers with.
///
/// The three tag operations all leave their success code DEFAULTED in the model (`@http`
/// carries no `code:`, i.e. `smithy_query(query="operation", operation="TagResource")`
/// reports `code: 200, codeDefaulted: true`), so the Smithy default of 200 is not a
/// statement about this service: the status is a behavioural fact and the recorded AWS
/// responses are what state it. Of those three, the two whose output is a declared
/// structure with NO members — `TagResourceResponse` and `UntagResourceResponse` — have
/// no content to parse and are answered with 204 No Content. A 200 there promises a body
/// the response does not carry, so every client path gated on 204 (conditional body
/// parsing, caches and proxies that key on it) takes the wrong branch.
///
/// The third, `ListTagsForResource`, keeps the defaulted 200: its output declares `Tags`,
/// so it HAS content to report. Nor does this extend to `StopSearchJob`, whose output
/// structure is just as empty but whose model carries an EXPLICIT `@http(code: 200)` (no
/// `codeDefaulted`): there the model states the status, so it keeps 200.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// The `SearchJobState` values the model declares, so no handler spells one as a bare
/// literal. The full set is `ops::types::SEARCH_JOB_STATE_VALUES`.
pub const RUNNING: &str = "RUNNING";
/// The status a search job the service has accepted but not yet started scanning for is
/// in — the FIRST status any caller can observe after `StartSearchJob`, before
/// [`RUNNING`].
///
/// This value is NOT in the model's `SearchJobState` enum
/// (`ops::types::SEARCH_JOB_STATE_VALUES`), and that is not a contradiction: restJson1
/// enums are open on the wire, and the service's own recordings report a just-started
/// search job with this status — the status-parameterised refusal an export started
/// back-to-back with `StartSearchJob` is answered with names it. A client reading the
/// status as a string therefore sees the setting-up phase it sees on AWS; one that
/// matches only the closed enum treats it as an unknown variant, exactly as it would
/// against AWS.
///
/// It is a TRANSIENT status (it is not in [`TERMINAL_STATES`]) and the clock owes the job
/// [`RUNNING`] after [`CREATING_DELAY`], so no caller is left in it.
pub const CREATING: &str = "CREATING";
/// See [`RUNNING`].
pub const COMPLETED: &str = "COMPLETED";
/// See [`RUNNING`].
pub const STOPPING: &str = "STOPPING";
/// See [`RUNNING`].
pub const STOPPED: &str = "STOPPED";
/// See [`RUNNING`].
pub const FAILED: &str = "FAILED";

/// The statuses a record of this service never leaves: a scan that ended, a stop that
/// took effect, or a scan or an export that failed. `CREATING`, `RUNNING` and `STOPPING`
/// are the transient ones, as is the `CREATED` an export job's own status vocabulary
/// declares (`smithy_query(query="shape", shape="ExportJobSummary")`).
///
/// It governs BOTH record types, because it is what decides whether a transition the
/// clock materialises writes a `CompletionTime` ([`settle_owed_statuses`]): the terminal
/// statuses of an export job's vocabulary are spelled by the same two constants as a
/// search job's.
pub const TERMINAL_STATES: &[&str] = &[COMPLETED, STOPPED, FAILED];

/// The status a recovery point's record reports for its backup INDEX while that index is
/// built and readable — the state an index has to be in for the backup to take part in a
/// search at all (`smithy_query(query="doc", shape="ListSearchJobBackups")`: only a
/// recovery point whose backup index has a status of `ACTIVE` is included in search
/// results, and an index in any other status is merely displayed with its status and a
/// status message).
///
/// This value is not in the model's `SearchJobState` enum, which is the type
/// `SearchJobBackupsResult$Status` carries
/// (`smithy_query(query="shape", shape="SearchJobBackupsResult")`), and that is not a
/// contradiction: restJson1 enums are open on the wire, so the index vocabulary the
/// operation's documentation uses can reach this store as it stands, while a record that
/// reports a built index in the member's own vocabulary spells it [`COMPLETED`], the
/// terminal success of that enum. [`SEARCHABLE_INDEX_STATUSES`] holds both spellings.
pub const INDEX_ACTIVE: &str = "ACTIVE";

/// The backup-index statuses that put a recovery point INSIDE a search — the two
/// spellings of a built, readable index ([`INDEX_ACTIVE`]) and nothing else, because every
/// other status (an index that failed, one still being built, one being deleted, or one a
/// record does not report at all) says the index could not be read.
pub const SEARCHABLE_INDEX_STATUSES: &[&str] = &[INDEX_ACTIVE, COMPLETED];

/// The comparison an item-filter condition states, as the model's three condition-operator
/// enums declare them (`types::STRING_CONDITION_OPERATOR_VALUES`,
/// `types::LONG_CONDITION_OPERATOR_VALUES`, `types::TIME_CONDITION_OPERATOR_VALUES`), so
/// no comparison spells one as a bare literal. This one and [`NOT_EQUALS_TO`] are declared
/// by all three; the orderings by the numeric and time enums, and the substring ones by the
/// string enum. `EQUALS_TO` is also the `@default` every `Operator` member carries.
pub const EQUALS_TO: &str = "EQUALS_TO";
/// See [`EQUALS_TO`].
pub const NOT_EQUALS_TO: &str = "NOT_EQUALS_TO";
/// See [`EQUALS_TO`].
pub const LESS_THAN_EQUAL_TO: &str = "LESS_THAN_EQUAL_TO";
/// See [`EQUALS_TO`].
pub const GREATER_THAN_EQUAL_TO: &str = "GREATER_THAN_EQUAL_TO";
/// See [`EQUALS_TO`].
pub const CONTAINS: &str = "CONTAINS";
/// See [`EQUALS_TO`].
pub const DOES_NOT_CONTAIN: &str = "DOES_NOT_CONTAIN";
/// See [`EQUALS_TO`].
pub const BEGINS_WITH: &str = "BEGINS_WITH";
/// See [`EQUALS_TO`].
pub const DOES_NOT_BEGIN_WITH: &str = "DOES_NOT_BEGIN_WITH";
/// See [`EQUALS_TO`].
pub const ENDS_WITH: &str = "ENDS_WITH";
/// See [`EQUALS_TO`].
pub const DOES_NOT_END_WITH: &str = "DOES_NOT_END_WITH";

/// The statuses `StopSearchJob` is already satisfied by.
///
/// The operation is `@idempotent` and asks for ONE end state: the job stopped. A job
/// that is `STOPPED` is in that state and one that is `STOPPING` is on its way to it, so
/// a repeat has nothing left to do and answers the empty 200 the model declares. Any
/// other status is judged by [`SearchJob::is_stoppable`].
pub const STOP_SATISFIED_STATES: &[&str] = &[STOPPING, STOPPED];

/// The message this service refuses a stop of a job it cannot stop with.
///
/// Recorded behaviour, not modelled: the model states no text for `ConflictException`, so
/// the service's own wording is the requirement — ONE fixed sentence whose only variable
/// part is the state the job is in, which is read from the record rather than from the
/// request.
pub fn stop_conflict_message(status: &str) -> String {
    format!("SearchJob can not be stopped since it is in {status} state. Please try again later")
}

/// The search-job statuses an export may be started from.
///
/// An export transmits the RESULTS of a search job, so the scan has to have finished
/// producing them: a search job still scanning (`RUNNING`) or still winding down
/// (`STOPPING`) has no settled result set to export, and `FAILED` is terminal but
/// produced none. The service's own recordings confirm this set — an export started for
/// a search job that ended `FAILED` is refused — and a start from any other status is
/// refused rather than accepted into a success nothing could back.
pub const EXPORTABLE_SEARCH_JOB_STATES: &[&str] = &[COMPLETED, STOPPED];

/// The message this service refuses an export of a search job whose results have not
/// settled with.
///
/// Recorded behaviour, not modelled: the model states no text for the refusal, so the
/// service's own wording is the requirement — ONE fixed sentence naming the statuses an
/// export is supported from ([`EXPORTABLE_SEARCH_JOB_STATES`], comma-separated as the
/// service lists them) and the status the job is in, read from the record rather than from
/// the request. It names no identifier: the refusal is about the status, and the caller's
/// fix is to poll and retry, which is what the closing clause says.
pub fn export_status_refusal_message(status: &str) -> String {
    format!(
        "Export is only supported for search job with {} statuses. SearchJob has status \
         {status}. Please try again later",
        EXPORTABLE_SEARCH_JOB_STATES.join(",")
    )
}

/// How long after the call that starts it the clock owes the next status of a transition
/// between two PHASES of a record: `CREATING` -> `RUNNING` after `StartSearchJob`
/// ([`CREATING_DELAY`]), `STOPPING` -> `STOPPED` after `StopSearchJob`, and an export
/// job's `RUNNING` -> `COMPLETED` after `StartSearchResultExportJob`.
///
/// AWS documents no duration for any of them, so this span is CHOSEN, not modelled: a few
/// seconds, which is this emulator's span for a transition the clock owes.
///
/// WHICH few seconds is decided by the client that made the call: the phase a call puts a
/// record in has to still be there when that client's NEXT call arrives, and the gap
/// between two calls of one client is not a round trip — it is that client's own work, its
/// intervening calls, its SDK retries and a loaded host, which is seconds and not
/// milliseconds. So this span sits at the long end of the few seconds the emulator gives a
/// transition it chooses (it replaces a sub-second span this crate used, which a single
/// round trip outran), while staying short enough that the phase is a status a poller
/// passing through observes rather than one it waits out. Nothing sleeps for it: the due
/// instant is stored and compared against the clock.
///
/// It is NOT the span a scan's own run takes, which is work and not a phase change: see
/// [`SCAN_DELAY`].
pub const TRANSITION_DELAY: Duration = Duration::from_secs(5);

/// How long after `StartSearchJob` the clock owes the job [`RUNNING`], i.e. how long it
/// sits in [`CREATING`] first.
///
/// The set-up phase is a transition the clock owes like any other, so it lasts one
/// [`TRANSITION_DELAY`]: a caller acting straight after the start call observes the job
/// being set up, and a caller one span later observes its scan running.
pub const CREATING_DELAY: Duration = TRANSITION_DELAY;

/// How long a search job's scan RUNS: the span between the instant its [`RUNNING`] phase
/// falls due and the instant the clock owes it the terminal status [`scan_outcome`]
/// decided for it.
///
/// CHOSEN rather than modelled — AWS documents no duration for a scan — but deliberately
/// NOT the same span as [`TRANSITION_DELAY`], because what it stands for is not a record
/// moving from one phase to the next: it is the WORK of getting through a scope, which
/// this service runs for minutes and longer. Compressing that work is what makes the
/// emulator usable, and the compression has two sides a client can see, which together
/// decide the span:
///
/// * while the scan runs the job is in a status a cancel acts on, and that is the ONLY
///   window `StopSearchJob` has — past it the service answers the conflict it refuses a
///   job whose scan has ended with ([`stop_conflict_message`]). So the scan has to still
///   be running when a client that started a job comes back to cancel it, and that client
///   comes back after its own intervening calls, its retries and a loaded host: tens of
///   seconds, not milliseconds. Sharing the one-phase-to-the-next span above left that
///   window shorter than the gap, so a cancel the service accepts was refused.
/// * a client that polls for the scan's terminal status has to REACH it, and a poll loop
///   gives up: about a minute is what a client waits for an asynchronous status before it
///   reports the wait as failed. So this span stays well inside that — half a minute — and
///   a poll begun while the job is still being set up still converges on the status the
///   scan ended in.
///
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const SCAN_DELAY: Duration = Duration::from_secs(25);

/// How long a `ClientToken` of this service takes part in idempotency, measured from
/// the moment the request that first used it completed.
///
/// MODELLED rather than chosen: the member's own documentation on BOTH operations that
/// take one states the window and what happens after it — a token is valid for eight
/// hours after the first request using it completed, and any later request carrying it
/// is treated as a new request
/// (`smithy_query(query="doc", shape="StartSearchJobInput", member="ClientToken")`,
/// same text on `StartSearchResultExportJobInput`). So this is not one of the emulator's
/// own spans: it is the duration the service publishes.
pub const CLIENT_TOKEN_VALIDITY: Duration = Duration::from_secs(8 * 60 * 60);

/// The instant `span` after `base`, computed FALLIBLY: every clock instant this service
/// stores — the instants a status is owed from ([`SearchJob::owed_statuses`],
/// [`ExportJob::next_status_due`]) and the instant a [`ClientToken`]'s window closes —
/// goes through here.
///
/// Adding a `Duration` to a `SystemTime` with `+` PANICS when the sum is not
/// representable, and a panic is not an answer this service can give: a handler that
/// aborts leaves the caller with no modelled response at all. So a sum the platform
/// cannot represent is taken as the span having ALREADY elapsed — the base itself —
/// which for an owed status means the transition is due at once and the next call
/// materialises it, and for a token window means the window is already closed and the
/// next call under that token is a new request. Either way the request is answered.
///
/// Under any clock whose sum is representable this is exactly `base + span`, so nothing
/// a caller can observe changes.
pub fn due_at(base: SystemTime, span: Duration) -> SystemTime {
    base.checked_add(span).unwrap_or(base)
}

/// A `ClientToken` a request carried, together with the instant its validity window
/// closes ([`CLIENT_TOKEN_VALIDITY`] after the request that used it completed).
///
/// The token and its window are ONE value, so no record can hold a token nothing
/// bounds: a stored token answers a repeat only while its window is open, and once the
/// clock has passed it the token names no earlier request at all — a call carrying it is
/// a new request, neither a repeat nor a conflict.
#[derive(Debug, Clone)]
pub struct ClientToken {
    value: String,
    expires_at: SystemTime,
}

impl ClientToken {
    /// The token `value` carried by a request that completed at `completed`, valid until
    /// [`CLIENT_TOKEN_VALIDITY`] after that instant.
    ///
    /// The window's end is computed fallibly through [`due_at`], like every other clock
    /// instant this store holds: an unrepresentable sum is a window that has ALREADY
    /// closed, so a clock far enough ahead makes the next call under the token a new
    /// request rather than aborting the handler.
    pub fn minted(value: String, completed: SystemTime) -> Self {
        Self {
            value,
            expires_at: due_at(completed, CLIENT_TOKEN_VALIDITY),
        }
    }

    /// The token as the request spelled it.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether this token still takes part in idempotency at `now`. The window is open
    /// up to but NOT including the instant it closes, so a token exactly its full
    /// validity old is already past it.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }

    /// Whether this token is `value` AND still inside its window — which is what makes
    /// the record carrying it the answer to a repeat rather than an unrelated record.
    pub fn is_live(&self, value: &str, now: SystemTime) -> bool {
        self.value == value && self.is_valid_at(now)
    }
}

/// Drop a record's `ClientToken` once the clock has carried it past its validity window.
///
/// Materialising a change the CLOCK already made due, like every other transition this
/// store settles: the window closed at an instant that was reached whether or not
/// anybody called, and from then on the record no longer holds a token for anything to
/// match. Clearing it is what keeps a token from naming one job for the life of the
/// process.
fn expire_client_token(token: &mut Option<ClientToken>, now: SystemTime) {
    if token.as_ref().is_some_and(|held| !held.is_valid_at(now)) {
        *token = None;
    }
}

/// One transition the clock owes a record: the instant it is owed FROM, the status it
/// moves the record to, and the `StatusMessage` that status comes with (`None` for one
/// that needs no message).
///
/// The message is OWNED rather than a fixed sentence, because a status the service has to
/// explain can need text counted from the record itself ([`scan_outcome`]) and not only
/// one of this module's templates.
pub type OwedTransition = (SystemTime, &'static str, Option<String>);

/// The members one owed transition writes, borrowed from whichever record holds them.
///
/// What lets [`settle_owed_statuses`] write them ONCE for every record type of this store
/// instead of once per record type.
#[derive(Debug)]
pub struct StatusMembers<'a> {
    /// The status the record reports.
    pub status: &'a mut String,
    /// The `StatusMessage` that status comes with.
    pub status_message: &'a mut Option<String>,
    /// The instant the record reached a status it never leaves.
    pub completion_time: &'a mut Option<DateTime>,
}

/// A record of this store whose status the CLOCK moves: the service owes it a later
/// status without anybody calling again, and it can hold a `ClientToken` whose validity
/// window the same clock closes.
///
/// Both record types this service persists are one ([`SearchJob`] and [`ExportJob`]), and
/// both are settled by the single step below rather than by a copy of it each: a record
/// states only WHERE its token, its owed transitions and its status members live, plus
/// whatever else reaching a status does to it.
pub trait OwedStatusRecord {
    /// The `ClientToken` the request that minted this record carried, if any.
    fn client_token_mut(&mut self) -> &mut Option<ClientToken>;

    /// The next transition the clock owes this record, WITHOUT dropping it — `None` for a
    /// record that is owed nothing more.
    fn owed_transition(&self) -> Option<OwedTransition>;

    /// Drop the transition [`OwedStatusRecord::owed_transition`] reported, now that it has
    /// been materialised.
    fn drop_owed_transition(&mut self);

    /// Where this record's status, that status's message and its completion time live.
    fn status_members(&mut self) -> StatusMembers<'_>;

    /// Whatever else reaching `status` does to THIS record, applied once the members above
    /// are written. Nothing by default: for a record whose status is all a transition
    /// moves, there is nothing else to write.
    fn on_status_reached(&mut self, _status: &'static str) {}
}

/// Materialise on `record` every transition the clock has already made due, in order — so
/// a record nobody looked at while two of its spans passed reports the later status
/// rather than catching up one call at a time — and drop a `ClientToken` the clock has
/// carried past its window.
///
/// THE one place this store's owed-transition rule is written: every record type goes
/// through it, so the rule cannot be spelled one way for one record type and another way
/// for the next.
pub fn settle_owed_statuses<R: OwedStatusRecord>(record: &mut R, now: SystemTime) {
    expire_client_token(record.client_token_mut(), now);
    while let Some((due, next, message)) = record.owed_transition() {
        if now < due {
            return;
        }
        record.drop_owed_transition();
        let members = record.status_members();
        *members.status = next.to_string();
        // The status and the message that status comes with are written together, so a
        // record can never explain a failure it has not reached, nor reach one silently.
        *members.status_message = message;
        // `CompletionTime` is when the record COMPLETED, so only a status it never leaves
        // sets it: passing through a transient status completes nothing.
        if TERMINAL_STATES.contains(&next) {
            *members.completion_time = Some(timestamp(due));
        }
        record.on_status_reached(next);
    }
}

/// A record a `ClientToken` of this service can name — what [`client_token_replay`] reads
/// off it, and the resource type a refusal about it names.
///
/// Implemented by the record of each operation that takes a `ClientToken` ([`SearchJob`]
/// and [`ExportJob`]), which is what lets the replay rule below be written ONCE for both.
pub trait TokenedRecord {
    /// The `resourceType` a refusal about this kind of record names
    /// ([`SEARCH_JOB_RESOURCE`], [`EXPORT_JOB_RESOURCE`]).
    const RESOURCE_TYPE: &'static str;

    /// The `ClientToken` the request that minted this record carried, if any, with the
    /// instant its validity window closes.
    fn held_client_token(&self) -> Option<&ClientToken>;

    /// The canonical form of that request's body ([`request_fingerprint`]), which is what
    /// tells a repeat of the same body from a different body under the same token.
    fn held_fingerprint(&self) -> &str;

    /// This record's identifier, which a refusal names as the resource in conflict.
    fn record_identifier(&self) -> &str;
}

/// The record holding `token` as a token still inside its validity window at `now`, or
/// `None` when no held record does.
///
/// ONE live-token lookup behind every `ClientToken` of this service, so the two `Start`
/// operations cannot drift apart on which held records a token is matched against or on
/// whether a closed window still matches. A token whose window has closed matches NOTHING
/// here, so the call carrying it is a new request. Read off the records themselves, so the
/// idempotency index cannot outgrow the store it indexes. At most one held record can
/// carry a live token of a given value, because minting another under it requires that no
/// live one exists.
fn live_token_holder<'a, T: TokenedRecord>(
    records: &'a HashMap<String, T>,
    token: &str,
    now: SystemTime,
) -> Option<&'a T> {
    records.values().find(|record| {
        record
            .held_client_token()
            .is_some_and(|held| held.is_live(token, now))
    })
}

/// The REPLAY RULE every `ClientToken` of this service follows, in its three steps: find
/// a held record carrying this token still inside its validity window; answer that stored
/// record when the request's fingerprint matches it; refuse with a conflict when it does
/// not.
///
/// One rule behind both `Start` operations of this service — the model states the same
/// window sentence on both `ClientToken` members — so which step runs first, what a
/// non-matching fingerprint answers and whether a closed window falls through are decided
/// here once instead of per handler. The operation named in the refusal and the record
/// type it is about ([`TokenedRecord::RESOURCE_TYPE`]) are parameters of the refusal, not
/// of the rule.
///
/// `Ok(None)` is a NEW request — no token, or none the store holds live — and the caller
/// goes on to mint a record. `Ok(Some(record))` is a repeat, which the caller answers out
/// of that stored record and creates nothing. `Err` is the conflict, which each caller
/// lifts into its own modelled error channel.
pub fn client_token_replay<'a, T: TokenedRecord>(
    records: &'a HashMap<String, T>,
    token: Option<&str>,
    fingerprint: &str,
    now: SystemTime,
    operation: &str,
) -> Result<Option<&'a T>, ConflictException> {
    let Some(token) = token else {
        return Ok(None);
    };
    let Some(record) = live_token_holder(records, token, now) else {
        return Ok(None);
    };
    if record.held_fingerprint() != fingerprint {
        return Err(ConflictException {
            message: format!(
                "ClientToken '{token}' was already used for a different {operation} request"
            ),
            resource_id: record.record_identifier().to_string(),
            resource_type: T::RESOURCE_TYPE.to_string(),
        });
    }
    Ok(Some(record))
}

/// A record kind this store CAPS — the parameters the quota refusal below reads off it.
///
/// Implemented by the record of each operation that grows a capped store of this service
/// ([`SearchJob`] and [`ExportJob`]), which is what lets the cap rule be written ONCE for
/// both. The resource type a refusal names is the one [`TokenedRecord`] already states for
/// this kind of record, so the two refusals cannot disagree about it.
pub trait CappedRecord: TokenedRecord {
    /// How many records of this kind this store holds at most ([`MAX_SEARCH_JOBS`],
    /// [`MAX_EXPORT_JOBS`]).
    const STORE_CAP: usize;

    /// The `quotaCode` a refusal about this kind of record names. AWS publishes no quota
    /// code for either cap, so the emulator names the quota it enforces, by one
    /// convention for both kinds: `<Kind>JobsPerAccount`.
    const QUOTA_CODE: &'static str;

    /// The word naming this kind of job in the cap sentence, as the refusal spells it.
    const JOB_KIND: &'static str;
}

/// The CAP RULE every capped store of this service follows: a store already holding its
/// maximum number of records refuses the call that would add another with the service's
/// own modelled `ServiceQuotaExceededException` (HTTP 402), whose members name the account
/// the quota is counted against, the kind of record it caps, the service enforcing it and
/// the quota itself.
///
/// One rule behind both `Start` operations of this service — both stores hold their
/// records for the life of the process and nothing evicts one — so the predicate, which
/// member carries which value and the wording of the cap sentence are decided here once
/// instead of per handler. The cap, the record type, the quota code and the job-kind word
/// are parameters of the refusal ([`CappedRecord`]), not of the rule.
///
/// `Ok(())` means the call may mint a record. `Err` is the refusal, which each caller
/// lifts into its own modelled error channel. `service_code` is the signing name of the
/// service the request reached, which each handler takes from its own request.
pub fn check_store_capacity<T: CappedRecord>(
    records: &HashMap<String, T>,
    service_code: &str,
) -> Result<(), ServiceQuotaExceededException> {
    if records.len() < T::STORE_CAP {
        return Ok(());
    }
    Err(ServiceQuotaExceededException {
        message: format!(
            "this account already holds the maximum of {} {} jobs",
            T::STORE_CAP,
            T::JOB_KIND
        ),
        resource_id: ::engine::emulator_account_id().to_string(),
        resource_type: T::RESOURCE_TYPE.to_string(),
        service_code: service_code.to_string(),
        quota_code: T::QUOTA_CODE.to_string(),
    })
}

/// `MaxResults`'s modelled `@default(1000)`, shared by `ListSearchJobs` and
/// `ListSearchJobResults`: the page size to serve when the member is absent.
pub const DEFAULT_MAX_RESULTS: usize = 1000;

/// The upper bound of `MaxResults`'s modelled `@range(min: 1, max: 1000)`. Kept apart
/// from [`DEFAULT_MAX_RESULTS`] because the model states them separately and either one
/// can move without the other; a page is clamped to THIS, so a response cannot be
/// amplified past what the model allows even if a request slips the range check.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The ceiling on how many search jobs this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process, and
/// `StartSearchJob` is the only thing that adds one, so the store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because `GetSearchJob`
/// must still answer what `StartSearchJob` created. The operation that grows the store
/// refuses past this cap with the service's own modelled `ServiceQuotaExceededException`
/// (HTTP 402), which the model declares on exactly the two `Start` operations and on no
/// read. The number is the emulator's own bound, not a published AWS default — AWS
/// publishes the search-job quota as adjustable, so no single value is the contract —
/// and it is set high enough that no ordinary workflow meets it.
pub const MAX_SEARCH_JOBS: usize = 5_000;

/// The ceiling on how many tags one search job carries, with the caps on one tag's key
/// and value. The model puts no length or count constraint on `TagMap`, so these are the
/// emulator's own bound on caller-driven growth of a stored record, set at the limits AWS
/// applies to resource tags generally (50 tags, a 128-character key, a 256-character
/// value). `StartSearchJob` and `TagResource` refuse a tag set past them with a
/// `ValidationException` naming the member and the bound.
pub const MAX_TAGS_PER_JOB: usize = 50;

/// See [`MAX_TAGS_PER_JOB`].
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// See [`MAX_TAGS_PER_JOB`].
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The punctuation a tag key or value may carry besides letters, numbers and spaces.
///
/// Source: the `Tags` member documentation of every operation of this service that takes
/// a `TagMap` — `smithy_query(query="doc", shape="TagResourceRequest", member="Tags")`,
/// and the same sentence on `StartSearchJobInput$Tags` and
/// `StartSearchResultExportJobInput$Tags` — which names the characters a tag of this
/// service is allowed to use. The model puts no `@pattern` on `TagMap`, so the boundary
/// validator cannot refuse a tag outside that set and the service enforces it itself
/// ([`check_tag_entry`]).
pub const TAG_PUNCTUATION: &[char] = &['+', '-', '=', '.', '_', ':', '/'];

/// How a refusal spells the set [`TAG_PUNCTUATION`] belongs to, so one validation message
/// describes the whole constraint and the caller can fix the request from it alone.
pub const ALLOWED_TAG_CHARACTERS: &str =
    "UTF-8 letters, numbers, spaces, and the following characters: + - = . _ : /";

/// The ARN resource segment of a search job: `search-job/<SearchJobIdentifier>`.
pub const SEARCH_JOB_RESOURCE: &str = "search-job";

/// How many times an identifier mint draws a UUID while the one it drew is already held
/// (see [`ComAmazonawsBackupsearchCryobackupsearchserviceState::mint_identifier`]). A
/// UUIDv4 has 122 bits of entropy, so one re-draw is already past what this store can
/// expect to need; the bound is what makes the mint provably terminating.
const UUID_MINT_ATTEMPTS: usize = 8;

/// The server fault's text when a mint could not draw an identifier this store does not
/// already hold — a broken random source, which is a server problem and never a reason
/// to hand the caller a resource id another record already uses.
pub const MINT_EXHAUSTED: &str = "a unique job identifier could not be minted";

/// The ARN resource segment of an export job: `export-job/<ExportJobIdentifier>`.
pub const EXPORT_JOB_RESOURCE: &str = "export-job";

/// The ceiling on how many export jobs this store holds, for the same reason as
/// [`MAX_SEARCH_JOBS`]: `StartSearchResultExportJob` is the only thing that adds one and
/// nothing evicts them, so the operation that grows the store refuses past this cap with
/// the modelled `ServiceQuotaExceededException` (HTTP 402) the model declares on it. The
/// number is the emulator's own bound, not a published AWS default.
pub const MAX_EXPORT_JOBS: usize = 5_000;

/// The one wording of the server fault every handler answers with when this emulator's
/// search-job store cannot be reached, so the text cannot drift between operations.
pub const STORE_UNAVAILABLE: &str = "the search job store is unavailable";

/// The `StatusMessage` a search job whose scope resolved to no recovery point carries.
///
/// The model documents `StatusMessage` only as the text returned "for ... jobs with
/// issues", so it states no wording — but the service's recordings do: a `StartSearchJob`
/// whose `SearchScope` matches nothing is accepted and then settles to [`FAILED`] with
/// THIS sentence, which carries none of the request's data and is the same for every such
/// job. Being request-independent is what makes it the service's fixed template for this
/// outcome rather than one call's example text, so the settle path takes it from here.
pub const SCOPE_EMPTY: &str =
    "Search scope is empty. Revise search scope input and retry search job";

/// The text this service's `ResourceNotFoundException` carries when a job identifier
/// names nothing.
///
/// The model documents the member only as "Request references a resource which does not
/// exist", so it states no text — but the service's own recordings do: the SAME
/// request-independent sentence comes back from `GetSearchJob`, `ListSearchJobResults`,
/// `StopSearchJob` and `GetSearchResultExportJob` for a well-formed identifier naming
/// nothing. Being byte-identical across operations and carrying none of the request's
/// data is what makes it the service's fixed not-found template rather than one call's
/// example text, so every job-identifier lookup here refuses with it and none echoes the
/// identifier back. (The tag APIs, which resolve a resource by ARN, are recorded with a
/// different refusal and keep their own.)
pub const RESOURCE_NOT_FOUND: &str = "Resource is not found";

/// The text this service's `ValidationException` carries when a job identifier is not a
/// well-formed one, so the request is refused as INVALID INPUT and never reported as a
/// missing resource.
///
/// The model documents `ValidationException$message` only as "The input fails to satisfy
/// the constraints specified by an Amazon service", so it states no text — but the
/// service's recordings do: this request-independent sentence, carrying none of the
/// request's data, is what a malformed job identifier comes back with. It is therefore
/// the service's fixed template for this refusal rather than one call's example text,
/// and every identifier guard here answers with it ([`check_job_identifier`]).
pub const INVALID_JOB_ID: &str = "Invalid Job Id";

/// The group widths of the shape a job identifier of this service has: `8-4-4-4-12` hex
/// digits, the canonical rendering of a UUID.
///
/// Source: what this service MINTS — [`new_uuid`], the single mint behind
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::mint_identifier`] and
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::mint_export_identifier`] —
/// which is the only shape a `GenericId` of this service ever takes. The model puts no
/// `@pattern` on `GenericId`, so the boundary validator cannot refuse a string outside
/// that shape and the service does ([`check_job_identifier`]).
const JOB_IDENTIFIER_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `identifier` has the SHAPE a job identifier of this service has: five
/// hyphen-separated groups of [`JOB_IDENTIFIER_GROUPS`] hex digits.
///
/// Only the shape is judged, not which record it names: a hex digit's CASE is not part
/// of the shape, so an upper-case rendering is well formed and simply resolves to no
/// record (this service mints the lower-case one), while a string that cannot be an
/// identifier at all — any other length, a group of the wrong width, a non-hex
/// character, a missing or extra group — is not well formed.
pub fn job_identifier_well_formed(identifier: &str) -> bool {
    let mut groups = identifier.split('-');
    for width in JOB_IDENTIFIER_GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|digit| digit.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Refuse a job identifier that is not well formed, BEFORE any store lookup.
///
/// Every operation that resolves a job identifier to a record goes through here — the
/// two `Get`s, the two bound listings, `StopSearchJob` and `StartSearchResultExportJob` —
/// so the two refusals stay apart service-wide: a string that cannot be an identifier is
/// invalid input ([`INVALID_JOB_ID`], HTTP 400), and a well-formed identifier that names
/// nothing is a missing resource ([`RESOURCE_NOT_FOUND`], HTTP 404). A member that is a
/// FILTER rather than a resource to resolve (`ListSearchResultExportJobs`'s
/// `SearchJobIdentifier`) is not an identifier this rule covers: it narrows a listing and
/// is never looked up, so it answers an empty page.
pub fn check_job_identifier(identifier: &str) -> Result<(), String> {
    if job_identifier_well_formed(identifier) {
        return Ok(());
    }
    Err(INVALID_JOB_ID.to_string())
}

/// The Region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The AWS Region a request was signed for, which is the Region the resources it creates
/// live in. Falls back to the emulator's default Region for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A `SearchJobArn`: `arn:aws:backup-search:<region>:<account>:search-job/<id>`.
pub fn search_job_arn(signing_name: &str, region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        signing_name,
        region,
        &format!("{SEARCH_JOB_RESOURCE}/{identifier}"),
    )
}

/// An `ExportJobArn`: `arn:aws:backup-search:<region>:<account>:export-job/<id>`.
pub fn export_job_arn(signing_name: &str, region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        signing_name,
        region,
        &format!("{EXPORT_JOB_RESOURCE}/{identifier}"),
    )
}

/// The account segment of an ARN, which is the fifth colon-separated field
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`). `None` when the string
/// carries no such field.
pub fn arn_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// The text the TAG APIs' not-found refusal carries. Request-independent — it names
/// neither the ARN nor a resource type — so it is the service's fixed template for this
/// refusal rather than one call's example text.
pub const TAG_RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The REFUSAL an unresolvable `ResourceArn` is answered with: HTTP 404 under the error
/// code `NotFoundException`, carrying [`TAG_RESOURCE_NOT_FOUND`].
///
/// That code is not the name of any shape in this service's model, which declares
/// `ResourceNotFoundException` (HTTP 404) on these operations — and it is the code the
/// service's own recorded responses carry for an ARN that names no resource it holds. A
/// client branches on the code string, so the code the service answers is the behavioural
/// fact and the modelled shape name is not: a caller catching `NotFoundException` around
/// a tag call, as it must against the real service, has to catch this one too. The error
/// channel that renders it is [`crate::scaffolded::Scaffolded::NotFound`].
///
/// One refusal behind all three tag operations of this service — the read
/// `ListTagsForResource` and the writes `TagResource` and `UntagResource` — the same way
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::taggable_record`] is the one
/// resolution behind them: the lookup answers `None` for the same ARNs on every path, so
/// the code and the sentence it is reported with are decided here once instead of per
/// handler.
/// The submitted ARN takes no part in it: the refusal is the same for every ARN that
/// resolves to nothing, so it takes no argument and echoes no caller-supplied value.
pub fn tag_arn_not_found() -> crate::scaffolded::NotFoundException {
    crate::scaffolded::NotFoundException::new(TAG_RESOURCE_NOT_FOUND)
}

/// Whether `arn` is a well-formed ARN at all — the `arn` scheme followed by the six
/// colon-separated fields of the ARN grammar,
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, with a non-empty partition,
/// service and resource field (the region and account fields are empty on the ARNs of a
/// global or account-less service, so neither is required here).
///
/// This is what separates a value the service takes as a RESOURCE REFERENCE from a string
/// that cannot be one at all: see [`role_arn_input_refusal`].
pub fn arn_well_formed(arn: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    matches!(
        (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ),
        (Some("arn"), Some(partition), Some(service), Some(_), Some(_), Some(resource))
            if !partition.is_empty() && !service.is_empty() && !resource.is_empty()
    )
}

/// The account segment of `role_arn` when the value names an IAM ROLE, and `None` when it
/// names anything else.
///
/// The form judged is the one the `IamRoleArn` member publishes,
/// `^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$`
/// (`smithy_query(query="shape",shape="IamRoleArn")`): one of the three partitions, the
/// `iam` service, an EMPTY region because IAM is global, an account of `[a-z0-9-]+`, and a
/// resource field introduced by `role/` with a non-empty name after it. The name itself
/// may say anything — the pattern's trailing group is `(.+)`, which is how a role under a
/// path (`role/service-role/<name>`) is one value.
pub fn iam_role_arn_account(role_arn: &str) -> Option<&str> {
    let mut fields = role_arn.splitn(6, ':');
    let (Some("arn"), Some(partition), Some("iam"), Some(""), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    if !IAM_ROLE_ARN_PARTITIONS.contains(&partition) {
        return None;
    }
    let account_is_published_form = !account.is_empty()
        && account
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-');
    if !account_is_published_form {
        return None;
    }
    let name = resource.strip_prefix("role/")?;
    (!name.is_empty()).then_some(account)
}

/// The partitions the `IamRoleArn` `@pattern` admits, in the order it lists them.
const IAM_ROLE_ARN_PARTITIONS: &[&str] = &["aws", "aws-cn", "aws-us-gov"];

/// Why this service cannot pass `role_arn` to itself, or `None` when it can.
///
/// Starting an export makes the service assume the submitted role to write the results,
/// so the value is USED as a role rather than read as a string: what decides is whether
/// it names a role this caller can pass, not whether its text matches a published syntax.
/// A role can only be passed by a principal in the role's OWN account — AWS refuses a
/// cross-account pass-role — and every caller of this emulator is attributed to
/// [`::engine::emulator_account_id`], so a value naming a role of any other account cannot
/// be passed; neither can one that names no IAM role at all, because it names no role in
/// this account either ([`iam_role_arn_account`]). Both are the modelled
/// `AccessDeniedException` (HTTP 403): the refusal is an authorization decision, which is
/// the class a caller branches on, and no well-formed ARN is turned away as invalid input
/// for it ([`role_arn_input_refusal`]).
pub fn pass_role_denial(role_arn: &str) -> Option<String> {
    let passable =
        iam_role_arn_account(role_arn).is_some_and(|account| account == ::engine::emulator_account_id());
    (!passable).then(|| CROSS_ACCOUNT_PASS_ROLE.to_string())
}

/// The `AccessDeniedException` message for a role in another account. AWS states this
/// refusal in one fixed sentence, because the rule behind it names nothing from the
/// request: passing a role across accounts is not allowed at all.
pub const CROSS_ACCOUNT_PASS_ROLE: &str = "Cross-account pass role is not allowed.";

/// The input member whose modelled constraints this service applies itself rather than at
/// the request boundary (`skip_validation = ["RoleArn"]` on
/// `StartSearchResultExportJobInput`). Spelled as the MODEL spells it, so the validation
/// path the framework reports is the lowerCamel `roleArn` a boundary refusal reports.
const ROLE_ARN_MEMBER: &str = "RoleArn";

/// The `IamRoleArn` `@length` bound, min and max
/// (`smithy_query(query="shape",shape="IamRoleArn")`), which this service keeps enforcing
/// after `skip_validation` takes it off the generated member schema.
const ROLE_ARN_LENGTH: (i64, i64) = (20, 2048);

/// The `IamRoleArn` `@pattern`, verbatim from the model
/// (`smithy_query(query="shape",shape="IamRoleArn")`).
const ROLE_ARN_PATTERN: &str = "^arn:(?:aws|aws-cn|aws-us-gov):iam::[a-z0-9-]+:role/(.+)$";

/// The constraints a `RoleArn` that is not an ARN at all is judged against: the modelled
/// `@length` and the modelled `@pattern`, exactly the pair the boundary carried.
static ROLE_ARN_TRAITS: std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::LengthTrait::new(
                Some(ROLE_ARN_LENGTH.0),
                Some(ROLE_ARN_LENGTH.1),
            ),
        ));
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::PatternTrait::new(ROLE_ARN_PATTERN),
        ));
        traits
    });

/// The constraints a `RoleArn` that IS a well-formed ARN is judged against: the modelled
/// `@length` alone. The `@pattern` is deliberately absent — for a value of that kind the
/// pass-role decision ([`pass_role_denial`]) takes its place.
static ROLE_ARN_LENGTH_TRAITS: std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    std::sync::LazyLock::new(|| {
        let mut traits = ::aws_smithy_schema::TraitMap::new();
        traits.insert(Box::new(
            ::aws_smithy_schema_server::constraints::LengthTrait::new(
                Some(ROLE_ARN_LENGTH.0),
                Some(ROLE_ARN_LENGTH.1),
            ),
        ));
        traits
    });

/// `RoleArn` as a MEMBER schema carrying [`ROLE_ARN_TRAITS`], so the check itself is the
/// framework's own walk: the violation's kind, its path and the ceiling on how much of the
/// refused value is echoed are the boundary's, not this module's.
static ROLE_ARN_SCHEMA: std::sync::LazyLock<::aws_smithy_schema::Schema> =
    std::sync::LazyLock::new(|| role_arn_member_schema(&ROLE_ARN_TRAITS));

/// `RoleArn` as a member schema carrying [`ROLE_ARN_LENGTH_TRAITS`].
static ROLE_ARN_LENGTH_SCHEMA: std::sync::LazyLock<::aws_smithy_schema::Schema> =
    std::sync::LazyLock::new(|| role_arn_member_schema(&ROLE_ARN_LENGTH_TRAITS));

/// The `RoleArn` member schema carrying `traits`, named as the model names the member.
fn role_arn_member_schema(
    traits: &'static std::sync::LazyLock<::aws_smithy_schema::TraitMap>,
) -> ::aws_smithy_schema::Schema {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.backupsearch#StartSearchResultExportJobInput$RoleArn",
            "com.amazonaws.backupsearch",
            ROLE_ARN_MEMBER,
        ),
        ::aws_smithy_schema::ShapeType::String,
        ROLE_ARN_MEMBER,
        0,
    )
    .with_traits(traits)
}

/// Why `role_arn` is not a value this service accepts as INPUT at all, or `None` when it
/// is one the pass-role decision judges ([`pass_role_denial`]).
///
/// The two refusals a `RoleArn` can draw are different classes, so what separates them is
/// decided here once: a value that is not an ARN at all cannot name a role, so it is a
/// member-value fault and keeps the `ValidationException` (HTTP 400) the boundary answered
/// — in the boundary's own wording, because the check is the framework's walk over the
/// modelled `@length` and `@pattern` — while a value that IS a well-formed ARN
/// ([`arn_well_formed`]) is a resource reference the service resolves, and only its
/// `@length` is judged here. A reference it cannot resolve to a role it may pass is an
/// authorization refusal, not an input one.
pub fn role_arn_input_refusal(role_arn: &str) -> Option<String> {
    use ::aws_smithy_schema::serde::ShapeSerializer as _;

    let schema: &::aws_smithy_schema::Schema = if arn_well_formed(role_arn) {
        &ROLE_ARN_LENGTH_SCHEMA
    } else {
        &ROLE_ARN_SCHEMA
    };
    let mut validator = ::aws_smithy_schema_server::constraints::ValidatingSerializer::new();
    // Infallible: the walk accumulates violations instead of failing.
    let _ = validator.write_string(schema, role_arn);
    ::aws_smithy_schema_server::envelope(&validator.into_violations())
}

/// One persisted search job. Every member a modelled response carries is read from
/// here; nothing in a response is recomputed from the request.
#[derive(Debug, Clone)]
pub struct SearchJob {
    pub identifier: String,
    pub arn: String,
    pub name: Option<String>,
    /// One of [`TERMINAL_STATES`], [`CREATING`], [`RUNNING`] or [`STOPPING`].
    pub status: String,
    pub creation_time: DateTime,
    pub completion_time: Option<DateTime>,
    pub status_message: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub search_scope: SearchScope,
    pub item_filters: ItemFilters,
    pub search_scope_summary: SearchScopeSummary,
    pub current_search_progress: CurrentSearchProgress,
    /// The matches the scan RECORDED. `ListSearchJobResults` pages over exactly this, so
    /// it is empty for a scan that has not got through its scope yet — the matches it will
    /// record wait in [`SearchJob::scan_matches`] until the clock makes the scan's
    /// completion due.
    pub results: Vec<ResultItem>,
    /// The matches the scan FINDS: the items held by the backup indexes of the recovery
    /// points in scope that this job's `ItemFilters` admit, resolved from the store when
    /// the job was started (the same instant the scope itself was resolved, so a recovery
    /// point indexed later does not change a running scan's outcome).
    ///
    /// [`settle_owed_statuses`] moves them into [`SearchJob::results`] when the scan reaches
    /// [`COMPLETED`], which is the instant the progress reports them matched: a reader can
    /// page exactly the matches the progress counts, never more.
    pub scan_matches: Vec<ResultItem>,
    /// The recovery points the scan covered, each with the status its backup index had.
    /// `ListSearchJobBackups` pages over exactly this: the recovery points of this account
    /// the job's scope resolved to ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::recovery_points_in_scope`]).
    ///
    /// Every recovery point the scope selected is here, INCLUDING one whose backup index
    /// was in no state to be read ([`index_searchable`]): that listing is where such a
    /// backup is displayed with the status and status message saying why the search left
    /// it out, so it is held here while none of its items reach
    /// [`SearchJob::scan_matches`], [`SearchJob::results`] or either count of the
    /// scope summary and the progress.
    pub backups: Vec<SearchJobBackupsResult>,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the call that minted this job carried, if any, with the instant
    /// its validity window closes. Idempotency is keyed off the job records themselves,
    /// so a caller cannot grow a second index by varying only the token; the window is
    /// what keeps that key from outliving the token
    /// ([`CLIENT_TOKEN_VALIDITY`]).
    pub client_token: Option<ClientToken>,
    /// Canonical form of the `StartSearchJob` body this job was created from. A repeat
    /// of the same body under the same `ClientToken` is idempotent; a different body
    /// under that token is a conflict.
    pub request_fingerprint: String,
    /// The statuses the clock owes this job IN DUE ORDER: each entry is the instant the
    /// status is owed from, the status, and the `StatusMessage` that status comes with
    /// (`None` for one that needs no message). A scan owes two — [`RUNNING`] and then its
    /// terminal status — so this is a queue and not a single transition; it is EMPTY for a
    /// job in a terminal status, which owes nothing.
    ///
    /// Each entry is due its OWN span after the entry before it — the first of them after
    /// the call that started the job — so every phase the queue holds lasts a whole span
    /// and a client acting right after one call is never carried past the phase that call
    /// put the job in. For a scan those spans are [`CREATING_DELAY`] (being set up) and
    /// then [`SCAN_DELAY`] (running), the longer of the two because a running scan is the
    /// window a cancel has.
    ///
    /// Each entry is an [`OwedTransition`], exactly as [`ExportJob::next_status_due`]'s
    /// one is: the message it carries is OWNED, because a terminal status the service has
    /// to explain can need text counted from the job's own record ([`scan_outcome`]), not
    /// only one of this module's templates.
    pub owed_statuses: VecDeque<OwedTransition>,
}

impl SearchJob {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATES.contains(&self.status.as_str())
    }

    /// Whether this job is already in, or already on its way to, the end state
    /// `StopSearchJob` asks for (see [`STOP_SATISFIED_STATES`]).
    pub fn is_stop_satisfied(&self) -> bool {
        STOP_SATISFIED_STATES.contains(&self.status.as_str())
    }

    /// Whether this job is in a status a stop may act from.
    ///
    /// DERIVED from the two status sets rather than listed, so a status this service
    /// later reports cannot be judged by a stale list. The operation's one-sentence
    /// documentation names the status a stop acts on ([`RUNNING`],
    /// `smithy_query(query="doc", shape="StopSearchJob")`) but says nothing about a job
    /// the service has accepted and not yet started scanning, and the service's own
    /// recordings settle that: a stop sent straight after `StartSearchJob`, while the job
    /// is still [`CREATING`], is accepted with the modelled empty 200 and the job
    /// converges on [`STOPPED`]. The service exposes no window in which a job it has
    /// accepted cannot be cancelled — the cancel is recorded against the job record and
    /// honoured by the scan whenever it starts — so a stop is refused only once the job
    /// has reached a status it never leaves ([`TERMINAL_STATES`]) that the stop itself
    /// does not produce: there the scan is over, so there is nothing left to cancel and
    /// the record cannot move from where it is. [`STOPPED`] is terminal too, but it
    /// already satisfies the call ([`STOP_SATISFIED_STATES`]).
    pub fn is_stoppable(&self) -> bool {
        !self.is_terminal() || self.is_stop_satisfied()
    }

    /// Make `status` the ONE status this job is still owed, due at `due`, dropping
    /// whatever it was owed before.
    ///
    /// What a stop does to a scan: the statuses the scan's own schedule still held are no
    /// longer owed, so a stopped job cannot later settle to the terminal status its scan
    /// would have reached.
    pub fn owe_only(
        &mut self,
        due: SystemTime,
        status: &'static str,
        message: Option<String>,
    ) {
        self.owed_statuses.clear();
        self.owed_statuses.push_back((due, status, message));
    }

}

/// A search job's status is moved by the clock, so [`settle_owed_statuses`] moves it —
/// including the one part of a transition that belongs to THIS record: the matches the
/// scan recorded and the progress written with them.
impl OwedStatusRecord for SearchJob {
    fn client_token_mut(&mut self) -> &mut Option<ClientToken> {
        &mut self.client_token
    }

    /// A scan owes a QUEUE of statuses ([`SearchJob::owed_statuses`]), so the next one due
    /// is the front of it.
    fn owed_transition(&self) -> Option<OwedTransition> {
        self.owed_statuses.front().cloned()
    }

    fn drop_owed_transition(&mut self) {
        self.owed_statuses.pop_front();
    }

    fn status_members(&mut self) -> StatusMembers<'_> {
        StatusMembers {
            status: &mut self.status,
            status_message: &mut self.status_message,
            completion_time: &mut self.completion_time,
        }
    }

    /// The scan's progress is brought to the scope it was given — a scan that ran to
    /// `COMPLETED` has scanned everything in scope and matched exactly the results it
    /// recorded; one that was `STOPPED` keeps the progress it had reached, and one that
    /// `FAILED` never got through its scope, so it keeps the progress it had too.
    ///
    /// The matches the scan found are RECORDED by the same transition
    /// ([`SearchJob::scan_matches`] -> [`SearchJob::results`]), so what
    /// `ListSearchJobResults` pages and what the progress counts as matched are written at
    /// one instant and can never disagree.
    fn on_status_reached(&mut self, status: &'static str) {
        if status == COMPLETED {
            // A scan that got through its scope has RECORDED the matches it found, so they
            // become readable at this instant and not before — and the progress is written
            // from the same record, so the matched count and the results a reader pages
            // cannot disagree.
            self.results = std::mem::take(&mut self.scan_matches);
            self.current_search_progress = CurrentSearchProgress {
                recovery_points_scanned_count: self
                    .search_scope_summary
                    .total_recovery_points_to_scan_count,
                items_scanned_count: self.search_scope_summary.total_items_to_scan_count,
                items_matched_count: Some(self.results.len() as i64),
            };
        } else if TERMINAL_STATES.contains(&status) {
            // The scan ended without getting through its scope, so the matches it would
            // have recorded are no longer owed to anybody: it keeps whatever it had already
            // recorded and finds nothing more.
            self.scan_matches = Vec::new();
        }
    }
}

/// A search job is the record a `StartSearchJob` `ClientToken` names, so the replay rule
/// reads it through [`client_token_replay`] rather than through logic of its own.
impl TokenedRecord for SearchJob {
    const RESOURCE_TYPE: &'static str = SEARCH_JOB_RESOURCE;

    fn held_client_token(&self) -> Option<&ClientToken> {
        self.client_token.as_ref()
    }

    fn held_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    fn record_identifier(&self) -> &str {
        &self.identifier
    }
}

/// The search-job store is capped, so `StartSearchJob` refuses past it by the cap rule
/// every capped store of this service follows ([`check_store_capacity`]).
impl CappedRecord for SearchJob {
    const STORE_CAP: usize = MAX_SEARCH_JOBS;
    const QUOTA_CODE: &'static str = "SearchJobsPerAccount";
    const JOB_KIND: &'static str = "search";
}

/// One recovery point of this account's backup index, WITH the items that index holds.
///
/// A scan has two layers and this record carries both. The outer one is the recovery
/// point itself, which a `SearchScope` selects and `ListSearchJobBackups` reports. The
/// inner one is the items inside that backup's index — the objects of a backed-up S3
/// bucket, the files of a backed-up EBS volume — which is the layer a job's `ItemFilters`
/// select from, `SearchScopeSummary.TotalItemsToScanCount` counts and
/// `ListSearchJobResults` pages. Holding them on ONE record is what keeps a recovery
/// point and the items it holds from drifting apart.
#[derive(Debug, Clone, Default)]
pub struct IndexedRecoveryPoint {
    /// The recovery point itself, exactly as `ListSearchJobBackups` reports it.
    pub backup: SearchJobBackupsResult,
    /// The backup vault holding it. A result item reports the vault its backup lives in
    /// while `SearchJobBackupsResult` carries no vault member, so the name is held here:
    /// it is a property of the backup, not of one item inside it.
    pub backup_vault_name: Option<String>,
    /// The items this recovery point's backup index holds, in index order: an
    /// `S3ResultItem` per backed-up object (its key, size, creation time, ETag and
    /// version id) or an `EBSResultItem` per backed-up file (its path, size and times).
    ///
    /// The three members naming the BACKUP an item came from
    /// (`BackupResourceArn`/`SourceResourceArn`/`BackupVaultName`) belong to this record
    /// rather than to one item, so a scan stamps them from here as it reads the index
    /// ([`scanned_items`]) and a result cannot name a backup it was not found in.
    pub items: Vec<ResultItem>,
}

impl From<SearchJobBackupsResult> for IndexedRecoveryPoint {
    /// A recovery point whose backup index this store holds no items for, which is every
    /// recovery point until something writes its index: a scope resolving to it has a
    /// backup to scan and no item inside it.
    fn from(backup: SearchJobBackupsResult) -> Self {
        Self {
            backup,
            backup_vault_name: None,
            items: Vec::new(),
        }
    }
}

/// One persisted export job: the record `StartSearchResultExportJob` writes and
/// `GetSearchResultExportJob`/`ListSearchResultExportJobs` report. Every member of a
/// modelled response is read from here; nothing is recomputed from the request.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub identifier: String,
    pub arn: String,
    /// The search job whose results this export transmits.
    pub search_job_identifier: String,
    pub search_job_arn: String,
    /// One of the modelled `ExportJobStatus` values ([`RUNNING`], [`COMPLETED`],
    /// [`FAILED`]).
    pub status: String,
    pub creation_time: DateTime,
    pub completion_time: Option<DateTime>,
    /// The text a `FAILED` export carries, as the model documents the member. Written
    /// only once the export has actually reached that status — the text the clock owes
    /// lives in [`ExportJob::next_status_due`] until then.
    pub status_message: Option<String>,
    pub export_specification: ExportSpecification,
    /// The `RoleArn` the start call carried, if any — stored so the record says which
    /// role the export was started under.
    pub role_arn: Option<String>,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the call that minted this export job carried, if any, with the
    /// instant its validity window closes ([`CLIENT_TOKEN_VALIDITY`]).
    pub client_token: Option<ClientToken>,
    /// Canonical form of the `StartSearchResultExportJob` body this job was created
    /// from: a repeat under the same `ClientToken` is idempotent, a different body under
    /// that token is a conflict.
    pub request_fingerprint: String,
    /// The transition the clock owes this export job ([`OwedTransition`]): the status, the
    /// instant it is owed from and the `StatusMessage` that status comes with. `None` once
    /// the job has reached a status it never leaves.
    pub next_status_due: Option<OwedTransition>,
}

/// An export job's status is moved by the clock exactly as a search job's is, so the same
/// [`settle_owed_statuses`] moves it; nothing beyond its status members is written when it
/// reaches one, so it overrides no more of the step than where its own members live.
impl OwedStatusRecord for ExportJob {
    fn client_token_mut(&mut self) -> &mut Option<ClientToken> {
        &mut self.client_token
    }

    /// An export job owes at most ONE transition, so that is the next one due.
    fn owed_transition(&self) -> Option<OwedTransition> {
        self.next_status_due.clone()
    }

    fn drop_owed_transition(&mut self) {
        self.next_status_due = None;
    }

    fn status_members(&mut self) -> StatusMembers<'_> {
        StatusMembers {
            status: &mut self.status,
            status_message: &mut self.status_message,
            completion_time: &mut self.completion_time,
        }
    }
}

/// An export job is the record a `StartSearchResultExportJob` `ClientToken` names, and the
/// same replay rule decides what a repeat under it answers ([`client_token_replay`]).
impl TokenedRecord for ExportJob {
    const RESOURCE_TYPE: &'static str = EXPORT_JOB_RESOURCE;

    fn held_client_token(&self) -> Option<&ClientToken> {
        self.client_token.as_ref()
    }

    fn held_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    fn record_identifier(&self) -> &str {
        &self.identifier
    }
}

/// The export-job store is capped for the same reason as the search-job one, and the SAME
/// cap rule decides what the call that would grow it past the cap answers
/// ([`check_store_capacity`]).
impl CappedRecord for ExportJob {
    const STORE_CAP: usize = MAX_EXPORT_JOBS;
    const QUOTA_CODE: &'static str = "ExportJobsPerAccount";
    const JOB_KIND: &'static str = "export";
}

/// Per-service state for com-amazonaws-backupsearch-cryobackupsearchservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupsearchCryobackupsearchserviceState {
    /// Every search job, keyed by its `SearchJobIdentifier`.
    pub search_jobs: HashMap<String, SearchJob>,
    /// Identifiers in creation order, so `ListSearchJobs` pages deterministically.
    /// `ListSearchJobs` walks this backwards (see
    /// [`jobs_newest_first`](Self::jobs_newest_first)).
    pub job_order: Vec<String>,
    /// The ITEMS the backup indexes of this account's recovery points hold, as records
    /// of the same shape the scope resolution works in ([`IndexedRecoveryPoint`]).
    ///
    /// The recovery points THEMSELVES are not kept here: a backup exists because AWS
    /// Backup took one, so the inventory a scope is resolved against is read from that
    /// component's store on every start ([`indexed_recovery_points`]) rather than
    /// copied here, which is what keeps a backup a client has just created — or just
    /// deleted — from having to be propagated into a second list to be searchable. What
    /// this field holds is the layer that component does NOT hold: the objects of a
    /// backed-up bucket and the files of a backed-up volume. A record here supersedes
    /// the projection of the same backup, so the two cannot report one backup twice
    /// ([`recovery_points_in_scope`](Self::recovery_points_in_scope)).
    ///
    /// BOTH layers a job reports are COUNTED from that resolved scope — the backups from
    /// [`recovery_points_in_scope`](Self::recovery_points_in_scope) and the items from
    /// [`scanned_items`] — rather than assumed by a handler, so every member a job
    /// reports about its scope and its progress, and the terminal status it settles to,
    /// follows the records the emulator holds.
    pub recovery_points: Vec<IndexedRecoveryPoint>,
    /// Every export job, keyed by its `ExportJobIdentifier`.
    pub export_jobs: HashMap<String, ExportJob>,
    /// Export-job identifiers in creation order, so `ListSearchResultExportJobs` pages
    /// deterministically. `ListSearchResultExportJobs` walks this backwards (see
    /// [`export_jobs_newest_first`](Self::export_jobs_newest_first)), exactly as
    /// `ListSearchJobs` walks `job_order`.
    pub export_job_order: Vec<String>,
}

/// The records `order` names, NEWEST-CREATED FIRST.
///
/// `order` holds identifiers in creation order and is only ever appended to, so walking
/// it backwards is exactly newest-first and the records it reports keep their relative
/// order across calls; an identifier the map no longer holds is skipped rather than
/// reported as a gap.
///
/// A record appended to `order` lands at the FRONT of what this reports and pushes every
/// older entry one place later, so a listing that pages this sequence anchors its page
/// token at the back of it ([`PageAnchor::NewestFirst`]) rather than at the front.
///
/// ONE helper behind BOTH job listings of this service
/// ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::jobs_newest_first`] and
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::export_jobs_newest_first`]),
/// so the two cannot drift apart on the order the service reports its jobs in.
fn newest_first<'a, T>(
    order: &'a [String],
    records: &'a HashMap<String, T>,
) -> impl Iterator<Item = &'a T> {
    order
        .iter()
        .rev()
        .filter_map(|identifier| records.get(identifier))
}

/// Which record of this store a tag operation's `ResourceArn` resolved to, and its
/// identifier in that record's map — what
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::taggable_record`] answers.
///
/// The variants are the resources this service mints that carry a tag set, so adding a
/// third taggable record type is a variant here and one more arm in the single resolution
/// step, rather than a second copy of the resolution order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TaggableRecord {
    SearchJob(String),
    ExportJob(String),
}

impl ComAmazonawsBackupsearchCryobackupsearchserviceState {
    /// Materialise every status transition the clock has already made due. Call this
    /// FIRST in every handler that reports or moves a search job's STATUS, including a
    /// read: it only applies a change that was owed at `now` whether or not anyone
    /// called. The tagging operations (`TagResource`, `UntagResource`,
    /// `ListTagsForResource`) are exempt — they reach a job's tag set by ARN, which no
    /// owed status transition touches, so settling could not change what they read or
    /// write.
    ///
    /// Both record types go through the ONE step that implements the rule
    /// ([`settle_owed_statuses`]), so what a status transition writes cannot differ
    /// between them.
    pub fn settle(&mut self, now: SystemTime) {
        for job in self.search_jobs.values_mut() {
            settle_owed_statuses(job, now);
        }
        for job in self.export_jobs.values_mut() {
            settle_owed_statuses(job, now);
        }
    }

    /// The job `identifier` names, or `None` when this account holds no such job.
    pub fn job(&self, identifier: &str) -> Option<&SearchJob> {
        self.search_jobs.get(identifier)
    }

    /// The job `identifier` names, mutably.
    pub fn job_mut(&mut self, identifier: &str) -> Option<&mut SearchJob> {
        self.search_jobs.get_mut(identifier)
    }

    /// The record whose minted ARN is `arn` — how the tagging operations, which take an
    /// unconstrained `ResourceArn`, resolve the resource they name. THE one resolution
    /// step: both [`tags_by_arn`](Self::tags_by_arn) (the read `ListTagsForResource`
    /// performs) and [`tags_by_arn_mut`](Self::tags_by_arn_mut) (the write `TagResource`
    /// and `UntagResource` perform) reach their tag set through this, so the ARNs the read
    /// path resolves and the ARNs the write path resolves cannot drift apart, and a change
    /// to the resolution — another taggable record type, or which record wins when two
    /// carry one ARN — is made here once.
    ///
    /// The lookup spans EVERY resource this service mints and holds, not one type of
    /// them: a `ResourceArn` is documented only as "the resource that will have the
    /// indicated tags", and both of this service's resources are its own
    /// (`SearchJobArn` and `ExportJobArn` are each `@arnReference` to this service) and
    /// both carry the same `TagMap` on the operation that mints them. So an ARN matching
    /// a held search job or a held export job resolves, and `None` — the not-found
    /// refusal the tag APIs answer ([`tag_arn_not_found`]) — is reserved for an ARN that
    /// matches no record this store holds.
    ///
    /// It answers WHICH record was matched rather than a borrow of it, so the one
    /// resolution serves the mutable path too: a map cannot be borrowed for the search and
    /// the write at once.
    fn taggable_record(&self, arn: &str) -> Option<TaggableRecord> {
        if let Some(job) = self.search_jobs.values().find(|job| job.arn == arn) {
            return Some(TaggableRecord::SearchJob(job.identifier.clone()));
        }
        let job = self.export_jobs.values().find(|job| job.arn == arn)?;
        Some(TaggableRecord::ExportJob(job.identifier.clone()))
    }

    /// The tag set stored on the resource whose minted ARN is `arn`, as
    /// `ListTagsForResource` reads it — the resource
    /// [`taggable_record`](Self::taggable_record) resolves the ARN to.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&HashMap<String, String>> {
        match self.taggable_record(arn)? {
            TaggableRecord::SearchJob(identifier) => {
                self.search_jobs.get(&identifier).map(|job| &job.tags)
            }
            TaggableRecord::ExportJob(identifier) => {
                self.export_jobs.get(&identifier).map(|job| &job.tags)
            }
        }
    }

    /// The tag set stored on the resource whose minted ARN is `arn`, mutably — the same
    /// resolution [`tags_by_arn`](Self::tags_by_arn) performs, for `TagResource` and
    /// `UntagResource`.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        match self.taggable_record(arn)? {
            TaggableRecord::SearchJob(identifier) => self
                .search_jobs
                .get_mut(&identifier)
                .map(|job| &mut job.tags),
            TaggableRecord::ExportJob(identifier) => self
                .export_jobs
                .get_mut(&identifier)
                .map(|job| &mut job.tags),
        }
    }

    /// Every job NEWEST-CREATED FIRST, which is the sequence `ListSearchJobs` filters
    /// and pages: the service reports the most recently created job first, so a
    /// single-page read is "the latest search job".
    ///
    /// `job_order` holds the identifiers in creation order and is only ever appended to,
    /// so walking it backwards is exactly newest-first and two calls report the same jobs
    /// in the same relative order. A job started in between lands at the HEAD of what
    /// this reports, which is why `ListSearchJobs` anchors its page token at the other
    /// end of the sequence ([`PageAnchor::NewestFirst`]).
    /// `job_order` and `search_jobs` are written together, so an identifier in the order
    /// always resolves.
    pub fn jobs_newest_first(&self) -> impl Iterator<Item = &SearchJob> {
        newest_first(&self.job_order, &self.search_jobs)
    }

    /// The recovery points of this account `scope` selects — the scan's resolved scope,
    /// and so what a search job over that scope has to get through. Each one carries the
    /// items its backup index holds, which is the layer [`scanned_items`] reads.
    ///
    /// The inventory selected FROM is the account's searchable backups: the ones AWS
    /// Backup holds, which the caller hands in after reading that component's store
    /// ([`indexed_recovery_points`]), plus the index items this store holds
    /// ([`Self::recovery_points`]) — a record held here supersedes the projection of the
    /// same backup, so one backup is never covered twice and the items of a backup this
    /// store knows the contents of are the ones the scan reads.
    ///
    /// A recovery point is in scope when its resource type is one the scope names and
    /// every filter the scope carries admits it; a filter the records cannot be judged
    /// against (a tag filter: a recovery point record here carries no tags, nor does the
    /// model give it any) admits none, because this store holds nothing that can be
    /// shown to match it.
    ///
    /// The scope is what the search COVERS, which is wider than what it reads: whether a
    /// covered recovery point's backup index could be read at all is a separate decision
    /// ([`index_searchable`]), because a backup left out for that reason is still
    /// displayed by `ListSearchJobBackups` ([`SearchJob::backups`]).
    pub fn recovery_points_in_scope(
        &self,
        scope: &SearchScope,
        held_by_backup: &[IndexedRecoveryPoint],
    ) -> Vec<IndexedRecoveryPoint> {
        let superseded = |point: &IndexedRecoveryPoint| {
            point.backup.backup_resource_arn.is_some()
                && self.recovery_points.iter().any(|held| {
                    held.backup.backup_resource_arn == point.backup.backup_resource_arn
                })
        };
        self.recovery_points
            .iter()
            .chain(held_by_backup.iter().filter(|point| !superseded(point)))
            .filter(|point| in_scope(&point.backup, scope))
            .cloned()
            .collect()
    }

    /// What a `StartSearchJob` carrying `token` over the body `fingerprint` renders is
    /// answered with: `Ok(None)` for a new request, `Ok(Some(job))` for a repeat to be
    /// answered out of that stored job, `Err` for the conflict.
    ///
    /// The decision is [`client_token_replay`]'s, not this method's — the same rule
    /// [`export_job_replay`](Self::export_job_replay) applies, because the model states
    /// the window sentence on both of this service's `ClientToken` members. `operation` is
    /// the name the refusal reads, which each handler takes from its own operation.
    pub fn search_job_replay(
        &self,
        token: Option<&str>,
        fingerprint: &str,
        now: SystemTime,
        operation: &str,
    ) -> Result<Option<&SearchJob>, ConflictException> {
        client_token_replay(&self.search_jobs, token, fingerprint, now, operation)
    }

    /// Whether another search job may be minted, by the cap rule every capped store of
    /// this service applies ([`check_store_capacity`]): `Err` is the refusal
    /// `StartSearchJob` answers with once this store holds [`MAX_SEARCH_JOBS`] jobs, which
    /// each handler lifts into its own error channel.
    ///
    /// The decision is the shared rule's, not this method's — the same rule
    /// [`check_export_job_capacity`](Self::check_export_job_capacity) applies, because
    /// both stores are capped for the same reason. `service_code` is the signing name the
    /// handler took from its own request.
    pub fn check_search_job_capacity(
        &self,
        service_code: &str,
    ) -> Result<(), ServiceQuotaExceededException> {
        check_store_capacity(&self.search_jobs, service_code)
    }

    /// Store a newly minted job, keeping the creation order.
    pub fn insert_job(&mut self, job: SearchJob) {
        self.job_order.push(job.identifier.clone());
        self.search_jobs.insert(job.identifier.clone(), job);
    }

    /// A fresh `SearchJobIdentifier` no job in this store holds, or `None` when no free
    /// one could be drawn (see [`fresh_identifier`](Self::fresh_identifier)), which the
    /// caller answers as a server fault rather than by handing out a held identifier.
    ///
    /// `GenericId` carries no pattern or length in the model, but the service's
    /// recordings show what it mints: a UUID, which is also the only shape the service's
    /// own job-identifier guard accepts. So this draws a version-4 UUID — unique, opaque,
    /// not derived from the request and not reproducible across runs, which is what keeps
    /// two independently started jobs from colliding on id. The caller STORES what this
    /// returns; nothing recomputes it.
    pub fn mint_identifier(&self) -> Option<String> {
        let sequence = self.search_jobs.len() as u64;
        self.fresh_identifier(sequence, |store, candidate| {
            store.search_jobs.contains_key(candidate)
        })
    }

    /// The export job `identifier` names, or `None` when this account holds no such
    /// export job.
    pub fn export_job(&self, identifier: &str) -> Option<&ExportJob> {
        self.export_jobs.get(identifier)
    }

    /// The export job `identifier` names, mutably — how the call that started an export
    /// records on it what writing the results into the destination bucket came to.
    pub fn export_job_mut(&mut self, identifier: &str) -> Option<&mut ExportJob> {
        self.export_jobs.get_mut(identifier)
    }

    /// Every export job NEWEST-CREATED FIRST, which is the sequence
    /// `ListSearchResultExportJobs` filters and pages — the SAME order
    /// [`jobs_newest_first`](Self::jobs_newest_first) reports a search job listing in,
    /// because the rule is the service's and not one listing's: a job listing of this
    /// service reports the most recently created job first, so a single-page read is
    /// "the latest export job".
    ///
    /// `export_job_order` and `export_jobs` are written together, so an identifier in
    /// the order always resolves.
    pub fn export_jobs_newest_first(&self) -> impl Iterator<Item = &ExportJob> {
        newest_first(&self.export_job_order, &self.export_jobs)
    }

    /// What a `StartSearchResultExportJob` carrying `token` over the body `fingerprint`
    /// renders is answered with — the SAME rule
    /// [`search_job_replay`](Self::search_job_replay) answers a search-job repeat by
    /// ([`client_token_replay`]), over this store's export jobs.
    pub fn export_job_replay(
        &self,
        token: Option<&str>,
        fingerprint: &str,
        now: SystemTime,
        operation: &str,
    ) -> Result<Option<&ExportJob>, ConflictException> {
        client_token_replay(&self.export_jobs, token, fingerprint, now, operation)
    }

    /// Whether another export job may be minted — the SAME rule
    /// [`check_search_job_capacity`](Self::check_search_job_capacity) answers a search-job
    /// start by ([`check_store_capacity`]), over this store's export jobs and its own cap
    /// [`MAX_EXPORT_JOBS`].
    pub fn check_export_job_capacity(
        &self,
        service_code: &str,
    ) -> Result<(), ServiceQuotaExceededException> {
        check_store_capacity(&self.export_jobs, service_code)
    }

    /// Store a newly minted export job, keeping the creation order.
    pub fn insert_export_job(&mut self, job: ExportJob) {
        self.export_job_order.push(job.identifier.clone());
        self.export_jobs.insert(job.identifier.clone(), job);
    }

    /// A fresh `ExportJobIdentifier` no export job in this store holds, or `None` when
    /// no free one could be drawn.
    ///
    /// Minted exactly like [`mint_identifier`](Self::mint_identifier), and for the same
    /// reason: both members are the model's `GenericId`, both are minted by this one
    /// service, and what it mints for a job is a UUID.
    pub fn mint_export_identifier(&self) -> Option<String> {
        let sequence = self.export_jobs.len() as u64;
        self.fresh_identifier(sequence, |store, candidate| {
            store.export_jobs.contains_key(candidate)
        })
    }

    /// A version-4 UUID this store does not already hold, as `taken` decides what
    /// "hold" means. One mint for every identifier this service hands out, so two of
    /// them cannot disagree about the shape of an id.
    ///
    /// A UUIDv4 collides with 122 bits of entropy behind it, so the re-draw is BOUNDED
    /// rather than a `loop`: the function terminates on its own. Exhausting the bound
    /// would mean a broken random source, and the answer is then `None` — never an
    /// identifier the store already holds, which a caller would insert over an existing
    /// record.
    fn fresh_identifier(
        &self,
        sequence: u64,
        taken: impl Fn(&Self, &str) -> bool,
    ) -> Option<String> {
        (0..UUID_MINT_ATTEMPTS)
            .map(|attempt| new_uuid(sequence.wrapping_add(attempt as u64)))
            .find(|candidate| !taken(self, candidate))
    }
}

/// A fresh identifier in the shape this service's recordings show it mints: a version-4
/// UUID, `8-4-4-4-12` lowercase hex with the version and variant bits RFC 4122 §4.4
/// fixes.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2` crate)
/// and hex-encoded (the `hex` crate): the OS-seeded randomness a `RandomState` draws, the
/// system clock in nanoseconds, and `sequence` — a number the store varies per draw, so
/// two draws in the same nanosecond cannot collide. Nothing here comes from the request,
/// and nothing is reproducible across runs. An id names a resource for correlation; a
/// credential would come from `ctx.credential_store()` instead, never from here.
fn new_uuid(sequence: u64) -> String {
    use ::sha2::{Digest, Sha256};
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Whether `point` is one of the recovery points `scope` selects.
///
/// Every member of `SearchScope` is a filter over the backup index: the resource type
/// (the model allows exactly one), the recovery-point and source-resource ARNs, the
/// window the backup was created in, and the backup's tags. A filter the request leaves
/// out admits everything, and so does one it carries that states no constraint at all (an
/// empty ARN list, an empty tag map, a creation window with neither bound set — all of
/// which the model allows); a filter that states a constraint admits only what matches it,
/// and a record that cannot be judged against such a filter (no creation time against a
/// bounded creation window, no tags at all against a non-empty tag filter) is not
/// selected, because nothing shows it to be in scope.
fn in_scope(point: &SearchJobBackupsResult, scope: &SearchScope) -> bool {
    // `BackupResourceTypes` is @required and @length(min: 1, max: 1), so it always names
    // a type and is never "no filter": a record whose type it does not name is out of
    // scope. (The ARN lists below read an EMPTY list the other way round, as no filter at
    // all, because the model makes them optional with @length(min: 0).)
    if !scope
        .backup_resource_types
        .iter()
        .any(|wanted| point.resource_type.as_deref() == Some(wanted.as_str()))
    {
        return false;
    }
    if !arn_admits(scope.backup_resource_arns.as_deref(), point.backup_resource_arn.as_deref()) {
        return false;
    }
    if !arn_admits(scope.source_resource_arns.as_deref(), point.source_resource_arn.as_deref()) {
        return false;
    }
    if !creation_window_admits(
        scope.backup_resource_creation_time.as_ref(),
        point.backup_creation_time,
    ) {
        return false;
    }
    // A tag filter: a recovery point carries no tags in this model, so no record this
    // store holds can be shown to carry the ones the scope asks for. Revisit this branch
    // if `SearchJobBackupsResult` ever gains a tags member (or the records written into
    // `recovery_points` carry tags some other way): it would then have to MATCH the
    // filter rather than exclude everything.
    scope
        .backup_resource_tags
        .as_ref()
        .is_none_or(|tags| tags.is_empty())
}

/// Whether an ARN filter admits `candidate`: an absent or empty list is no filter at all,
/// and a list admits exactly the ARNs it names.
fn arn_admits(filter: Option<&[String]>, candidate: Option<&str>) -> bool {
    match filter {
        None => true,
        Some([]) => true,
        Some(arns) => candidate.is_some_and(|arn| arns.iter().any(|wanted| wanted == arn)),
    }
}

/// Whether a creation-window filter admits `candidate`: an absent window is no filter at
/// all, and neither is a window PRESENT but stating no bound — both of its bounds are
/// optional in the model, so `{}` is a value a request may send and it constrains nothing,
/// exactly as an empty ARN list or an empty tag map does. A window that names at least one
/// bound admits the instants inside it, the bounds themselves included (the filter names
/// the edge of the window, not a point outside it), and a record carrying no instant cannot
/// be shown to fall inside it.
fn creation_window_admits(
    window: Option<&BackupCreationTimeFilter>,
    candidate: Option<DateTime>,
) -> bool {
    let Some(window) = window else {
        return true;
    };
    if window.created_after.is_none() && window.created_before.is_none() {
        return true;
    }
    let Some(created) = candidate.map(instant_key) else {
        return false;
    };
    !window
        .created_after
        .is_some_and(|after| created < instant_key(after))
        && !window
            .created_before
            .is_some_and(|before| created > instant_key(before))
}

/// A `DateTime` as a totally ordered key, so two instants can be compared: the model's
/// timestamps carry seconds and sub-second nanos, and `DateTime` itself is not `Ord`.
fn instant_key(instant: DateTime) -> (i64, u32) {
    (instant.secs(), instant.subsec_nanos())
}

/// The terminal status the clock owes a scan over the recovery points `points` a scope
/// resolved to, with the `StatusMessage` that status comes with.
///
/// Both the status and the message follow from the SAME two facts about that resolved
/// scope, which is why they are decided from the resolved points themselves rather than
/// from a count a caller passes in:
///
/// * a scope that resolved to NO recovery point is a terminal failure — there was nothing
///   to search — and the service says so in the message ([`SCOPE_EMPTY`]), so a caller
///   branching on a terminal status takes its failure path instead of reading results that
///   were never produced;
/// * a scope that resolved to recovery points the scan could all read ends `COMPLETED`
///   and explains nothing, because it got through everything it covered;
/// * a scope holding a covered recovery point the scan could NOT read
///   ([`index_searchable`]) also ends `COMPLETED` — the scan ran — but it is a completion
///   WITH ISSUES: part of the scope was left unscanned, so the status comes with the
///   message that says so ([`unscanned_points_message`]). The model requires exactly that
///   of the member: a status message is returned for a `COMPLETED` job with issues, and
///   the example it names is a search containing recovery points unable to be scanned
///   (`smithy_query(query="doc", shape="GetSearchJobOutput", member="StatusMessage")`).
pub fn scan_outcome(points: &[IndexedRecoveryPoint]) -> (&'static str, Option<String>) {
    if points.is_empty() {
        return (FAILED, Some(SCOPE_EMPTY.to_string()));
    }
    let unscanned = points.len() - searched_recovery_point_count(points);
    if unscanned == 0 {
        return (COMPLETED, None);
    }
    (COMPLETED, Some(unscanned_points_message(unscanned, points.len())))
}

/// The `StatusMessage` a scan that covered `covered` recovery points and could not read
/// `unscanned` of them reports, i.e. the text of a completion WITH ISSUES.
///
/// The model states no wording for this outcome, so the sentence it DOES state the case in
/// is the template: a search that "contained recovery points unable to be scanned"
/// (`smithy_query(query="doc", shape="GetSearchJobOutput", member="StatusMessage")`). Its
/// variable parts are counted from the job's own resolved scope — how many of the covered
/// recovery points the search was left out of — so the message a caller reads says how
/// much of the scope went unscanned instead of repeating one job's text. Which backups
/// those were, and the status each one's index was in, is what the listing that pages
/// [`SearchJob::backups`] displays.
pub fn unscanned_points_message(unscanned: usize, covered: usize) -> String {
    format!("Search contained {unscanned} of {covered} recovery points unable to be scanned")
}

/// Whether the backup index of `point` is in a state a search can read, and so whether
/// that recovery point takes part in the search at all.
///
/// An index that is built is read, and its items are the ones a scan gets through
/// ([`scanned_items`]); an index in any other state could not be read, so the recovery
/// point is left OUT of the search — none of its items is scanned, counted or matched,
/// and the backup itself is displayed, with the status and status message that say why, by
/// the listing that pages [`SearchJob::backups`], while the job's own `StatusMessage`
/// reports that part of the scope went unscanned ([`scan_outcome`]). A record reporting no
/// index status at all cannot be shown to hold a readable index, so it is left out too — the
/// same way the scope resolution treats a record it cannot judge ([`in_scope`]).
pub fn index_searchable(point: &SearchJobBackupsResult) -> bool {
    point
        .status
        .as_deref()
        .is_some_and(|status| SEARCHABLE_INDEX_STATUSES.contains(&status))
}

/// The recovery points of `points` a scan actually READS: the covered ones whose backup
/// index is in a state a search can read ([`index_searchable`]).
///
/// ONE filter behind both layers a job counts, so the backups it reports as to-be-scanned
/// and the items it scans cannot disagree about which of the covered backups the search
/// got into.
fn searched(points: &[IndexedRecoveryPoint]) -> impl Iterator<Item = &IndexedRecoveryPoint> {
    points
        .iter()
        .filter(|point| index_searchable(&point.backup))
}

/// How many of the covered `points` a scan reads, which is what
/// `SearchScopeSummary.TotalRecoveryPointsToScanCount` — "the total number of backups that
/// will be scanned in a search" (`smithy_query(query="shape",shape="SearchScopeSummary")`)
/// — reports, and what `CurrentSearchProgress.RecoveryPointsScannedCount` reaches once the
/// scan has got through them.
pub fn searched_recovery_point_count(points: &[IndexedRecoveryPoint]) -> usize {
    searched(points).count()
}

/// The items the backup indexes of `points` hold, in recovery-point order and then index
/// order, each naming the backup it was found in.
///
/// This is the ITEM layer of a scan: what a job over that resolved scope has to get
/// through (`SearchScopeSummary.TotalItemsToScanCount`,
/// `CurrentSearchProgress.ItemsScannedCount`) and the sequence its `ItemFilters` select
/// the matches from. The three members naming the backup are stamped from the recovery
/// point record the item was read out of, so a result never names a backup it was not
/// found in and never reports a vault the record does not hold.
///
/// Only a recovery point the search could get into contributes items ([`searched`]): the
/// items indexed under a backup whose index was in no state to be read are not part of
/// this sequence, so they are neither counted nor matched nor paged.
pub fn scanned_items(points: &[IndexedRecoveryPoint]) -> Vec<ResultItem> {
    searched(points)
        .flat_map(|point| point.items.iter().map(|item| item_of_backup(item, point)))
        .collect()
}

/// `item` as the scan read it out of `point`'s backup index: its own properties, with the
/// backup it was found in named from that record.
fn item_of_backup(item: &ResultItem, point: &IndexedRecoveryPoint) -> ResultItem {
    let backup_resource_arn = point.backup.backup_resource_arn.clone();
    let source_resource_arn = point.backup.source_resource_arn.clone();
    let backup_vault_name = point.backup_vault_name.clone();
    match item.clone() {
        ResultItem::S3ResultItem(object) => ResultItem::S3ResultItem(S3ResultItem {
            backup_resource_arn,
            source_resource_arn,
            backup_vault_name,
            ..object
        }),
        ResultItem::EBSResultItem(file) => ResultItem::EBSResultItem(EBSResultItem {
            backup_resource_arn,
            source_resource_arn,
            backup_vault_name,
            ..file
        }),
    }
}

/// Whether `filters` admit `item`, i.e. whether a scan over the item layer records it as
/// a match.
///
/// `ItemFilters` carries one list per item type and each list holds up to ten filter
/// objects, so the rule has three levels: an item is admitted when ANY filter object of
/// the list for ITS OWN type matches it, a filter object matches when EVERY item property
/// it constrains is satisfied, and a property is satisfied by ANY ONE of the conditions
/// listed for it — which is what the model documents for every one of those condition
/// lists ("if one value is included, the results will return only items that match the
/// value. If more than one value is included, the results will return all items that
/// match any of the values").
///
/// A list that is absent or empty constrains nothing and admits every item, exactly as an
/// absent scope filter admits every recovery point — and so does the list for the OTHER
/// item type, which says nothing about items of this one.
pub fn item_matches_filters(item: &ResultItem, filters: &ItemFilters) -> bool {
    match item {
        ResultItem::S3ResultItem(object) => {
            satisfied_by_any(filters.s3_item_filters.as_deref(), |filter| {
                s3_filter_matches(filter, object)
            })
        }
        ResultItem::EBSResultItem(file) => {
            satisfied_by_any(filters.ebs_item_filters.as_deref(), |filter| {
                ebs_filter_matches(filter, file)
            })
        }
    }
}

/// Whether one `S3ItemFilter` matches a backed-up object: every property the filter
/// constrains is satisfied, and a property it leaves out constrains nothing.
fn s3_filter_matches(filter: &S3ItemFilter, object: &S3ResultItem) -> bool {
    satisfied_by_any(filter.object_keys.as_deref(), |condition| {
        string_condition_met(condition, object.object_key.as_deref())
    }) && satisfied_by_any(filter.sizes.as_deref(), |condition| {
        long_condition_met(condition, object.object_size)
    }) && satisfied_by_any(filter.creation_times.as_deref(), |condition| {
        time_condition_met(condition, object.creation_time)
    }) && satisfied_by_any(filter.version_ids.as_deref(), |condition| {
        string_condition_met(condition, object.version_id.as_deref())
    }) && satisfied_by_any(filter.e_tags.as_deref(), |condition| {
        string_condition_met(condition, object.e_tag.as_deref())
    })
}

/// Whether one `EBSItemFilter` matches a backed-up file, by the same rule
/// [`s3_filter_matches`] applies to an object.
fn ebs_filter_matches(filter: &EBSItemFilter, file: &EBSResultItem) -> bool {
    satisfied_by_any(filter.file_paths.as_deref(), |condition| {
        string_condition_met(condition, file.file_path.as_deref())
    }) && satisfied_by_any(filter.sizes.as_deref(), |condition| {
        long_condition_met(condition, file.file_size)
    }) && satisfied_by_any(filter.creation_times.as_deref(), |condition| {
        time_condition_met(condition, file.creation_time)
    }) && satisfied_by_any(filter.last_modification_times.as_deref(), |condition| {
        time_condition_met(condition, file.last_modified_time)
    })
}

/// Whether an optional list of ALTERNATIVES is satisfied: an absent or empty list states
/// no requirement at all, and a list that states one is satisfied by any ONE of its
/// entries. One helper behind both levels of the item filters that read this way — the
/// filter objects of a list, and the conditions of one property.
fn satisfied_by_any<T>(alternatives: Option<&[T]>, met: impl Fn(&T) -> bool) -> bool {
    alternatives.is_none_or(|list| list.is_empty() || list.iter().any(met))
}

/// Whether `candidate` satisfies one `StringCondition`.
///
/// A property the item does not carry cannot be SHOWN to satisfy a condition on it, so it
/// satisfies none — the same rule the scope resolution applies to a record it cannot
/// judge. An operator outside the model's enum is likewise not satisfied (restJson1 enums
/// are open on the wire, so an unknown value reaches here) rather than silently admitting
/// every item.
fn string_condition_met(condition: &StringCondition, candidate: Option<&str>) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };
    let value = condition.value.as_str();
    match condition_operator(&condition.operator) {
        EQUALS_TO => candidate == value,
        NOT_EQUALS_TO => candidate != value,
        CONTAINS => candidate.contains(value),
        DOES_NOT_CONTAIN => !candidate.contains(value),
        BEGINS_WITH => candidate.starts_with(value),
        DOES_NOT_BEGIN_WITH => !candidate.starts_with(value),
        ENDS_WITH => candidate.ends_with(value),
        DOES_NOT_END_WITH => !candidate.ends_with(value),
        _ => false,
    }
}

/// Whether `candidate` satisfies one `LongCondition` — an item's size, by the operators
/// `LongConditionOperator` declares. Judged like [`string_condition_met`]: a size the item
/// does not carry satisfies nothing.
fn long_condition_met(condition: &LongCondition, candidate: Option<i64>) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };
    match condition_operator(&condition.operator) {
        EQUALS_TO => candidate == condition.value,
        NOT_EQUALS_TO => candidate != condition.value,
        LESS_THAN_EQUAL_TO => candidate <= condition.value,
        GREATER_THAN_EQUAL_TO => candidate >= condition.value,
        _ => false,
    }
}

/// Whether `candidate` satisfies one `TimeCondition` — an item's creation or last
/// modification instant, by the operators `TimeConditionOperator` declares. The two
/// instants are compared as ordered keys ([`instant_key`]), so sub-second precision
/// counts.
fn time_condition_met(condition: &TimeCondition, candidate: Option<DateTime>) -> bool {
    let Some(candidate) = candidate.map(instant_key) else {
        return false;
    };
    let value = instant_key(condition.value);
    match condition_operator(&condition.operator) {
        EQUALS_TO => candidate == value,
        NOT_EQUALS_TO => candidate != value,
        LESS_THAN_EQUAL_TO => candidate <= value,
        GREATER_THAN_EQUAL_TO => candidate >= value,
        _ => false,
    }
}

/// The operator a condition compares with: the one the request spelled, or the model's
/// `@default("EQUALS_TO")` when it left the member out — the framework leaves an absent
/// enum member as the empty string, which is not an operator.
fn condition_operator(operator: &str) -> &str {
    if operator.is_empty() {
        EQUALS_TO
    } else {
        operator
    }
}

/// A timestamp for `instant`, truncated to milliseconds — the precision the model's
/// own documentation gives these members ("accurate to milliseconds").
pub fn timestamp(instant: SystemTime) -> DateTime {
    let millis = instant
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    DateTime::from_millis(millis)
}

/// A `NextToken` for the page that resumes at `position` of the sequence `context` names.
///
/// The context is part of the token because a position alone says nothing about WHICH
/// sequence it indexes: a token issued for one `ByStatus` filter (or one search job's
/// results) would otherwise silently resume a different sequence at that position and
/// answer 200 with a page that skips or repeats entries.
///
/// Which END of the sequence the position is counted from is [`PageAnchor`]'s decision,
/// and is what keeps the number resumable after the store changed under the token.
pub fn encode_next_token(context: &str, position: usize) -> String {
    format!("{position}@{context}")
}

/// Which end of a listing's held sequence a `NextToken` counts its resume position from.
///
/// A token is resumable only while the number inside it still names the same place in the
/// sequence it was issued over, and the only change a held sequence of this service
/// undergoes between two pages of one walk is a record being APPENDED to the creation
/// order the sequence is built from — nothing is removed and nothing is reordered. So the
/// position has to be counted from the end that appending does not move, which depends on
/// the order the listing REPORTS its records in: a walk whose cursor is anchored at the
/// moving end repeats the entries a new record pushed past the cursor and never reports
/// that record either.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageAnchor {
    /// The listing reports its held sequence in the order it is appended to, so a new
    /// record lands AFTER every position already handed out and a position counted from
    /// the front keeps naming the same entry.
    CreationOrder,
    /// The listing reports the REVERSE of the sequence that is appended to, so a new
    /// record lands at the front and pushes every older entry one place later; a position
    /// counted from the BACK — the number of entries still unreported behind it — is the
    /// one an append leaves alone.
    NewestFirst,
}

impl PageAnchor {
    /// The number a token carries for a page of a sequence of `len` entries that examined
    /// everything up to `end`: the resume position, counted from this anchor's end.
    fn position(self, len: usize, end: usize) -> usize {
        match self {
            PageAnchor::CreationOrder => end,
            PageAnchor::NewestFirst => len.saturating_sub(end),
        }
    }

    /// Where a token carrying `position` resumes, as an offset into a sequence of `len`
    /// entries in the order the listing reports them — the inverse of
    /// [`position`](Self::position), and `None` for a position outside such a sequence.
    fn offset(self, len: usize, position: usize) -> Option<usize> {
        match self {
            PageAnchor::CreationOrder => (position <= len).then_some(position),
            PageAnchor::NewestFirst => len.checked_sub(position),
        }
    }
}

/// Where one page of a listing starts, and which end of that listing's sequence the token
/// for the page after it counts from.
///
/// Carried from [`decode_next_token`] to [`page_matching`] so the two cannot disagree
/// about the anchor — a token decoded against one end and issued against the other would
/// resume somewhere nobody asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageCursor {
    /// The offset the page starts at, in the sequence as the listing reports it.
    offset: usize,
    anchor: PageAnchor,
}

impl PageCursor {
    /// The cursor the FIRST page of a listing anchored at `anchor` starts from.
    pub fn start(anchor: PageAnchor) -> Self {
        Self { offset: 0, anchor }
    }

    /// Where this page starts in the sequence as the listing reports it.
    pub fn offset(self) -> usize {
        self.offset
    }
}

/// Where a `NextToken` resumes, as a cursor into the sequence `context` names — the
/// sequence of records this store HOLDS for that listing, in the order the listing
/// reports them, before any of the request's filters narrow it ([`page_matching`]).
///
/// `len` is the length of that held sequence and never of a filtered view of it, and
/// `anchor` is the end of it the token counts from ([`PageAnchor`]). Which of its records
/// a filter keeps can change after a token was issued, because a record's own status moves
/// on with no caller involved; a position in the held sequence, counted from the end
/// appending does not move, keeps naming the same record for as long as the store holds
/// it.
///
/// `Err` is the caller's problem: a token this service did not issue, one issued for a
/// different sequence, or one that points outside what it is paging over.
pub fn decode_next_token(
    token: Option<&str>,
    context: &str,
    len: usize,
    anchor: PageAnchor,
) -> Result<PageCursor, String> {
    let Some(token) = token else {
        return Ok(PageCursor::start(anchor));
    };
    let malformed =
        || format!("NextToken '{token}' is not a pagination token this service issued");
    let (position, token_context) = token.split_once('@').ok_or_else(malformed)?;
    if token_context != context {
        return Err(format!(
            "NextToken '{token}' was issued for a different request and cannot resume this one"
        ));
    }
    let position = token_position(position).ok_or_else(malformed)?;
    let offset = anchor.offset(len, position).ok_or_else(|| {
        format!("NextToken '{token}' resumes past the end of the results")
    })?;
    Ok(PageCursor { offset, anchor })
}

/// The resume position the text before a token's `@` names, and `None` when that text is
/// not the one [`encode_next_token`] would have written for any position.
///
/// A cursor is resumable only AS THE STRING the service handed out, so the number has to
/// be held to the single spelling the encoder renders it in — a `usize`'s canonical
/// decimal. `usize::from_str` by itself is wider than that: it also admits a leading `+`
/// and any number of leading zeros, spellings no token this service issued can carry, so
/// the parse is confirmed by rendering the number back and requiring the text to be what
/// the encoder would have put there.
fn token_position(text: &str) -> Option<usize> {
    let position: usize = text.parse().ok()?;
    (text == position.to_string()).then_some(position)
}

/// One page of `items` starting at `cursor`, with the token that resumes after it — the
/// whole sequence, which is what a listing carrying no filter pages. The paging rule
/// itself is [`page_matching`]'s.
pub fn page<T: Clone>(
    items: &[T],
    cursor: PageCursor,
    max_results: usize,
    context: &str,
) -> (Vec<T>, Option<String>) {
    page_matching(items, cursor, max_results, context, |_| true)
}

/// One page of the entries of `items` that `admits` keeps, starting at `cursor`, with the
/// token that resumes after it. The token carries `context`, the sequence it indexes, and
/// counts its position from the end `cursor`'s anchor names.
///
/// The cursor is a position in `items` — the sequence the store HOLDS, in the order the
/// listing reports it — and not in the shorter subsequence a request's filter leaves of
/// it, and the token this returns resumes after the last entry the page examined rather
/// than after the last one it carried. That is what keeps every token this service issues
/// resumable: a filter's membership can change between two calls of one paginating walk,
/// since a record's status advances on its own, so a position in the filtered subsequence
/// can fall outside it by the time the next page is asked for, while a position in the
/// held sequence cannot. A filter that now keeps fewer entries simply answers a shorter
/// page — down to an empty last one — instead of turning a token the service issued into
/// a refusal. A record APPENDED to the held sequence while the walk is in flight does not
/// move that position either, because the token counts from the end appending leaves
/// alone ([`PageAnchor`]): the walk reports every record the store held throughout it
/// exactly once, and a record created after the walk began is reported by the next walk,
/// which starts at the sequence's new head.
///
/// A page carries a `NextToken` whenever it is FULL — it holds as many entries as the
/// requested page size — because a full page is all this call was asked for and says
/// nothing about what follows it. A short page is the end of the sequence and carries
/// none, which is how a paginating client knows to stop: it follows the token until a
/// page comes back short, and the page a token at the very end of the sequence resumes
/// is the empty one.
pub fn page_matching<T: Clone>(
    items: &[T],
    cursor: PageCursor,
    max_results: usize,
    context: &str,
    admits: impl Fn(&T) -> bool,
) -> (Vec<T>, Option<String>) {
    let mut page = Vec::new();
    let mut end = cursor.offset().min(items.len());
    while end < items.len() && page.len() < max_results {
        if admits(&items[end]) {
            page.push(items[end].clone());
        }
        end += 1;
    }
    let next = (page.len() == max_results)
        .then(|| encode_next_token(context, cursor.anchor.position(items.len(), end)));
    (page, next)
}

/// One VALUE written into a page context: the string with its byte length in front of
/// it, so the value cannot be read as anything but itself.
///
/// A context has to name its sequence INJECTIVELY — two requests paging different
/// sequences must never produce the same string — because [`decode_next_token`] can only
/// compare the two strings and a collision is exactly the resume-the-wrong-sequence
/// confusion the context is in the token to prevent. The values that go into one are
/// request data under no syntactic constraint (the model puts a `@pattern` on none of
/// these members), so a value can spell any separator the context uses and any word that
/// stands for something else in it; a length-prefixed value is self-delimiting and
/// therefore cannot.
fn context_value(value: &str) -> String {
    format!(":{}:{value}", value.len())
}

/// One FILTER written into a page context: its value ([`context_value`]) when the request
/// carried the filter, and otherwise a marker that no present value renders as — a bare
/// `-` where a value always starts with its decimal length.
///
/// Spelling "no filter" with an in-band word instead would make the unfiltered sequence's
/// context byte-identical to the one a request carrying that very word as its filter
/// value produces, so a token issued for either would resume the other.
fn context_filter(value: Option<&str>) -> String {
    match value {
        Some(value) => context_value(value),
        None => ":-".to_string(),
    }
}

/// The sequence a `ListSearchJobs` page indexes: the jobs in this status, or every job
/// when the request carries no `ByStatus`. Part of the page's `NextToken`, so a token
/// cannot resume a differently filtered listing.
pub fn jobs_page_context(by_status: Option<&str>) -> String {
    format!("jobs{}", context_filter(by_status))
}

/// The sequence a `ListSearchJobResults` page indexes: one search job's results.
pub fn results_page_context(identifier: &str) -> String {
    format!("results{}", context_value(identifier))
}

/// The sequence a `ListSearchJobBackups` page indexes: one search job's backups.
pub fn backups_page_context(identifier: &str) -> String {
    format!("backups{}", context_value(identifier))
}

/// The sequence a `ListSearchResultExportJobs` page indexes: the export jobs left by
/// both of that request's filters. Part of the page's `NextToken` for the same reason
/// [`jobs_page_context`] is: an offset alone does not say which filtered sequence it
/// indexes.
pub fn export_jobs_page_context(by_status: Option<&str>, by_search_job: Option<&str>) -> String {
    format!(
        "export-jobs{}{}",
        context_filter(by_status),
        context_filter(by_search_job)
    )
}

/// A canonical rendering of one request's body, stored with the resource it created so a
/// later call under the same `ClientToken` can be told apart: the same body is a repeat,
/// a different body is a conflict.
///
/// The body is re-serialized with the protocol's own JSON codec — so exactly the
/// modelled members are compared, however the client sent them — with every object's
/// members put in key order, because a JSON object's member order is not part of the
/// request while a list's order is. One helper for every `Start` operation, so two
/// idempotency checks cannot disagree about what "the same body" means.
pub fn request_fingerprint(
    schema: &'static ::aws_smithy_schema::Schema,
    input: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Result<String, String> {
    use ::aws_smithy_schema::codec::{Codec, FinishSerializer};
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer =
        ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    serializer
        .write_struct(schema, input)
        .map_err(|error| error.to_string())?;
    let json = serializer.finish();
    let value: ::serde_json::Value =
        ::serde_json::from_slice(&json).map_err(|error| error.to_string())?;
    ::serde_json::to_string(&key_ordered(&value)).map_err(|error| error.to_string())
}

/// `value` rebuilt with every object's members in key order, so two bodies differing
/// only in member order render identically.
///
/// The ordering is imposed here rather than left to `serde_json::Map`, whose iteration
/// order is sorted only while nothing in the build enables the crate's `preserve_order`
/// feature; inserting in sorted order is correct either way.
fn key_ordered(value: &::serde_json::Value) -> ::serde_json::Value {
    match value {
        ::serde_json::Value::Object(members) => {
            let ordered: ::std::collections::BTreeMap<String, ::serde_json::Value> = members
                .iter()
                .map(|(name, member)| (name.clone(), key_ordered(member)))
                .collect();
            ::serde_json::Value::Object(ordered.into_iter().collect())
        }
        ::serde_json::Value::Array(elements) => {
            ::serde_json::Value::Array(elements.iter().map(key_ordered).collect())
        }
        scalar => scalar.clone(),
    }
}

/// The page size to serve: the requested `MaxResults` clamped to the modelled maximum,
/// or the modelled default when the member is absent (the framework leaves an absent
/// integer at zero).
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(requested) if requested > 0 => requested.min(MAX_PAGE_SIZE),
        _ => DEFAULT_MAX_RESULTS,
    }
}

/// Refuse one tag entry whose key or value is longer than this store's cap or carries a
/// character outside the set a tag of this service may use, naming the member and the
/// constraint the way a validation refusal does.
///
/// Every operation that WRITES a tag goes through here — `TagResource` merging into a
/// stored record and the two `Start` operations minting one — so a tag AWS refuses never
/// becomes state of any resource this service holds.
pub fn check_tag_entry(key: &str, value: &str) -> Result<(), String> {
    let key_length = key.chars().count();
    if key_length > MAX_TAG_KEY_LENGTH {
        return Err(format!(
            "Tags key is {key_length} characters, more than the maximum of {MAX_TAG_KEY_LENGTH}"
        ));
    }
    let value_length = value.chars().count();
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Err(format!(
            "Tags value for key '{key}' is {value_length} characters, more than the maximum of {MAX_TAG_VALUE_LENGTH}"
        ));
    }
    if let Some(character) = disallowed_tag_character(key) {
        return Err(format!(
            "Tags key '{key}' contains '{character}', which is not one of the allowed characters: \
             {ALLOWED_TAG_CHARACTERS}"
        ));
    }
    if let Some(character) = disallowed_tag_character(value) {
        return Err(format!(
            "Tags value for key '{key}' contains '{character}', which is not one of the allowed \
             characters: {ALLOWED_TAG_CHARACTERS}"
        ));
    }
    Ok(())
}

/// The first character of `text` outside the set a tag key or value may use, or `None`
/// when every one of them is in it. The scan is over CHARACTERS, so a multi-byte letter
/// is one letter and never a run of bytes outside the set.
pub fn disallowed_tag_character(text: &str) -> Option<char> {
    text.chars().find(|character| !tag_character_allowed(*character))
}

/// Whether one character is in the set a tag key or value may use: a letter, a number, a
/// space, or one of [`TAG_PUNCTUATION`].
///
/// `char::is_alphanumeric` is what carries "UTF-8 letters, numbers": it admits every
/// Unicode letter and digit, not just the ASCII ones, so a tag in a non-Latin script is
/// as acceptable as an English one. Everything else — `*`, `#`, `@`, `,`, `"`, a control
/// character, a tab or a newline — is outside the set.
pub fn tag_character_allowed(character: char) -> bool {
    character.is_alphanumeric() || character == ' ' || TAG_PUNCTUATION.contains(&character)
}

/// Refuse a tag set a caller submitted unless the whole of it may be written: every
/// submitted entry passes [`check_tag_entry`], and the number of tags the record would
/// hold once the write landed passes [`check_tag_count`].
///
/// This is the one admission rule every operation that writes a tag applies — the two
/// `Start` operations minting a record's tag set and `TagResource` merging into a stored
/// one — so the per-entry check, the count that is checked, the order of the two checks
/// and the refusal channel are decided here once instead of per handler.
///
/// `stored` is the tag set the write merges into: `None` for a create, which starts from
/// nothing, so the post-write count is simply what the request carries; for a merge it is
/// the entries already held, and the post-write count adds only the submitted keys that
/// are not among them, because a submitted key that is already stored is overwritten
/// rather than added.
///
/// The whole set is checked before anything is written, so a refused call leaves no
/// partial write and a later read cannot report an entry this service refused.
///
/// `E` is NOT a choice this helper makes: the refusal it answers with is always the
/// `ValidationException` variant of the error wrapper, and `E` is only the calling
/// operation's modelled error enum, which every handler's own error type fixes, so each
/// call site writes `check_tag_map(..)?` and names nothing.
pub fn check_tag_map<E>(
    submitted: &HashMap<String, String>,
    stored: Option<&HashMap<String, String>>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    for (key, value) in submitted {
        check_tag_entry(key, value).map_err(crate::scaffolded::Scaffolded::validation)?;
    }
    let count = match stored {
        Some(stored) => {
            stored.len()
                + submitted
                    .keys()
                    .filter(|key| !stored.contains_key(*key))
                    .count()
        }
        None => submitted.len(),
    };
    check_tag_count(count).map_err(crate::scaffolded::Scaffolded::validation)
}

/// Refuse a tag set that would leave one job holding more than [`MAX_TAGS_PER_JOB`]
/// tags. `count` is the number of tags the job would hold after the write.
pub fn check_tag_count(count: usize) -> Result<(), String> {
    if count > MAX_TAGS_PER_JOB {
        return Err(format!(
            "Tags would hold {count} entries, more than the maximum of {MAX_TAGS_PER_JOB} for one resource"
        ));
    }
    Ok(())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBackupsearchCryobackupsearchserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBackupsearchCryobackupsearchserviceState>)
}
