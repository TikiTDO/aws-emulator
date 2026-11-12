// SPDX-License-Identifier: CC0-1.0

//! The long-term memory EXTRACTION plane: the extraction jobs a memory container holds.
//!
//! An extraction job is a unit of long-term extraction WORK over the messages a memory
//! container accepted — not an extraction result. `ListMemoryExtractionJobs` reports the
//! units that are eligible to be started, and `StartMemoryExtractionJob` picks up the
//! messages one of them left unprocessed, so a unit is listed for as long as no
//! extraction has produced records from its messages.
//!
//! One unit is one SESSION of one actor: a session's conversational messages are what
//! long-term extraction reads together, and the modelled job names the actor and session
//! its messages come from beside the `(eventId, messageIndex)` pair of each one
//! ([`ExtractionJobUnit::to_metadata`]). Both ways a container accepts messages for
//! long-term extraction compose a session's unit: the events `CreateEvent` stored in it
//! and the content `IngestData` submitted into it, in the order the container accepted
//! them.
//!
//! What a job does NOT carry is an outcome. Nothing in this emulator has run extraction
//! over a stored message, so no job here reports a `status` or a `failureReason`: a unit
//! is reported with the identity and the messages its session supports, and with nothing
//! an attempt would have produced.
//!
//! The work this walk does is bounded by the store's own ceilings
//! ([`super::MAX_STORED_EVENTS`] events and [`super::MAX_STORED_INGESTIONS`] ingestions
//! across every session), so a listing cannot grow with request volume.

use super::{normalize_memory_id, ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState};
use crate::ops::types::{
    ExtractionJobMessages, ExtractionJobMetadata, IngestPayloadType, MessageMetadata, PayloadType,
};

/// `ExtractionMode`'s only value: an event created with it "is excluded from long-term
/// memory extraction" (source: smithy_query query="shape" shape="ExtractionMode", values
/// SKIP), so it composes no message of a container's extraction jobs.
pub const EXTRACTION_MODE_SKIP: &str = "SKIP";

/// The `jobId` of the unit composed of one actor's session, in the ONE spelling the two
/// extraction-job operations read it by.
///
/// `ExtractionJob$jobId` and `ExtractionJobMetadata$jobID` are plain strings with no
/// modelled form, and the unit a job stands for is identified by the actor and session
/// whose messages compose it — so the id is that pair, base64-encoded (the codec this
/// crate's pagination tokens already use) to keep it opaque to a caller. A `sessionId`
/// admits neither `:` nor any other separator character
/// (`^[a-zA-Z0-9][a-zA-Z0-9-_]*$`), so the pair is unambiguous in that order.
#[must_use]
pub fn extraction_job_id(actor_id: &str, session_id: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{session_id}:{actor_id}"))
}

/// One unit of long-term extraction work a memory container holds: the messages of one
/// session that no extraction has processed, with the identity a listing reports them
/// under.
#[derive(Debug, Clone)]
pub struct ExtractionJobUnit {
    /// The unit's `jobId`, as [`extraction_job_id`] spells it.
    pub job_id: String,
    pub actor_id: String,
    pub session_id: String,
    /// The messages that compose the unit, in the order the container accepted them.
    pub messages: Vec<MessageMetadata>,
    /// The sequence of the earliest event the unit's messages come from: the order the
    /// container accepted this unit of work, which is the order a listing reports units
    /// in.
    pub accepted_seq: u64,
}

impl ExtractionJobUnit {
    /// This unit as the modelled `ExtractionJobMetadata`.
    ///
    /// `status`, `failureReason` and `strategyId` are left out: the first two report an
    /// extraction attempt's outcome and nothing has attempted one, and a memory strategy
    /// is owned by the CONTROL plane, a different service, so this plane can name none
    /// for a unit it derived from stored messages.
    #[must_use]
    pub fn to_metadata(&self) -> ExtractionJobMetadata {
        ExtractionJobMetadata {
            job_id: self.job_id.clone(),
            messages: ExtractionJobMessages::messagesList(self.messages.clone()),
            status: None,
            failure_reason: None,
            strategy_id: None,
            session_id: Some(self.session_id.clone()),
            actor_id: Some(self.actor_id.clone()),
        }
    }
}

impl super::StoredEvent {
    /// Whether this event's messages are read by long-term extraction: an event created
    /// with `extractionMode` `SKIP` is excluded from it, and one created without the
    /// member "is processed for extraction as usual".
    #[must_use]
    pub fn read_by_extraction(&self) -> bool {
        self.extraction_mode.as_deref() != Some(EXTRACTION_MODE_SKIP)
    }

    /// The messages of this event, as the metadata an extraction job names each one by.
    ///
    /// `PayloadType` carries a conversational turn, a JSON document or a blob, and a
    /// MESSAGE is the conversational turn — `MessageMetadata$messageIndex` is "the
    /// position of this message within that event's ordered list of messages", so the
    /// index counts this event's conversational items in payload order and an event
    /// carrying none composes no message.
    #[must_use]
    pub fn extraction_messages(&self) -> Vec<MessageMetadata> {
        self.payload
            .iter()
            .filter(|item| matches!(item, PayloadType::Conversational(_)))
            .enumerate()
            .map(|(index, _)| MessageMetadata {
                event_id: self.event_id.clone(),
                message_index: i32::try_from(index).unwrap_or(i32::MAX),
            })
            .collect()
    }
}

impl super::IngestedContent {
    /// The messages of this ingested content, as the metadata an extraction job names
    /// each one by.
    ///
    /// `IngestPayloadType` carries a conversational turn or a JSON document, and a
    /// message is the conversational turn, indexed exactly as an event's messages are
    /// ([`super::StoredEvent::extraction_messages`]). The content is named by the id this
    /// service minted for the record it stored it in: `MessageMetadata$eventId` is the
    /// only identity a job can name a message by, and ingested content is stored as its
    /// own record rather than as one of the session's short-term events.
    #[must_use]
    pub fn extraction_messages(&self) -> Vec<MessageMetadata> {
        self.payload
            .iter()
            .filter(|item| matches!(item, IngestPayloadType::Conversational(_)))
            .enumerate()
            .map(|(index, _)| MessageMetadata {
                event_id: self.content_id.clone(),
                message_index: i32::try_from(index).unwrap_or(i32::MAX),
            })
            .collect()
    }
}

impl ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState {
    /// The units of long-term extraction work the memory container `memory_id` names
    /// holds, oldest unit first.
    ///
    /// A session composes one unit, from the messages of the events extraction reads
    /// ([`super::StoredEvent::read_by_extraction`]) and of the content ingested into it,
    /// ordered by the write that stored each one; a session whose records compose no
    /// message at all is no unit of work and is left out. The order of the units is the
    /// one the container accepted them in, with the `jobId` breaking a tie, so the pages
    /// of one listing cannot disagree about where a unit sits.
    ///
    /// A container this plane has nothing stored for holds no unit — which is not a
    /// statement about whether the CONTAINER exists, resolved by
    /// [`super::memory_container_resolved`] before this is read.
    #[must_use]
    pub fn extraction_job_units(&self, memory_id: &str) -> Vec<ExtractionJobUnit> {
        let Some(memory) = self.memories.get(&normalize_memory_id(memory_id)) else {
            return Vec::new();
        };
        let mut units: Vec<ExtractionJobUnit> = Vec::new();
        for (actor_id, actor) in &memory.actors {
            for (session_id, session) in &actor.sessions {
                // The records of this session that carry messages for extraction, in the
                // order the container accepted them: the engine-wide sequence totally
                // orders the two kinds of write against each other.
                let mut accepted: Vec<(u64, Vec<MessageMetadata>)> = session
                    .events
                    .iter()
                    .filter(|event| event.read_by_extraction())
                    .map(|event| (event.seq, event.extraction_messages()))
                    .chain(
                        session
                            .ingestions
                            .iter()
                            .map(|ingested| (ingested.seq, ingested.extraction_messages())),
                    )
                    .filter(|(_, messages)| !messages.is_empty())
                    .collect();
                accepted.sort_by_key(|(seq, _)| *seq);
                let Some(accepted_seq) = accepted.first().map(|(seq, _)| *seq) else {
                    continue;
                };
                units.push(ExtractionJobUnit {
                    job_id: extraction_job_id(actor_id, session_id),
                    actor_id: actor_id.clone(),
                    session_id: session_id.clone(),
                    messages: accepted
                        .into_iter()
                        .flat_map(|(_, messages)| messages)
                        .collect(),
                    accepted_seq,
                });
            }
        }
        units.sort_by(|left, right| {
            left.accepted_seq
                .cmp(&right.accepted_seq)
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        units
    }
}
