// SPDX-License-Identifier: CC0-1.0

//! What a speech synthesis task's COMPLETION does outside this service: it writes the
//! rendered output into the bucket the start call named, at exactly the key the task's own
//! `OutputUri` reports, and — when that call named a topic — publishes one notification
//! about the terminal status it reached.
//!
//! That is the whole point of the asynchronous API: a client polls to a terminal status and
//! then reads the object its `OutputUri` names, or waits on the notification instead of
//! polling at all. So the completion is NOT the status field alone. A task carries the
//! delivery its completion owes ([`super::TaskOutputDelivery`]) and the status moves with
//! that delivery: the object exists before any read reports the task terminal, and a task
//! whose output could not be written reaches the terminal status its failure deserves rather
//! than reporting a success whose object is not there.
//!
//! Two things drive the delivery, through the one claim below, so it is made EXACTLY ONCE:
//!
//! * the start call arms a timer for the instant the completion falls due
//!   ([`deliver_when_due`]), because a client that waits on the notification never calls
//!   this service again and the delivery cannot wait for a read that never comes;
//! * every task read makes the delivery the clock has ALREADY made due
//!   ([`deliver_due_completions`]), which is the same materialisation a read already makes
//!   for the status — owed at that instant whether or not anybody polled — and is what still
//!   delivers when the timer did not run, or when the read that claimed a delivery was
//!   dropped before it reported back ([`DELIVERY_RECLAIM_AFTER`]).
//!
//! Nothing here reaches off this process. Both hops are the in-process callback into this
//! same emulator ([`::service_common::s3_client`], [`::service_common::sns_client`]), each
//! addressed by the REGION the start call was signed for and by nothing else: the bucket, the
//! key and the topic a request carries select resources of this emulator, and are never an
//! address.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use smithy_server_runtime::EmbeddedEmulator;

use super::{
    service_state, settle_task_status, DeliveryState, StoredSynthesisTask,
    TASK_COMPLETION_DELAY,
};
use crate::ops::synthesize_speech::{
    content_type_for, default_sample_rate, lexicon::Lexicons, render::Timbre,
    resolve_sample_rate, Synthesis, SynthesisFailure, SYNTHESIS_TIME_BUDGET,
};

/// How many task deliveries this service may have in flight at once.
///
/// CHOSEN, and it is a bound on WORK rather than on tasks: every started task arms a timer,
/// so without it a burst of start calls would leave an unbounded number of renders
/// outstanding. A start call that finds no slot free arms nothing and leaves the delivery to
/// the first read that finds it due, which is the same delivery through the same claim.
pub const MAX_DELIVERIES_IN_FLIGHT: usize = 32;

/// The most deliveries ONE read makes before it answers.
///
/// CHOSEN. A read materialises what the clock owes, and the listing can find many tasks due
/// at once; this bounds how many one response pays for. What is left over is delivered by
/// the next read or by its own timer — nothing is dropped, because the claim leaves an
/// undelivered task exactly as it found it.
const MAX_DELIVERIES_PER_READ: usize = 4;

/// How long into one read a further delivery may still be STARTED.
///
/// CHOSEN, and the bound that matters for a read's latency: with it, the wait one response
/// can add is this budget plus the one synthesis it was inside the budget to start — not
/// [`MAX_DELIVERIES_PER_READ`] whole synthesis budgets.
const READ_DELIVERY_BUDGET: Duration = Duration::from_secs(2);

/// How long a CLAIMED delivery may be outstanding before another caller may claim it again.
///
/// CHOSEN, and it is a liveness bound rather than a timeout on the work: a claim is made
/// under the state lock and reported back by the claimant, so a claimant that never reports
/// back — the read it was made on was dropped — would otherwise leave the task claimed and
/// never terminal. It is past the longest a delivery can legitimately take (one synthesis
/// budget plus the hand-over of the object), and it is measured on the MONOTONIC clock the
/// claim is stamped with, so a reclaim cannot race a claimant that is still working — not
/// even across a wall-clock jump.
const DELIVERY_RECLAIM_AFTER: Duration = Duration::from_secs(SYNTHESIS_TIME_BUDGET.as_secs() + 10);

/// Added to the wait a timer makes, so the completion is due — not a microsecond short of
/// due — when the claim runs.
const DUE_SLACK: Duration = Duration::from_millis(10);

/// One claimed delivery: what the output object is produced from, and where it goes.
struct ClaimedDelivery {
    /// The task this delivery belongs to.
    task_id: String,
    /// The Region the bucket and the topic are addressed in.
    region: String,
    /// The bucket the object is written into.
    bucket: String,
    /// The key the object is written at — the key `OutputUri` reports.
    key: String,
    /// The media type of the task's `OutputFormat`.
    content_type: &'static str,
    /// The synthesis that produces the object's body, built exactly as the synchronous path
    /// builds it, so one request's output is one utterance whichever path renders it.
    synthesis: Synthesis,
    /// The topic the terminal status is published to, when the start call named one.
    sns_topic_arn: Option<String>,
}

/// Why a delivery produced no object, as the task's own `TaskStatusReason` then states it.
///
/// The text is this service's own: Amazon Polly reports a reason for a failed task and
/// documents `SynthesisTask$TaskStatusReason` only as "Reason for the current status of a
/// specific speech synthesis task", so what is stated here is the failure in the service's
/// own terms, with the bucket taken from the request the task recorded.
enum DeliveryFailure {
    /// The output could not be written into the bucket the start call named.
    Write(String),
    /// The text could not be synthesized at all.
    Synthesis,
}

impl DeliveryFailure {
    /// The `TaskStatusReason` a task carries for this failure.
    fn reason(&self) -> String {
        match self {
            Self::Write(bucket) => format!(
                "Unable to write the output of this synthesis task to the Amazon S3 bucket \
                 {bucket}"
            ),
            Self::Synthesis => {
                "Unable to synthesize the text of this synthesis task".to_string()
            }
        }
    }
}

/// The `TaskStatus` a task that could not deliver its output reaches.
///
/// `TaskStatus` is one of `scheduled | inProgress | completed | failed`
/// (`smithy_query(query="shape", shape="TaskStatus")`), and a task with no output is not a
/// task that completed.
const FAILED_TASK_STATUS: &str = "failed";

/// Arm the delivery of the task `task_id` names for the instant its completion falls due.
///
/// Returns at once: the delivery runs in a task of its own, because the start call has to
/// answer while the synthesis task is still in its non-terminal status. It exists for the
/// client that never polls — the one waiting on the notification — and for the object a
/// consumer reads straight off `OutputUri`.
///
/// It materialises exactly what the clock has made due, through the same claim a read uses,
/// so what it writes is what a read of the same instant would have written, and a delivery
/// another caller already claimed is left alone.
pub fn deliver_when_due<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    task_id: String,
    due_at: SystemTime,
) {
    let Some(state) = service_state(&*engine) else {
        return;
    };
    if !state.lock().begin_delivery() {
        ::tracing::warn!(
            task = %task_id,
            "{MAX_DELIVERIES_IN_FLIGHT} synthesis task deliveries are already in flight; this \
             task's output is written by the next read that finds its completion due"
        );
        return;
    }
    ::tokio::spawn(async move {
        let wait = due_at
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .saturating_add(DUE_SLACK)
            // A due instant a clock jump put far in the future cannot make this task wait
            // longer than the span the completion was promised in.
            .min(TASK_COMPLETION_DELAY.saturating_add(DUE_SLACK));
        ::tokio::time::sleep(wait).await;
        // The guard is released before anything is delivered, so this service's store is
        // never held across a call into another service's.
        let claimed = {
            service_state(&*engine).and_then(|state| {
                let mut guard = state.lock();
                claim_one(&mut guard, &task_id, SystemTime::now())
            })
        };
        if let Some(claim) = claimed {
            deliver(&*engine, &callback, claim).await;
        }
        // The slot is held for the whole wait AND the delivery, and given back however
        // either ends.
        if let Some(state) = service_state(&*engine) {
            state.lock().finish_delivery();
        }
    });
}

/// Make the deliveries the CLOCK has already made due, at most
/// [`MAX_DELIVERIES_PER_READ`] of them and starting no further one past
/// [`READ_DELIVERY_BUDGET`].
///
/// Every task read calls this before it reports: the delivery was owed at this instant
/// whether or not anybody polled, so making it changes nothing a caller could not have
/// observed by waiting — and it is what makes the object present before the read that
/// reports the task terminal. Each delivery is claimed one at a time, so a read that has
/// spent its budget leaves the rest exactly as it found them, for their own timers or for
/// the next read.
///
/// `asked_about` is the task the read is ABOUT, when it is about one: its delivery is made
/// first, so a read of one task reports that task's own completion rather than spending its
/// budget on another's and answering non-terminal.
pub async fn deliver_due_completions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    now: SystemTime,
    asked_about: Option<&str>,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // The tasks this read may deliver, named under the lock and bounded before anything is
    // produced: what is left over is for the next read or for the tasks' own timers.
    let due: Vec<String> = {
        let guard = state.lock();
        let mut due: Vec<String> = guard
            .synthesis_tasks
            .iter()
            .filter(|(_, task)| delivery_is_due(task, now))
            .map(|(task_id, _)| task_id.clone())
            .collect();
        // The read's own task first, then the rest in the store's order.
        due.sort_by_key(|task_id| asked_about != Some(task_id.as_str()));
        due.truncate(MAX_DELIVERIES_PER_READ);
        due
    };
    let started = Instant::now();
    for task_id in &due {
        if started.elapsed() >= READ_DELIVERY_BUDGET {
            break;
        }
        // The guard is released before anything is delivered, so this service's store is
        // never held across a call into another service's. A task that cannot be claimed
        // here — another caller got to it first — costs this read nothing: the next one in
        // the list is tried instead.
        let claimed = {
            let mut guard = state.lock();
            claim_one(&mut guard, task_id, now)
        };
        let Some(claim) = claimed else {
            continue;
        };
        deliver(engine, callback, claim).await;
    }
}

/// Whether `task` still owes the delivery its completion makes, and the clock has made that
/// completion due.
///
/// A delivery another caller claimed is not due — until that claim is stale
/// ([`DELIVERY_RECLAIM_AFTER`]), which is how a claimant that never reported back (the read
/// it was made on was dropped, or the process it ran in went away) leaves the task
/// deliverable instead of wedged.
fn delivery_is_due(task: &StoredSynthesisTask, now: SystemTime) -> bool {
    let claimable = task
        .output_delivery
        .as_ref()
        .is_some_and(|delivery| match delivery.state {
            DeliveryState::Owed => true,
            DeliveryState::InFlight(claimed_at) => claimed_at.elapsed() >= DELIVERY_RECLAIM_AFTER,
            DeliveryState::Done => false,
        });
    claimable
        && task
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at)
}

/// Claim the delivery the task `task_id` names, or answer `None` when it owes none, when its
/// completion is not due yet, or when another caller holds a claim on it that is not stale.
///
/// The claim is the write that makes the delivery exactly once: it moves the task's delivery
/// to [`DeliveryState::InFlight`], stamped with this instant, under the state lock — so only
/// one caller is ever handed the work, and until it reports back the task keeps the
/// non-terminal status it was in.
fn claim_one(
    store: &mut super::ComAmazonawsPollyParrotV1State,
    task_id: &str,
    now: SystemTime,
) -> Option<ClaimedDelivery> {
    // What producing the output needs from the REST of the store — the lexemes of the
    // lexicons the task named, in the Region it was started in, and the timbre of the voice
    // it named — read before the task itself is borrowed mutably.
    let (region, lexicon_names, voice_id) = {
        let task = store.synthesis_tasks.get(task_id)?;
        if !delivery_is_due(task, now) {
            return None;
        }
        let delivery = task.output_delivery.as_ref()?;
        (
            delivery.region.clone(),
            task.lexicon_names.clone(),
            task.voice_id.clone(),
        )
    };
    let mut lexicons = Lexicons::default();
    for name in &lexicon_names {
        if let Some(stored) = store.lexicon(&region, name) {
            lexicons.add(&stored.content);
        }
    }
    let timbre = Timbre::for_gender(store.voice(&voice_id).map(|voice| voice.gender));

    let task = store.synthesis_tasks.get_mut(task_id)?;
    let content_type = content_type_for(&task.output_format)?;
    // `json` output IS the speech-mark stream and carries no audio, so it is rendered at no
    // frequency; every audio format is rendered at the rate the request named, or at the
    // format's and engine's own default when it named none — the same resolution the
    // synchronous path makes, so the two cannot disagree about one request.
    let sample_rate = (task.output_format != "json").then(|| {
        task.sample_rate
            .as_deref()
            .and_then(|requested| resolve_sample_rate(&task.output_format, requested))
            .unwrap_or_else(|| default_sample_rate(&task.output_format, &task.engine))
    });
    let text_type = task.text_type.clone();
    let output_format = task.output_format.clone();
    let marks = task.speech_mark_types.clone();
    let sns_topic_arn = task.sns_topic_arn.clone();
    let delivery = task.output_delivery.as_mut()?;
    delivery.state = DeliveryState::InFlight(Instant::now());
    Some(ClaimedDelivery {
        task_id: task_id.to_string(),
        region,
        bucket: delivery.bucket.clone(),
        key: delivery.key.clone(),
        content_type,
        synthesis: Synthesis {
            text: delivery.text.clone(),
            text_type,
            output_format,
            sample_rate,
            marks,
            lexicons,
            timbre,
        },
        sns_topic_arn,
    })
}

/// Make one claimed delivery: write the output object, apply the terminal status it settles,
/// and publish the notification about that status when the task asked for one.
async fn deliver<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    claim: ClaimedDelivery,
) {
    let ClaimedDelivery {
        task_id,
        region,
        bucket,
        key,
        content_type,
        synthesis,
        sns_topic_arn,
    } = claim;
    let failure = match produce(synthesis).await {
        None => Some(DeliveryFailure::Synthesis),
        Some(body) => {
            let written = write_output(callback, &region, &bucket, &key, content_type, body).await;
            (!written).then(|| DeliveryFailure::Write(bucket.clone()))
        }
    };
    let Some(published) = settle(engine, &task_id, failure.as_ref()) else {
        return;
    };
    if let Some(topic_arn) = sns_topic_arn.as_deref() {
        publish(callback, &region, topic_arn, &published).await;
    }
}

/// Produce the object's body: the speech-mark stream for `json`, and the rendered waveform in
/// its own encoding for every audio format.
///
/// The render is synchronous, CPU-bound work with no await of its own, so it goes to the
/// BLOCKING pool — the runtime stays responsive to a health check, a cancellation and a
/// shutdown while it runs — and the wait for it is bounded by the same budget the synchronous
/// path allows one synthesis.
async fn produce(synthesis: Synthesis) -> Option<Vec<u8>> {
    match ::tokio::time::timeout(
        SYNTHESIS_TIME_BUDGET,
        ::tokio::task::spawn_blocking(move || synthesis.produce()),
    )
    .await
    {
        Ok(Ok(Ok(body))) => Some(body),
        Ok(Ok(Err(failure))) => {
            let detail = match failure {
                SynthesisFailure::InvalidSsml => "the text is not a readable SSML document",
                SynthesisFailure::Encoding => "the encoder refused the stream",
                SynthesisFailure::Format(_) => "the output format is not one this service encodes",
            };
            ::tracing::warn!(detail, "a synthesis task's output could not be produced");
            None
        }
        Ok(Err(_)) | Err(_) => {
            ::tracing::warn!(
                "a synthesis task's output did not finish inside the synthesis time budget"
            );
            None
        }
    }
}

/// Write the task's output object into the bucket the start call named, at the key the task's
/// `OutputUri` reports, carrying the media type of its `OutputFormat`.
///
/// `false` when the object store did not take it — the bucket does not exist, or it refused
/// the write — which is a task with no output rather than a completed one.
async fn write_output(
    callback: &EmbeddedEmulator,
    region: &str,
    bucket: &str,
    key: &str,
    content_type: &str,
    body: Vec<u8>,
) -> bool {
    let written = ::service_common::s3_client::s3_client(callback, region)
        .put_object()
        .bucket(bucket)
        .key(key)
        .content_type(content_type)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    match written {
        Ok(_) => true,
        Err(error) => {
            ::tracing::warn!(
                bucket = %bucket,
                key = %key,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a synthesis task's output could not be written to its destination bucket"
            );
            false
        }
    }
}

/// What the notification one delivered task publishes carries.
struct PublishedTask {
    /// The task's own identifier.
    task_id: String,
    /// The terminal `TaskStatus` it reached.
    task_status: String,
    /// The reason it carries, for a task that failed.
    task_status_reason: Option<String>,
    /// Where its output is (`OutputUri`).
    output_uri: String,
    /// When it was started.
    creation_time: SystemTime,
    /// How many billable characters it synthesized.
    request_characters: i32,
    /// The format its output is in.
    output_format: String,
    /// The topic this notification goes to, as the start call named it.
    sns_topic_arn: String,
}

/// Apply the terminal status this delivery settles, and answer the task as the notification
/// then reports it — or `None` when the service no longer holds the task.
///
/// A delivery that wrote the object applies the transition the clock already made due, which
/// is the one the start call recorded; a delivery that produced no object makes the task
/// `failed`, carrying the reason for it, because a task whose output is not there did not
/// complete. Either way the delivery is marked made, so nothing is delivered twice.
fn settle<Eng: Send + Sync + 'static>(
    engine: &Eng,
    task_id: &str,
    failure: Option<&DeliveryFailure>,
) -> Option<PublishedTask> {
    let state = service_state(engine)?;
    let mut guard = state.lock();
    let task = guard.synthesis_tasks.get_mut(task_id)?;
    if let Some(delivery) = task.output_delivery.as_mut() {
        delivery.state = DeliveryState::Done;
    }
    match failure {
        None => settle_task_status(task, SystemTime::now()),
        Some(failure) => {
            task.pending_status = None;
            task.task_status = FAILED_TASK_STATUS.to_string();
            task.task_status_reason = Some(failure.reason());
        }
    }
    Some(PublishedTask {
        task_id: task.task_id.clone(),
        task_status: task.task_status.clone(),
        task_status_reason: task.task_status_reason.clone(),
        output_uri: task.output_uri.clone(),
        creation_time: task.creation_time,
        request_characters: task.request_characters,
        output_format: task.output_format.clone(),
        sns_topic_arn: task.sns_topic_arn.clone()?,
    })
}

/// The body one notification carries: the task's own record — which task it is about, the
/// terminal status it reached, where its output is, when it was started, what it billed and
/// the topic it was asked to be announced on.
///
/// Built from the task as the store holds it, so a subscriber reads the outcome without
/// calling `GetSpeechSynthesisTask`.
fn notification_body(task: &PublishedTask) -> String {
    let mut document = ::serde_json::Map::new();
    document.insert(
        "taskId".to_string(),
        ::serde_json::Value::from(task.task_id.clone()),
    );
    document.insert(
        "taskStatus".to_string(),
        ::serde_json::Value::from(task.task_status.clone()),
    );
    if let Some(reason) = task.task_status_reason.as_deref() {
        document.insert(
            "taskStatusReason".to_string(),
            ::serde_json::Value::from(reason),
        );
    }
    document.insert(
        "outputUri".to_string(),
        ::serde_json::Value::from(task.output_uri.clone()),
    );
    if let Ok(created) = ::aws_smithy_types::DateTime::from(task.creation_time)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
    {
        document.insert("creationTime".to_string(), ::serde_json::Value::from(created));
    }
    document.insert(
        "requestCharacters".to_string(),
        ::serde_json::Value::from(task.request_characters),
    );
    document.insert(
        "outputFormat".to_string(),
        ::serde_json::Value::from(task.output_format.clone()),
    );
    document.insert(
        "snsTopicArn".to_string(),
        ::serde_json::Value::from(task.sns_topic_arn.clone()),
    );
    ::serde_json::Value::Object(document).to_string()
}

/// Publish one notification about a task that reached a terminal status.
///
/// A delivery failure is LOGGED, not raised: the notification is made after the call that
/// started the task has long answered, and the task has already reached its terminal status,
/// so a topic that was deleted or that refuses the publish cannot change the task's outcome.
/// Nothing is requeued — the notification is at-most-once.
async fn publish(
    callback: &EmbeddedEmulator,
    region: &str,
    topic_arn: &str,
    task: &PublishedTask,
) {
    let published = ::service_common::sns_client::sns_client(callback, region)
        .publish()
        .topic_arn(topic_arn)
        .message(notification_body(task))
        .send()
        .await;
    if let Err(error) = published {
        ::tracing::warn!(
            topic = %topic_arn,
            task = %task.task_id,
            detail = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "a synthesis task's status notification could not be published to its topic"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A delivery is claimable exactly once at a time — and a claim that never reported
    /// back does not wedge the task: it is claimable again once it cannot still be running.
    #[test]
    fn a_claim_is_exclusive_until_it_goes_stale() {
        let now = SystemTime::now();
        let due_at = now - Duration::from_secs(1);
        let task = |state: DeliveryState| StoredSynthesisTask {
            task_id: "task".to_string(),
            engine: "standard".to_string(),
            output_format: "mp3".to_string(),
            output_uri: "https://s3.us-east-1.amazonaws.com/bucket/task.mp3".to_string(),
            creation_time: now,
            request_characters: 5,
            sns_topic_arn: None,
            lexicon_names: Vec::new(),
            sample_rate: None,
            speech_mark_types: Vec::new(),
            text_type: "text".to_string(),
            voice_id: "Joanna".to_string(),
            language_code: None,
            task_status: "scheduled".to_string(),
            task_status_reason: None,
            pending_status: Some(crate::state::PendingTaskStatus {
                due_at,
                status: "completed".to_string(),
                reason: None,
            }),
            output_delivery: Some(crate::state::TaskOutputDelivery {
                region: "us-east-1".to_string(),
                bucket: "bucket".to_string(),
                key: "task.mp3".to_string(),
                text: "hello".to_string(),
                state,
            }),
        };
        // Every delivery state, and a claim on each side of the instant it goes stale.
        let claimed_ago = |ago: Duration| {
            DeliveryState::InFlight(
                Instant::now()
                    .checked_sub(ago)
                    .expect("the monotonic clock is past this test's own spans"),
            )
        };
        for (case, state, expected) in [
            ("owed", DeliveryState::Owed, true),
            ("claimed just now", claimed_ago(Duration::ZERO), false),
            (
                "claimed a moment before it goes stale",
                claimed_ago(DELIVERY_RECLAIM_AFTER - Duration::from_secs(1)),
                false,
            ),
            (
                "a claim exactly as stale as the bound",
                claimed_ago(DELIVERY_RECLAIM_AFTER),
                true,
            ),
            (
                "a claim long past the bound",
                claimed_ago(DELIVERY_RECLAIM_AFTER * 10),
                true,
            ),
            ("already delivered", DeliveryState::Done, false),
        ] {
            let subject = task(state);
            assert_eq!(
                delivery_is_due(&subject, now),
                expected,
                "case {case:?}: claimable is {}, expected {expected}",
                delivery_is_due(&subject, now)
            );
        }

        // And a completion the clock has not made due yet is not deliverable at all,
        // whatever its delivery state says.
        let mut early = task(DeliveryState::Owed);
        early.pending_status = Some(crate::state::PendingTaskStatus {
            due_at: now + Duration::from_secs(60),
            status: "completed".to_string(),
            reason: None,
        });
        assert!(
            !delivery_is_due(&early, now),
            "a completion that is not due owes no delivery yet"
        );
    }

    /// A failed delivery states which bucket the output could not be written to, taken from
    /// the task's own record rather than from any one request's text.
    #[test]
    fn a_failed_delivery_states_the_bucket_it_could_not_write_to() {
        for bucket in ["polly-output", "another.bucket-7"] {
            let reason = DeliveryFailure::Write(bucket.to_string()).reason();
            assert!(
                reason.contains(bucket),
                "case {bucket:?}: the reason must name the bucket, got {reason:?}"
            );
        }
        let reason = DeliveryFailure::Synthesis.reason();
        assert!(
            !reason.is_empty(),
            "a synthesis failure must state a reason, got {reason:?}"
        );
    }

    /// The notification carries the task's identifier and the terminal status it reached,
    /// whichever status that is — which is what an event-driven consumer reads instead of
    /// polling.
    #[test]
    fn the_notification_carries_the_task_and_its_terminal_status() {
        for (case, status, reason) in [
            ("a completed task", "completed", None),
            ("a failed task", FAILED_TASK_STATUS, Some("no output".to_string())),
        ] {
            let task = PublishedTask {
                task_id: "11111111-2222-3333-4444-555555555555".to_string(),
                task_status: status.to_string(),
                task_status_reason: reason.clone(),
                output_uri: "https://s3.us-east-1.amazonaws.com/bucket/key.mp3".to_string(),
                creation_time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600),
                request_characters: 11,
                output_format: "mp3".to_string(),
                sns_topic_arn: "arn:aws:sns:us-east-1:000000000000:tasks".to_string(),
            };
            let body: ::serde_json::Value = ::serde_json::from_str(&notification_body(&task))
                .unwrap_or_else(|error| panic!("case {case:?}: not a JSON document: {error}"));
            assert_eq!(
                body["taskId"].as_str(),
                Some(task.task_id.as_str()),
                "case {case:?}: the notification must name the task, got {body}"
            );
            assert_eq!(
                body["taskStatus"].as_str(),
                Some(status),
                "case {case:?}: the notification must carry the terminal status, got {body}"
            );
            assert_eq!(
                body["taskStatusReason"].as_str(),
                reason.as_deref(),
                "case {case:?}: the reason must travel with the status, got {body}"
            );
            assert_eq!(
                body["snsTopicArn"].as_str(),
                Some(task.sns_topic_arn.as_str()),
                "case {case:?}: the notification must name the topic it was asked for, got {body}"
            );
        }
    }
}
