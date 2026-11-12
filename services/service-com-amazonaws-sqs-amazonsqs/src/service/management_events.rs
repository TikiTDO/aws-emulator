// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every QUEUE-level call it serves is a management event of
//! `sqs.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it — and, while a trail of the Region is logging calls of its kind,
//! published onto the account's default event bus as `AWS API Call via CloudTrail`
//! ([`service_common::api_call_events`]).
//!
//! Every control-plane call of an account is a management event, which makes the recording
//! a property of SERVING a call and not of any one operation — so it is not written per
//! handler here. The shared wrapper ([`service_common::audited_operation`]) is handed the
//! deserialized input before the handler runs, the handler's own `Result` after, and the
//! rendered response last, and this module is only what THIS service contributes to each
//! record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region the call was served in and the account it was served for — this service is
//!   REGIONAL, so a record of it is filed in the Region the request was signed for and the
//!   event published for it is published in that Region's bus;
//! * the session a call's temporary credential belongs to, so a call made by a role session
//!   is attributed to that session and to the role that issued it; and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot read
//!   off a request, so it is stated per operation in [`audited_table`].
//!
//! Every request this service serves reaches that table, because it is the routing table
//! this service's definition answers with ([`AuditedDefinition`]) — the error-envelope
//! wrapper this service serves every request through resolves the operation from that same
//! table, so no request can be served by a path that skips it.
//!
//! # What is NOT a management event here
//!
//! The per-MESSAGE data plane — the sends, the receives, the deletes and the visibility
//! changes ([`DATA_PLANE_OPERATIONS`]) — is not a management event of the account, so those
//! operations are filed in the table UNRECORDED: they appear in no event history and
//! publish nothing onto the event bus. They are still served exactly as before.
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded either: the
//! wrapper drops it, because event history is the record of the calls an account's APIs
//! actually served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable};
use service_common::management_events::{
    ManagementEventLog, RecordedSession, ServedApiCall, SessionAuditDirectory,
};
use smithy_server_runtime::{
    ErasedOperation, Operation, OperationContext, OperationRoutingTable, PreDispatchProtocol,
};

/// This service's model-derived half, routing through the AUDITED operations — the shared
/// wrapper every recording service uses, re-exported under the name this service's
/// `EmulatedService` holds it by.
pub use service_common::audited_operation::AuditedDefinition;

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: sqs`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
pub const EVENT_SOURCE: &str = "sqs.amazonaws.com";

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
#[derive(Debug)]
pub struct SqsAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> SqsAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        management_event_log(&*self.engine)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for SqsAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in the log by definition, so
            // it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's.
        // The guard is BOUND rather than used as a temporary: a local is released before
        // the handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session and as the role that issued it: the shared store the
        // minting service wrote it to.
        session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read the way every handler of this service
        // reads it — which is also the Region of every queue ARN the call names. This
        // service is regional, so that is the Region its records are filed in and the
        // Region the event published for the call belongs to.
        crate::state::request_region(ctx.parts())
    }

    fn account_id(&self) -> String {
        ::engine::emulator_account_id().to_string()
    }

    // `resources` is the shared recorder's default: none. The pairs CloudTrail reports in
    // an event's `Resources` are not this change's to invent — a record naming a resource
    // nothing resolved would be reported to a lookup by `ResourceName` as fact — so a
    // record of a call to this service carries the call, its parameters and its outcome,
    // and no resource list.

    fn sensitive_members(&self) -> &'static [&'static str] {
        // This service's model declares no `@sensitive` member at all (its shapes carry no
        // such trait), so no value of a recorded call is replaced by the redaction literal.
        &[]
    }

    // `stopped_at_authorization` is the shared recorder's default: NONE. This service
    // decides a queue's resource policy and a caller's identity policies while SERVING the
    // call — the queue is resolved first, and the refusal is answered by the operation
    // itself (`crate::state::authorization`) — so every refusal it answers belongs to a
    // call that WAS dispatched and its record carries the parameters it was dispatched
    // with.
}

/// The account's management-event log in the shared engine.
///
/// A SHARED type ([`service_common::management_events::ManagementEventLog`]) and not a
/// field of this service's own state: CloudTrail reports the management calls of every
/// service, and the service that served a call is the only one that knows it happened, so
/// recorder and reporter reach one log rather than two copies of it.
pub fn management_event_log<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ManagementEventLog>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ManagementEventLog>)
}

/// The account's SESSION directory in that same shared engine: the sessions the token
/// service minted, each keyed by the access key id it was minted under, which is what lets
/// a record of a call made with one name the session it was made by.
///
/// A SHARED type for the same reason the log is: the service that MINTS a session and the
/// services that record the calls made WITH it are not the same service.
pub fn session_audit_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<SessionAuditDirectory>)
}

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsJson1_0`, which is the same value the generated per-operation
/// shim passes.
const JSON_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Json {
    content_type: "application/x-amz-json-1.0",
    // The awsJson families convey the error type in the body's `__type`, not in a header.
    errortype_header: false,
};

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// The kind per row names the shared table's own method for it
/// ([`AuditedTable::reports`] / [`AuditedTable::mutates`]), so nothing about what a kind
/// means is restated here.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.$kind(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
            );
        )*
    }};
}

/// File one operation of this service in the table UNRECORDED, under the modelled name the
/// protocol resolves for it ([`Operation::NAME`]) — the message data plane, which is no
/// management event of the account (see this module's docs).
macro_rules! unrecorded {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident ),* $(,)?
    ) => {{
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                Box::new(crate::ops::$module::$operation::new(
                    ::std::sync::Arc::clone($engine),
                )) as Box<dyn ErasedOperation>,
            );
        )*
    }};
}

/// The per-MESSAGE operations this service serves that are NOT management events of the
/// account: the sends, the receives, the deletes and the visibility changes.
///
/// A management event is a control-plane call — one that creates, configures, inspects or
/// removes a RESOURCE of the account. Moving a message through a queue that already exists
/// changes no resource and is not one, so none of these is recorded in event history and
/// none is published onto the event bus; a client auditing them asks the queue, not
/// CloudTrail. Named here as documentation of the set [`audited_table`] files unrecorded.
pub const DATA_PLANE_OPERATIONS: &[&str] = &[
    "ChangeMessageVisibility",
    "ChangeMessageVisibilityBatch",
    "DeleteMessage",
    "DeleteMessageBatch",
    "ReceiveMessage",
    "SendMessage",
    "SendMessageBatch",
];

/// Every modelled operation of this service, each constructed from the shared engine: the
/// queue-level ones served behind the audit recorder, the message data plane filed
/// unrecorded beside them ([`DATA_PLANE_OPERATIONS`]).
///
/// EVERY one is here, so a request can only be served through this table and the operation
/// it resolves is the one the model names for the request.
///
/// The kind per audited row is the operation's own semantics: `reports` for the `Get*` /
/// `List*` readers, which report a queue's configuration, its tags or the account's queues
/// and change none of it, and `mutates` for every operation that creates, configures,
/// tags, purges, moves or removes something the account then holds.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(SqsAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, JSON_ENVELOPE);
    audited!(
        table, engine,
        add_permission::AddPermission => mutates,
        cancel_message_move_task::CancelMessageMoveTask => mutates,
        create_queue::CreateQueue => mutates,
        delete_queue::DeleteQueue => mutates,
        get_queue_attributes::GetQueueAttributes => reports,
        get_queue_url::GetQueueUrl => reports,
        list_dead_letter_source_queues::ListDeadLetterSourceQueues => reports,
        list_message_move_tasks::ListMessageMoveTasks => reports,
        list_queue_tags::ListQueueTags => reports,
        list_queues::ListQueues => reports,
        purge_queue::PurgeQueue => mutates,
        remove_permission::RemovePermission => mutates,
        set_queue_attributes::SetQueueAttributes => mutates,
        start_message_move_task::StartMessageMoveTask => mutates,
        tag_queue::TagQueue => mutates,
        untag_queue::UntagQueue => mutates,
    );
    let mut table = table.finish();
    unrecorded!(
        table, engine,
        change_message_visibility::ChangeMessageVisibility,
        change_message_visibility_batch::ChangeMessageVisibilityBatch,
        delete_message::DeleteMessage,
        delete_message_batch::DeleteMessageBatch,
        receive_message::ReceiveMessage,
        send_message::SendMessage,
        send_message_batch::SendMessageBatch,
    );
    table
}
