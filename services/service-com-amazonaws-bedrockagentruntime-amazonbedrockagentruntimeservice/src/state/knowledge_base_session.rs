// SPDX-License-Identifier: CC0-1.0

//! The conversational SESSION a `RetrieveAndGenerate` call is served in: the turns one
//! caller has taken against a knowledge base, and how the `sessionId` that addresses them
//! is minted and resolved.
//!
//! It is a resource of THIS plane, and not the session `CreateSession` creates
//! ([`crate::state::Session`], which holds invocations and their steps): a caller never
//! creates one of these and never names one it was not handed — "When you first make a
//! RetrieveAndGenerate request, Amazon Bedrock automatically generates this value. You must
//! reuse this value for all subsequent requests in the same conversational session. … You
//! can't explicitly set the sessionId yourself"
//! (`smithy_query(query="operation",operation="RetrieveAndGenerate")` -> `sessionId`). So:
//!
//! * a request carrying no `sessionId` OPENS a session, which is the one write that mints
//!   one, and the id it is handed back under is the id of a session this store really
//!   holds;
//! * a request carrying one CONTINUES that session, appending its turn to the turns already
//!   recorded under it, so the id a caller reuses addresses the conversation it was handed;
//! * a request carrying an id this store does not hold names no session this service issued
//!   and is refused ([`session_not_valid_message`]) rather than quietly opening a second
//!   one under the caller's own id.

use ::aws_smithy_types::DateTime;

/// Resource guard, NOT an AWS quota: the most conversational sessions this store holds at
/// once. Sessions live in this process's memory and no operation deletes one, so without a
/// ceiling a stream of session-opening calls would grow the store until the emulator is
/// starved; past it the call answers its modelled `ServiceQuotaExceededException` (HTTP
/// 400) instead of allocating. The same guard the rest of this service's store declares
/// ([`crate::state::MAX_SESSIONS`]), and far above any plausible test or demo workload.
pub const MAX_KNOWLEDGE_BASE_SESSIONS: usize = 1_000;

/// Resource guard: the most turns one conversational session records. Each turn holds the
/// query and the answer it was served, so the ceiling bounds one session's memory the way
/// [`MAX_KNOWLEDGE_BASE_SESSIONS`] bounds the store's. Also an emulator limit rather than a
/// published quota.
pub const MAX_TURNS_PER_KNOWLEDGE_BASE_SESSION: usize = 100;

/// One turn of a conversation: the query the caller made, what it was made against, and the
/// answer this service served for it.
#[derive(Debug, Clone)]
pub struct KnowledgeBaseTurn {
    /// The query text the turn was made with.
    pub query: String,
    /// The knowledge base the query was made to.
    pub knowledge_base_id: String,
    /// The model the response was generated with, as the catalogue spells its id.
    pub model_id: String,
    /// The response this service served for the turn.
    pub text: String,
    /// How many segments of that response were cited to a retrieved passage.
    pub citations: usize,
    /// When the turn was served.
    pub at: DateTime,
}

/// One conversational session with a knowledge base.
#[derive(Debug, Clone)]
pub struct KnowledgeBaseSession {
    /// The `sessionId` this service minted for it.
    pub session_id: String,
    pub created_at: DateTime,
    /// When the last turn of the session was served, so the record says what the session
    /// has done rather than only that it exists.
    pub last_used_at: DateTime,
    /// The KMS key the opening request's `sessionConfiguration` named, when it named one.
    pub encryption_key_arn: Option<String>,
    /// The turns taken in the session, oldest first.
    pub turns: Vec<KnowledgeBaseTurn>,
}

impl KnowledgeBaseSession {
    /// A session opened at `at`, under the key its opening request named.
    pub fn opened(session_id: String, encryption_key_arn: Option<String>, at: DateTime) -> Self {
        Self {
            session_id,
            created_at: at,
            last_used_at: at,
            encryption_key_arn,
            turns: Vec::new(),
        }
    }

    /// Record one served turn on the session.
    pub fn record(&mut self, turn: KnowledgeBaseTurn) {
        self.last_used_at = turn.at;
        self.turns.push(turn);
    }
}

/// The `ValidationException` message a `sessionId` naming no session this service issued is
/// refused with: the caller cannot set the id itself, so an id the store does not hold is
/// not a session that can be continued. The identifier the request carried fills the one
/// variable part; the rest is fixed text for this error class.
pub fn session_not_valid_message(session_id: &str) -> String {
    format!("Session with Id {session_id} is not valid. Please check and try again.")
}

/// The `ValidationException` message a request earns when it continues a session under a
/// `sessionConfiguration` other than the one the session was opened with: the configuration
/// names the key the session is encrypted under, which is settled when the session is
/// opened, so a second value for it would describe a session this service does not hold.
pub fn session_configuration_conflict_message(session_id: &str) -> String {
    format!(
        "sessionConfiguration.kmsKeyArn: session {session_id} was opened under a different \
         key. The key encrypting a session is settled when the session is opened, so omit \
         sessionConfiguration to continue the session it was opened under."
    )
}

/// The `ServiceQuotaExceededException` message a store or a session at its ceiling is
/// refused with, naming the ceiling that was reached in the words this service's other
/// resource guards are stated in (`crate::ops::create_session`).
pub fn session_quota_message(limit: usize, what: &str) -> String {
    format!("This emulator holds at most {limit} {what} at once.")
}
