// SPDX-License-Identifier: CC0-1.0

//! DOMAIN CONTROLLER SECURITY LOG FORWARDING: what a directory's audited acts DELIVER to
//! the CloudWatch Logs log group its log subscription names.
//!
//! `CreateLogSubscription` "creates a subscription to forward real-time Directory Service
//! domain controller security logs to the specified Amazon CloudWatch log group", and
//! `DeleteLogSubscription` "deletes the specified log subscription". Storing the subscribed
//! group and reporting it back through `ListLogSubscriptions` is NOT that behaviour: the
//! group has to RECEIVE the records, or the presence and the absence of forwarding read the
//! same to every client — a SIEM ingesting the group, an alarm on account-creation events,
//! a metric filter over it.
//!
//! # What produces a record
//!
//! A directory's domain controllers audit the acts that change the directory's own objects,
//! and it is those Security-log records that forwarding carries. So a record is produced by
//! the very call that makes the change, in this service (a computer account created, an
//! account's password reset) and in the Directory Service DATA plane that writes the users,
//! groups and memberships inside the same directory — that plane calls [`record`] for its
//! own writes, which is why this module is the crate's public seam rather than private to
//! an operation.
//!
//! An act is forwarded only while the subscription that forwards it EXISTS: a change made
//! before `CreateLogSubscription`, or after `DeleteLogSubscription` has taken the
//! subscription away, produces nothing in the group, because the record is produced and
//! delivered at the instant of the act and nothing replays the directory's history.
//!
//! The records come from the directory's DOMAIN CONTROLLERS, so one is forwarded only while
//! the directory has a controller in service ([`SERVING_STATUS`]), and the log stream it is
//! written to is named for that controller — the same `DomainControllerId`
//! `DescribeDomainControllers` reports — so a reader can tell which controller logged what.
//!
//! # Where it goes
//!
//! Into the CloudWatch Logs service's OWN store in the SAME shared engine, through that
//! service's own record types, so what was forwarded is exactly what
//! `logs:FilterLogEvents`, `logs:GetLogEvents` and `logs:DescribeLogStreams` report — the
//! way Amazon MWAA creates a log group in it (`service-com-amazonaws-mwaa-amazonmwaa`'s
//! `state::log_groups`) and Step Functions delivers its history events to one. It is
//! written there directly rather than sent over `logs:PutLogEvents` because the act that
//! produces the record is resolved under this service's own store guard and answers in one
//! step: going out over the in-process API would mean awaiting with a guard held.
//!
//! Delivery is BEST EFFORT, as forwarding to a customer-owned destination is: a group
//! CloudWatch Logs does not hold receives nothing — nothing creates a group here, since the
//! subscription names a group the account already owns — and a group with no room for
//! another stream takes no further record. Either way the directory call that produced the
//! record still answers its caller, which asked for none of this; the loss is traced.
//!
//! The destination is IN-PROCESS and nothing else: the subscribed group is a record in this
//! emulator's own CloudWatch Logs store, so no client is configured, no address is resolved
//! and no connection is made for a delivery. How much the deliveries can hold is the owning
//! store's own business, and they are admitted against its ceilings
//! ([`logs::MAX_STREAMS_PER_GROUP`] and one stream's own retention), so no number of
//! directory writes can grow it past what that service already allows.
//!
//! LOCK ORDER: [`record`] takes THIS service's store, releases it, and only then takes the
//! CloudWatch Logs store, so the two are never held at once and a caller holding its own
//! service's store cannot invert the order.

use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

/// The Windows event log channel these records come from: the Security log, which is the
/// one domain controller log `CreateLogSubscription` forwards.
pub const SECURITY_CHANNEL: &str = "Security";

/// The `DomainControllerStatus` a controller must be in for its Security log to be
/// forwarded: a controller still being placed is not yet auditing anything.
pub const SERVING_STATUS: &str = super::DOMAIN_CONTROLLER_STATUS_ACTIVE;

/// One audited act, as the Security log of a Windows domain controller records it: the
/// event id of the record and the sentence that record states.
///
/// The ids and their sentences are Windows' own, from Microsoft's security auditing event
/// reference — the `Audit User Account Management`, `Audit Security Group Management` and
/// `Audit Computer Account Management` subcategories a domain controller audits directory
/// object changes under — because the records AWS forwards are the controller's own
/// Security log entries rather than anything this API models. The catalog below is the set
/// of acts this emulator's two Directory Service planes can perform on a directory's
/// objects; nothing else is logged, because nothing else changes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditedAct {
    /// The Windows Security-log event id, e.g. 4720 for an account created.
    pub event_id: u32,
    /// The sentence the record states, as Windows words it.
    pub description: &'static str,
}

/// A user account was created (`ds-data:CreateUser`).
pub const USER_ACCOUNT_CREATED: AuditedAct = AuditedAct {
    event_id: 4720,
    description: "A user account was created.",
};

/// An account's password was reset by an administrator (`ds:ResetUserPassword`).
pub const USER_PASSWORD_RESET: AuditedAct = AuditedAct {
    event_id: 4724,
    description: "An attempt was made to reset an account's password.",
};

/// A user account was deleted (`ds-data:DeleteUser`).
pub const USER_ACCOUNT_DELETED: AuditedAct = AuditedAct {
    event_id: 4726,
    description: "A user account was deleted.",
};

/// A user account was changed (`ds-data:UpdateUser`).
pub const USER_ACCOUNT_CHANGED: AuditedAct = AuditedAct {
    event_id: 4738,
    description: "A user account was changed.",
};

/// A security-enabled global group was created (`ds-data:CreateGroup`).
pub const GROUP_CREATED: AuditedAct = AuditedAct {
    event_id: 4727,
    description: "A security-enabled global group was created.",
};

/// A member was added to a security-enabled global group (`ds-data:AddGroupMember`).
pub const GROUP_MEMBER_ADDED: AuditedAct = AuditedAct {
    event_id: 4728,
    description: "A member was added to a security-enabled global group.",
};

/// A member was removed from a security-enabled global group
/// (`ds-data:RemoveGroupMember`).
pub const GROUP_MEMBER_REMOVED: AuditedAct = AuditedAct {
    event_id: 4729,
    description: "A member was removed from a security-enabled global group.",
};

/// A computer account was created (`ds:CreateComputer`).
pub const COMPUTER_ACCOUNT_CREATED: AuditedAct = AuditedAct {
    event_id: 4741,
    description: "A computer account was created.",
};

/// One Security-log record a directory's domain controller produced, with everything the
/// write into the subscribed log group needs.
///
/// Built while this service's store is held and delivered once it is released, so nothing
/// here reaches back into the directory it came from.
#[derive(Debug, Clone)]
pub struct ForwardedEvent {
    /// The log group the directory's subscription names — as `CreateLogSubscription`
    /// spelled it, never a group of this module's own naming.
    pub log_group_name: String,
    /// The stream the record is written to: the `DomainControllerId` of the controller that
    /// logged it.
    pub log_stream_name: String,
    /// The Region the stream's ARN is stamped with: the directory's own primary Region.
    pub region: String,
    /// The record itself, as [`rendered`] writes it.
    pub message: String,
    /// When the audited act happened — which is the event's timestamp, not its ingestion
    /// time.
    pub at: SystemTime,
}

impl super::Directory {
    /// The record this directory's domain controllers produce for `act` on `target`, when
    /// the directory's Security log is being forwarded; `None` when nothing forwards it.
    ///
    /// `None` is the whole of "no event ever reaches the group": a directory with no log
    /// subscription, and a directory whose controllers are not in service yet, forward
    /// nothing.
    pub fn forwarded_security_event(
        &self,
        act: AuditedAct,
        target: &str,
        member: Option<&str>,
        at: SystemTime,
    ) -> Option<ForwardedEvent> {
        let subscription = self.log_subscription.as_ref()?;
        let controller = self.serving_domain_controller()?;
        Some(ForwardedEvent {
            log_group_name: subscription.log_group_name.clone(),
            log_stream_name: controller.to_string(),
            region: self.primary_region.clone(),
            message: rendered(
                act,
                &self.directory_id,
                controller,
                &self.name,
                target,
                member,
            ),
            at,
        })
    }

    /// The `DomainControllerId` of the controller whose Security log a forwarded record is
    /// taken from: the first one placed that is in service ([`SERVING_STATUS`]).
    ///
    /// The first by placement order, so one directory's records stay in one stream instead
    /// of moving between controllers call by call.
    fn serving_domain_controller(&self) -> Option<&str> {
        self.domain_controllers_in_placement_order()
            .into_iter()
            .find(|controller| controller.status == SERVING_STATUS)
            .map(|controller| controller.domain_controller_id.as_str())
    }
}

/// One forwarded record's message: the Security-log entry as one JSON object.
///
/// The members are the ones the entry is ABOUT — the channel and event id Windows records
/// it under, the sentence it states, the directory and the domain controller it came from,
/// the account that was acted on under the domain it lives in, and, for an act about a
/// group MEMBERSHIP, the member the group gained or lost beside the group itself. Nothing
/// is invented for a member this emulator holds no record behind: a controller's host name,
/// the subject that performed the act and the Windows record's own per-field layout are not
/// modelled here, so they are left out rather than filled in with something no reader could
/// rely on.
pub fn rendered(
    act: AuditedAct,
    directory_id: &str,
    domain_controller_id: &str,
    domain_name: &str,
    target_account_name: &str,
    member_name: Option<&str>,
) -> String {
    let mut record = ::serde_json::Map::new();
    record.insert(
        "Channel".to_string(),
        ::serde_json::json!(SECURITY_CHANNEL),
    );
    record.insert("EventID".to_string(), ::serde_json::json!(act.event_id));
    record.insert(
        "Description".to_string(),
        ::serde_json::json!(act.description),
    );
    record.insert("DirectoryId".to_string(), ::serde_json::json!(directory_id));
    record.insert(
        "DomainControllerId".to_string(),
        ::serde_json::json!(domain_controller_id),
    );
    record.insert(
        "TargetDomainName".to_string(),
        ::serde_json::json!(domain_name),
    );
    record.insert(
        "TargetAccountName".to_string(),
        ::serde_json::json!(target_account_name),
    );
    if let Some(member) = member_name {
        record.insert("MemberName".to_string(), ::serde_json::json!(member));
    }
    ::serde_json::Value::Object(record).to_string()
}

/// Audit `act` on the account `target` in the directory `directory_id`, and forward the
/// record to the log group that directory's log subscription names.
///
/// Does nothing at all when the directory forwards no logs, which is what makes an act
/// performed outside a subscription unobservable in the group. Never fails and never
/// refuses: the call that performed the act asked for none of this.
///
/// Call it with NO store of the calling service locked — it takes this service's store and
/// then CloudWatch Logs' (see this module's lock order).
pub fn record<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    act: AuditedAct,
    target: &str,
    at: SystemTime,
) {
    forward_produced(engine, directory_id, act, target, None, at);
}

/// Audit `act` on the MEMBERSHIP of `member` in the group `group` of the directory
/// `directory_id`, and forward the record the same way [`record`] does.
///
/// A membership record names both objects: the group is what changed, and the member is
/// what it gained or lost.
pub fn record_membership<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    act: AuditedAct,
    group: &str,
    member: &str,
    at: SystemTime,
) {
    forward_produced(engine, directory_id, act, group, Some(member), at);
}

/// Produce the record and deliver it, if the directory forwards its security log at all.
fn forward_produced<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    act: AuditedAct,
    target: &str,
    member: Option<&str>,
    at: SystemTime,
) {
    let Some(event) = produced(engine, directory_id, act, target, member, at) else {
        return;
    };
    forward(engine, &event);
}

/// The record `act` produces in `directory_id`, read out of this service's own store.
///
/// The store's guard is released with the answer: the delivery that follows holds only the
/// destination's.
fn produced<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    act: AuditedAct,
    target: &str,
    member: Option<&str>,
    at: SystemTime,
) -> Option<ForwardedEvent> {
    let state = super::service_state(engine)?;
    let mut store = state.lock();
    // The controller statuses the clock already owes are materialised before they are
    // read, so a controller the clock has brought into service is in service whichever
    // call asks — exactly as every operation of this service reads them.
    store.advance_clock(at);
    store
        .directories
        .get(directory_id)?
        .forwarded_security_event(act, target, member, at)
}

/// Write one record into the CloudWatch Logs store of the SAME engine.
///
/// Best effort: a destination that is not there, or has no room, loses the record and is
/// traced ([`record`]).
fn forward<Eng: Send + Sync + 'static>(engine: &Eng, event: &ForwardedEvent) {
    let Some(state) = logs::service_state(engine) else {
        ::tracing::warn!(
            log_group = %event.log_group_name,
            "no CloudWatch Logs store is mounted, so a directory's forwarded security event \
             was not delivered"
        );
        return;
    };
    let mut store = state.lock();
    if !store.log_groups.contains_key(&event.log_group_name) {
        // The subscription names a group the ACCOUNT owns; nothing here creates one, so a
        // group that is not there is a destination nothing reaches.
        ::tracing::warn!(
            log_group = %event.log_group_name,
            "the subscribed log group is not held by CloudWatch Logs, so a directory's \
             forwarded security event was not delivered"
        );
        return;
    }
    let ingested_at = logs::now_ms();
    // The data identifiers the data protection policies in force for the group name: such a
    // policy masks what is ingested into the group, by whichever path the events arrive.
    let data_identifiers = store
        .log_groups
        .get(&event.log_group_name)
        .map(|group| store.data_protection_identifiers_for(group))
        .unwrap_or_default();
    let stored = logs::StoredEvent {
        timestamp: epoch_millis(event.at),
        masked_message: logs::data_protection::masked(&event.message, &data_identifiers),
        message: event.message.clone(),
        ingestion_time: ingested_at,
        ingestion_seq: store.take_ingestion_seq(),
    };
    let added = stored.stored_bytes();
    let Some(group) = store.log_groups.get_mut(&event.log_group_name) else {
        return;
    };
    // A stream is opened per domain controller, and it is admitted against the ceiling the
    // owning service's own `CreateLogStream` admits one against, so no write path can take
    // a group past it.
    if !group.streams.contains_key(&event.log_stream_name)
        && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
    {
        ::tracing::warn!(
            log_group = %event.log_group_name,
            log_stream = %event.log_stream_name,
            "the subscribed log group already holds the maximum number of streams, so a \
             directory's forwarded security event was not delivered"
        );
        return;
    }
    let stream = group
        .streams
        .entry(event.log_stream_name.clone())
        .or_insert_with(|| {
            logs::LogStreamRecord::new(
                &event.region,
                &event.log_group_name,
                &event.log_stream_name,
                ingested_at,
            )
        });
    stream.events.push(stored);
    // The stream is kept in the `(timestamp, ingestion_seq)` order its reader pages it in;
    // records arrive in that order already, so only one predating what is there moves.
    stream.events.sort_by_key(logs::StoredEvent::key);
    let discarded = stream.enforce_storage_ceiling();
    stream.refresh_event_times();
    stream.last_ingestion_time = Some(ingested_at);
    group.stored_bytes = group
        .stored_bytes
        .saturating_add(added)
        .saturating_sub(discarded)
        .max(0);
}

/// `at` in milliseconds since the Unix epoch, which is what every CloudWatch Logs timestamp
/// is expressed in. An instant before the epoch, which no clock reports, reads as the epoch.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}
