// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivschat-amazoninteractivevideoservicechat service.
//!
//! This service persists two resources. The chat `Room`: `CreateRoom` mints it,
//! `GetRoom`/`ListRooms` report it, `UpdateRoom` mutates it and `DeleteRoom` removes it
//! together with every chat token issued against it. The `LoggingConfiguration`:
//! `CreateLoggingConfiguration` mints it, `GetLoggingConfiguration` and
//! `ListLoggingConfigurations` report it, `UpdateLoggingConfiguration` mutates it and
//! `DeleteLoggingConfiguration` removes it. Every response field an operation returns is
//! read back out of the record stored here, so a create followed by a get answers exactly
//! what was created.

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait, Violation};

use crate::ops::types::{
    DestinationConfiguration, MessageReviewHandler, ResourceNotFoundException, ValidationException,
    CREATE_LOGGING_CONFIGURATION_STATE_VALUES, RESOURCE_TYPE_VALUES,
    UPDATE_LOGGING_CONFIGURATION_STATE_VALUES,
};

/// Resolving the destination a logging configuration logs to against the service that owns
/// it, which is what a write that names one is gated on.
pub mod log_destinations;

/// How this service words a request it refuses because one member's value is wrong, and the
/// modelled `ValidationException` that wording travels in.
pub mod refusals;

pub use refusals::{
    check_range, field_validation_failed, member_path, missing_member_violation,
    stated_validation_exception, violation_clause, FIELD_VALIDATION_FAILED,
};

/// `CreateRoom`/`UpdateRoom` default for `maximumMessageRatePerSecond`, from the model's
/// own documentation of that member ("Maximum number of messages per second that can be
/// sent to the room (by all clients). Default: 10.").
pub const DEFAULT_MAXIMUM_MESSAGE_RATE_PER_SECOND: i32 = 10;

/// `CreateRoom`/`UpdateRoom` default for `maximumMessageLength`, from the model's own
/// documentation of that member ("Maximum number of characters in a single message …
/// Default: 500.").
pub const DEFAULT_MAXIMUM_MESSAGE_LENGTH: i32 = 500;

/// `CreateChatToken` default for `sessionDurationInMinutes`, from the model's own
/// documentation of that member ("Session duration (in minutes), after which the session
/// expires. Default: 60 (1 hour).").
pub const DEFAULT_SESSION_DURATION_IN_MINUTES: i32 = 60;

/// `ListRooms` default page size, from the model's own documentation of `maxResults`
/// ("Maximum number of rooms to return. Default: 50.").
pub const DEFAULT_MAX_ROOM_RESULTS: i32 = 50;

/// The `@range` the model declares for a numeric request member: the inclusive bounds a
/// value has to sit between, as `smithy_query(query="operation", …)` reports them.
///
/// Held as data rather than as two loose constants so one checker
/// ([`check_range`]) can refuse a violation of ANY of this service's range-constrained
/// members with the same message rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelledRange {
    /// Inclusive minimum.
    pub min: i32,
    /// Inclusive maximum.
    pub max: i32,
}

impl ModelledRange {
    const fn new(min: i32, max: i32) -> Self {
        Self { min, max }
    }
}

/// `CreateChatToken.sessionDurationInMinutes` targets `SessionDurationInMinutes`,
/// `@range(min: 1, max: 180)`.
pub const SESSION_DURATION_IN_MINUTES_RANGE: ModelledRange = ModelledRange::new(1, 180);

/// `ListRooms.maxResults` targets `MaxRoomResults`, `@range(min: 1, max: 50)`.
pub const MAX_ROOM_RESULTS_RANGE: ModelledRange = ModelledRange::new(1, 50);

/// `CreateRoom`/`UpdateRoom`'s `maximumMessageRatePerSecond` targets
/// `RoomMaxMessageRatePerSecond`, `@range(min: 1, max: 100)`.
pub const ROOM_MAX_MESSAGE_RATE_PER_SECOND_RANGE: ModelledRange = ModelledRange::new(1, 100);

/// `CreateRoom`/`UpdateRoom`'s `maximumMessageLength` targets `RoomMaxMessageLength`,
/// `@range(min: 1, max: 500)`.
pub const ROOM_MAX_MESSAGE_LENGTH_RANGE: ModelledRange = ModelledRange::new(1, 500);

/// The most tags one resource carries: the modelled `Tags` map is `@length(0..50)`, and
/// every operation that REPORTS a resource's tags reports them through that same shape, so
/// a resource holding more than this could not be rendered. It is also what bounds the
/// store: `TagResource` can be called repeatedly with fresh keys.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The HTTP status a successful tag mutation answers with.
///
/// `TagResource` and `UntagResource` are the two operations of this service that CHANGE a
/// resource's tags, and both declare an output structure with no members
/// (`TagResourceResponse` / `UntagResourceResponse`), so their answer conveys nothing but
/// its status. Both leave the status itself defaulted in the model (`@http` carries no
/// `code:`, i.e. `codeDefaulted`), so Smithy's fallback of 200 is not a statement about
/// this service; the recorded AWS responses are, and they answer `204 No Content` with no
/// body — the same status the two operations this model DOES pin explicitly carry
/// (`DeleteRoom` and `DeleteLoggingConfiguration`, `@http code 204`, equally empty-bodied).
/// A 200 there promises a document the response does not carry, so a client that branches
/// on the status parses a body that is not there.
///
/// `ListTagsForResource` keeps the defaulted 200: it HAS content, the `tags` map.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// The number of rooms one account may hold in one Region: Amazon IVS Chat publishes a
/// rooms-per-account-per-Region service quota (50,000), and `ServiceQuotaExceededException`
/// (HTTP 402) is `CreateRoom`'s modelled refusal past it. This emulator serves one
/// Region's worth of state, so the quota applies to the whole store — which is also what
/// bounds it: a repeated create cannot grow the process without limit.
pub const ROOMS_PER_REGION: usize = 50_000;

/// The most chat-token records this store keeps.
///
/// AWS publishes no token-retention quota and `CreateChatToken` models no quota error, so
/// this is the emulator's own memory bound rather than a service rule: a mint first drops
/// every record whose session the clock has already ended, then the oldest-expiring
/// records until the set fits. No operation reads an ended session, so dropping one
/// changes no answer this emulator gives.
pub const MAX_RETAINED_CHAT_TOKENS: usize = 10_000;

/// How long a freshly minted chat token stays usable for opening a connection. AWS does
/// not state this span in the model — the model only says the token "becomes invalid at
/// the token-expiration timestamp included in the response" — so the emulator chooses one
/// minute and reports it in `tokenExpirationTime`, which is the value a client must obey.
pub const CHAT_TOKEN_LIFETIME: Duration = Duration::from_secs(60);

/// Length of a `RoomID`: the model constrains `RoomID` to `len 12..12`, pattern
/// `^[a-zA-Z0-9]+$`.
pub const ROOM_ID_LEN: usize = 12;

/// Length of a `LoggingConfigurationID`: the model constrains it to `len 12..12`, pattern
/// `^[a-zA-Z0-9]+$`.
pub const LOGGING_CONFIGURATION_ID_LEN: usize = 12;

/// `ListLoggingConfigurations` default page size, from the model's own documentation of
/// `maxResults` ("Maximum number of logging configurations to return. Default: 50.").
pub const DEFAULT_MAX_LOGGING_CONFIGURATION_RESULTS: i32 = 50;

/// The largest page `ListLoggingConfigurations` answers, from the modelled
/// `MaxLoggingConfigurationResults` range 1..50.
pub const MAX_LOGGING_CONFIGURATION_RESULTS: i32 = 50;

/// How many logging configurations this store holds for one account in one Region.
///
/// The model carries no quota figure for this resource — only the per-room attachment
/// limit (`LoggingConfigurationIdentifierList`, len 0..3) — so this is the emulator's own
/// bound rather than a value read off the model: it is what keeps a repeated create from
/// growing the process without limit, and `ServiceQuotaExceededException` (HTTP 402) is
/// `CreateLoggingConfiguration`'s modelled refusal past it.
pub const LOGGING_CONFIGURATIONS_PER_REGION: usize = 50;

/// The ARN infix that names a logging configuration, from the modelled
/// `LoggingConfigurationArn` pattern
/// (`^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:logging-configuration/[a-zA-Z0-9-]+$`).
pub const LOGGING_CONFIGURATION_ARN_INFIX: &str = ":logging-configuration/";

/// This service's own namespace: the model's `arnNamespace` and `sigv4Name`, which
/// `smithy_query(query="service_info")` reports as `ivschat` for both. It is the service
/// segment of every ARN this service mints, and the prefix of the IAM action a refusal
/// that names the attempted operation spells (`<namespace>:<OperationName>`).
pub const SERVICE_NAMESPACE: &str = "ivschat";

/// The state `CreateLoggingConfiguration` reports a configuration it just minted in.
///
/// `CreateLoggingConfigurationState` carries exactly one value, so a configuration that
/// call answers is already ready to log: the transient members of
/// `LoggingConfigurationState` (`CREATING`, `UPDATING`, `DELETING`, the `*_FAILED` ones)
/// are states this operation never returns, and the clock owes the resource no transition.
pub fn created_logging_configuration_state() -> &'static str {
    CREATE_LOGGING_CONFIGURATION_STATE_VALUES
        .first()
        .copied()
        .unwrap_or("ACTIVE")
}

/// The state `UpdateLoggingConfiguration` reports a configuration it just updated in.
///
/// Read off `UpdateLoggingConfigurationState`'s own catalog rather than the create one, so
/// each operation reports the enum the model binds to IT even if the two ever diverge.
/// That enum also carries exactly one value, so an update answers a configuration that is
/// ready to log again.
pub fn updated_logging_configuration_state() -> &'static str {
    UPDATE_LOGGING_CONFIGURATION_STATE_VALUES
        .first()
        .copied()
        .unwrap_or("ACTIVE")
}

/// The alphabet room ids and chat tokens are rendered in — exactly the character set
/// `RoomID`'s `^[a-zA-Z0-9]+$` pattern admits.
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Domain separators mixed into the mint counter, so a room id and a chat token minted at
/// the same counter value are different strings.
const ROOM_ID_DOMAIN: u64 = 0x524f_4f4d_5f49_4400; // "ROOM_ID"
const LOGGING_CONFIGURATION_ID_DOMAIN: u64 = 0x4c4f_4747_5f49_4400; // "LOG_ID"
const CHAT_TOKEN_DOMAIN: u64 = 0x544f_4b45_4e5f_3031; // "TOKEN_01"
const CHAT_TOKEN_DOMAIN_HI: u64 = 0x544f_4b45_4e5f_3032; // "TOKEN_02"

/// SplitMix64's finalizer: a bijection on `u64`, so distinct mint counters always give
/// distinct ids while the rendered id carries no readable sequence number.
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Render `value` in base62, zero-padded to exactly `len` characters.
///
/// 62^12 exceeds `u64::MAX`, so a 12-character rendering of a `u64` loses nothing: the
/// mapping stays injective and the result matches `^[a-zA-Z0-9]+$`.
fn base62_fixed(mut value: u64, len: usize) -> String {
    let mut out = vec![b'0'; len];
    for slot in out.iter_mut().rev() {
        *slot = BASE62[(value % 62) as usize];
        value /= 62;
    }
    String::from_utf8(out).expect("the base62 alphabet is ASCII")
}

/// A chat room, as `CreateRoom` persisted it and the reporting operations render it.
#[derive(Debug, Clone)]
pub struct Room {
    /// System-generated `RoomID`: 12 characters, `^[a-zA-Z0-9]+$`.
    pub id: String,
    /// `arn:aws:ivschat:<region>:<account>:room/<id>`, built through `engine::emulator_arn`.
    pub arn: String,
    pub name: Option<String>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
    pub maximum_message_rate_per_second: i32,
    pub maximum_message_length: i32,
    pub message_review_handler: Option<MessageReviewHandler>,
    pub logging_configuration_identifiers: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

/// A logging configuration, as `CreateLoggingConfiguration` persisted it and the
/// reporting operations render it.
#[derive(Debug, Clone)]
pub struct LoggingConfiguration {
    /// System-generated `LoggingConfigurationID`: 12 characters, `^[a-zA-Z0-9]+$`.
    pub id: String,
    /// `arn:aws:ivschat:<region>:<account>:logging-configuration/<id>`, built through
    /// `engine::emulator_arn`.
    pub arn: String,
    pub name: Option<String>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
    /// The one destination chat content is logged to: the modelled
    /// `DestinationConfiguration` union, so exactly one of s3/cloudWatchLogs/firehose.
    pub destination_configuration: DestinationConfiguration,
    /// A member of the modelled `LoggingConfigurationState` enum.
    pub state: String,
    pub tags: BTreeMap<String, String>,
}

/// A chat token minted by `CreateChatToken` against a room.
#[derive(Debug, Clone)]
pub struct ChatToken {
    pub token: String,
    pub room_id: String,
    pub user_id: String,
    pub capabilities: Vec<String>,
    pub attributes: BTreeMap<String, String>,
    pub token_expiration_time: SystemTime,
    pub session_expiration_time: SystemTime,
}

/// Per-service state for com-amazonaws-ivschat-amazoninteractivevideoservicechat. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIvschatAmazoninteractivevideoservicechatState {
    /// Rooms keyed by `RoomID`, so a lookup by id or ARN is one map hit. `ListRooms`
    /// imposes its own documented order on top of this.
    pub rooms: BTreeMap<String, Room>,
    /// Chat tokens keyed by the token string itself.
    pub chat_tokens: BTreeMap<String, ChatToken>,
    /// Logging configurations keyed by `LoggingConfigurationID`, so a lookup by id or ARN
    /// is one map hit.
    pub logging_configurations: BTreeMap<String, LoggingConfiguration>,
    /// Monotonic mint counter. Never decreases, so an id is never reused after a delete.
    minted: u64,
}

impl ComAmazonawsIvschatAmazoninteractivevideoservicechatState {
    /// Mint a fresh `RoomID`: 12 characters from `^[a-zA-Z0-9]+$`, unique within this store.
    pub fn mint_room_id(&mut self) -> String {
        self.minted = self.minted.wrapping_add(1);
        base62_fixed(mix64(self.minted ^ ROOM_ID_DOMAIN), ROOM_ID_LEN)
    }

    /// Mint a fresh opaque chat token. Unique within this store: two calls never collide,
    /// so no caller can treat the value as a constant.
    pub fn mint_chat_token(&mut self) -> String {
        self.minted = self.minted.wrapping_add(1);
        let n = self.minted;
        format!(
            "{}{}",
            base62_fixed(mix64(n ^ CHAT_TOKEN_DOMAIN), 24),
            base62_fixed(mix64(n ^ CHAT_TOKEN_DOMAIN_HI), 24)
        )
    }

    /// The `RoomID` a room identifier names: the part after `:room/` of a room ARN, or a
    /// bare id. Does NOT say whether that room exists.
    ///
    /// The modelled `RoomIdentifier` pattern makes the boundary validator refuse anything
    /// but a room ARN before a handler runs, so the bare-id arm only serves callers
    /// inside the emulator; an identifier this cannot read at all answers `None`.
    pub fn room_id_of(identifier: &str) -> Option<&str> {
        if let Some((_, id)) = identifier.split_once(":room/") {
            return (!id.is_empty()).then_some(id);
        }
        if !identifier.is_empty() && identifier.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Some(identifier);
        }
        None
    }

    /// The stored room a room identifier names, if any.
    pub fn room(&self, identifier: &str) -> Option<&Room> {
        self.rooms.get(Self::room_id_of(identifier)?)
    }

    /// The stored room a room identifier names, for in-place mutation.
    pub fn room_mut(&mut self, identifier: &str) -> Option<&mut Room> {
        let id = Self::room_id_of(identifier)?.to_string();
        self.rooms.get_mut(&id)
    }

    /// Remove a room and every chat token issued against it.
    pub fn remove_room(&mut self, identifier: &str) -> Option<Room> {
        let id = Self::room_id_of(identifier)?.to_string();
        let room = self.rooms.remove(&id)?;
        self.chat_tokens.retain(|_, token| token.room_id != id);
        Some(room)
    }

    /// Whether the store already holds this Region's quota of rooms, so `CreateRoom` must
    /// answer the modelled `ServiceQuotaExceededException` instead of minting another.
    pub fn rooms_at_quota(&self) -> bool {
        self.rooms.len() >= ROOMS_PER_REGION
    }

    /// Mint a fresh `LoggingConfigurationID`: 12 characters from `^[a-zA-Z0-9]+$`, unique
    /// within this store.
    pub fn mint_logging_configuration_id(&mut self) -> String {
        self.minted = self.minted.wrapping_add(1);
        base62_fixed(
            mix64(self.minted ^ LOGGING_CONFIGURATION_ID_DOMAIN),
            LOGGING_CONFIGURATION_ID_LEN,
        )
    }

    /// The `LoggingConfigurationID` a logging-configuration identifier names: the part
    /// after `:logging-configuration/` of an ARN, or a bare id. Does NOT say whether that
    /// configuration exists.
    ///
    /// The modelled `LoggingConfigurationIdentifier` pattern makes the boundary validator
    /// refuse anything but an ARN before a handler runs, so the bare-id arm only serves
    /// callers inside the emulator; an identifier this cannot read at all answers `None`.
    pub fn logging_configuration_id_of(identifier: &str) -> Option<&str> {
        if let Some((_, id)) = identifier.split_once(LOGGING_CONFIGURATION_ARN_INFIX) {
            return (!id.is_empty()).then_some(id);
        }
        if !identifier.is_empty() && identifier.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Some(identifier);
        }
        None
    }

    /// The stored logging configuration an identifier names, if any.
    pub fn logging_configuration(&self, identifier: &str) -> Option<&LoggingConfiguration> {
        self.logging_configurations
            .get(Self::logging_configuration_id_of(identifier)?)
    }

    /// The stored logging configuration an identifier names, for in-place mutation.
    pub fn logging_configuration_mut(
        &mut self,
        identifier: &str,
    ) -> Option<&mut LoggingConfiguration> {
        let id = Self::logging_configuration_id_of(identifier)?.to_string();
        self.logging_configurations.get_mut(&id)
    }

    /// Remove a logging configuration.
    pub fn remove_logging_configuration(
        &mut self,
        identifier: &str,
    ) -> Option<LoggingConfiguration> {
        let id = Self::logging_configuration_id_of(identifier)?.to_string();
        self.logging_configurations.remove(&id)
    }

    /// The ids of the rooms this logging configuration is currently attached to, in the
    /// store's room order.
    ///
    /// A room holds its attachments as `LoggingConfigurationIdentifier` values, so the
    /// comparison is on the id each identifier names rather than on the raw string: a room
    /// attached by ARN and one attached by bare id name the same configuration.
    pub fn rooms_attached_to_logging_configuration(&self, identifier: &str) -> Vec<String> {
        let Some(wanted) = Self::logging_configuration_id_of(identifier) else {
            return Vec::new();
        };
        self.rooms
            .values()
            .filter(|room| {
                room.logging_configuration_identifiers
                    .iter()
                    .any(|attached| {
                        Self::logging_configuration_id_of(attached) == Some(wanted)
                    })
            })
            .map(|room| room.id.clone())
            .collect()
    }

    /// Whether the store already holds this Region's bound of logging configurations, so
    /// `CreateLoggingConfiguration` must answer the modelled
    /// `ServiceQuotaExceededException` instead of minting another.
    pub fn logging_configurations_at_quota(&self) -> bool {
        self.logging_configurations.len() >= LOGGING_CONFIGURATIONS_PER_REGION
    }

    /// Make room for one more chat-token record, keeping the set bounded.
    ///
    /// First drops every record whose session `now` has already ended — a change the clock
    /// had already made due — then, if the set is still at
    /// [`MAX_RETAINED_CHAT_TOKENS`], drops the oldest-expiring records in one batch so a
    /// steady stream of mints cannot grow the store without limit.
    pub fn prune_chat_tokens(&mut self, now: SystemTime) {
        self.chat_tokens
            .retain(|_, token| token.session_expiration_time > now);
        if self.chat_tokens.len() < MAX_RETAINED_CHAT_TOKENS {
            return;
        }
        // Trim to nine tenths of the cap, so the batch is amortised over many mints
        // rather than run on every one.
        let target = MAX_RETAINED_CHAT_TOKENS * 9 / 10;
        let excess = self.chat_tokens.len().saturating_sub(target);
        let mut by_expiry: Vec<(SystemTime, String)> = self
            .chat_tokens
            .iter()
            .map(|(token, record)| (record.session_expiration_time, token.clone()))
            .collect();
        by_expiry.sort();
        for (_, token) in by_expiry.into_iter().take(excess) {
            self.chat_tokens.remove(&token);
        }
    }
}

/// The modelled `ResourceNotFoundException` for an identifier that names no stored
/// resource.
///
/// The service builds the `Message` from the request by one rule, identical across its
/// operations: `Resource: <the identifier the caller sent> not found`. The identifier is
/// echoed exactly as received — the full ARN, not the id extracted out of it — because
/// that is the value the caller has to fix.
///
/// `resourceId` is the identifier the caller sent, echoed exactly as received — the value
/// the caller has to fix, and the same string the message names; `resourceType` is the
/// token for the KIND of resource that identifier names ([`resource_type_of`]).
pub fn resource_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Resource: {identifier} not found"),
        resource_id: identifier.to_string(),
        resource_type: resource_type_of(identifier),
    }
}

/// The `resourceType` token a refusal states for the kind of resource an identifier names:
/// the modelled `ResourceType` value for that kind when the model declares one, and
/// otherwise the resource-type element of the ARN as this service writes it there.
///
/// `RESOURCE_TYPE_VALUES` is the generated catalog of the modelled `ResourceType` enum and
/// carries `ROOM` alone, so a room ARN is reported under that modelled token while a
/// logging-configuration ARN — a kind the published enum does not carry — is reported
/// under its own ARN element (`logging-configuration`), which is what the recorded
/// refusals of each kind carry. An identifier that is not an ARN of this service at all
/// names no kind, and takes the modelled value.
fn resource_type_of(identifier: &str) -> String {
    let element = arn_resource_type(identifier).unwrap_or_default();
    RESOURCE_TYPE_VALUES
        .iter()
        .find(|modelled| modelled.eq_ignore_ascii_case(element))
        .map(|modelled| (*modelled).to_string())
        .unwrap_or_else(|| {
            if element.is_empty() {
                RESOURCE_TYPE_VALUES
                    .first()
                    .copied()
                    .unwrap_or("ROOM")
                    .to_string()
            } else {
                element.to_string()
            }
        })
}

/// The resource-type element of an ARN — `arn:<partition>:<service>:<region>:<account>:
/// <resource-type>/<id>` — when `identifier` is an ARN in this service's own namespace,
/// which is the only form the modelled identifier patterns of this service admit.
fn arn_resource_type(identifier: &str) -> Option<&str> {
    let fields: Vec<&str> = identifier.split(':').collect();
    let [prefix, _partition, service, _region, _account, resource, ..] = fields.as_slice() else {
        return None;
    };
    if !prefix.eq_ignore_ascii_case("arn") || *service != SERVICE_NAMESPACE {
        return None;
    }
    let element = resource.split('/').next().unwrap_or_default();
    (!element.is_empty()).then_some(element)
}

/// Render a page position as the opaque `nextToken` a paginated operation carries.
///
/// Hex-encoded so a caller cannot read or hand-build one: the only thing a client may do
/// with a token is pass it back, which is what `@paginated` means. Each paginator chooses
/// what its position string holds; this is only the codec they share.
pub fn encode_pagination_token(position: &str) -> String {
    hex::encode(position)
}

/// The page position an opaque `nextToken` names, or `None` when the value is not a token
/// this service issued.
pub fn decode_pagination_token(token: &str) -> Option<String> {
    let position = String::from_utf8(hex::decode(token).ok()?).ok()?;
    (!position.is_empty()).then_some(position)
}

/// The region a resource a request creates lives in: the one the caller signed for, since
/// an ARN names the region the request was addressed to. Falls back to the emulator's
/// default region for a request that carries no SigV4 credential scope, and for a scope
/// region the modelled ARN patterns (`[a-z0-9-]+`) would not admit.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ===========================================================================
// `resourceArn`: the tagging operations' own check for the member whose modelled
// `@pattern` the published model mis-states
// ===========================================================================
//
// `TagResource`/`UntagResource`/`ListTagsForResource` take `resourceArn`
// (`smithy_query(query="operation", operation="TagResource")`): `@length(1..128)` plus
// `@pattern(^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$)`. That pattern's
// resource-type segment is the SINGLE-character class `[a-z-]`, which no resource type
// this service mints can match — `room/…` and `logging-configuration/…` both need more
// than one character — so the published pattern refuses every ARN the service itself
// produces, and the real service accepts those ARNs. The three inputs therefore carry
// `skip_validation = ["resourceArn"]` in `src/ops/types/`, and this module re-declares the
// member's constraints with the only reading of that resource-type class under which a
// real ARN matches: one-or-more of the same characters. Everything else about the member
// — its length bound, the rest of the pattern — is enforced exactly as modelled.
//
// NOTHING about constraint checking is reimplemented here: the constraints are
// re-declared in the framework's own form (a [`TraitMap`] of
// [`LengthTrait`]/[`PatternTrait`]) hung on a member schema of the same name, and the
// check is [`validate`] — the same walk the request boundary runs — so a refusal made
// here reads exactly like one made there.

/// This service's Smithy namespace, for the member schema below.
const NAMESPACE: &str = "com.amazonaws.ivschat";

/// The modelled `ResourceArn` pattern with its resource-type segment read as one-or-more
/// of the characters the model's own class names, which is what makes a real `room/<id>`
/// or `logging-configuration/<id>` ARN match.
const RESOURCE_ARN_PATTERN: &str = r"^arn:aws:ivschat:[a-z0-9-]+:[0-9]+:[a-z-]+/[a-zA-Z0-9-]+$";

/// `ResourceArn`: the modelled `@length(1..128)` plus [`RESOURCE_ARN_PATTERN`].
static RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(RESOURCE_ARN_PATTERN)));
    traits
});

/// The `resourceArn` member schema the walk reads: its name gives the violation path and
/// its trait map carries the constraints.
static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ivschat#TaggedResource$resourceArn",
        NAMESPACE,
        "resourceArn",
    ),
    ShapeType::String,
    "resourceArn",
    0,
)
.with_traits(&RESOURCE_ARN_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`TaggedResource`] WRITES, and a root struct schema
/// carries no member name, so it contributes no path segment.
static TAGGED_RESOURCE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.ivschat#TaggedResource",
        NAMESPACE,
        "TaggedResource",
    ),
    ShapeType::Structure,
    &[],
);

/// One request's `resourceArn`, written for the validating walk the way the codec writes a
/// string member.
struct TaggedResource<'a> {
    resource_arn: &'a str,
}

impl SerializableStruct for TaggedResource<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_RESOURCE_ARN, self.resource_arn)
    }
}

/// Refuse a `resourceArn` that violates the member's length bound or its corrected
/// pattern, or pass it through.
///
/// The violations come from the framework's own walk, and the refusal they are stated in is
/// the one every member refusal of this service is stated in
/// ([`stated_validation_exception`]): the operation, the member path, and the requirement
/// the value failed — so a refusal made here reads exactly like one the walk at the request
/// boundary found.
pub fn check_resource_arn(
    operation: &str,
    resource_arn: &str,
) -> Result<(), ValidationException> {
    let violations: Vec<Violation> =
        validate(&TaggedResource { resource_arn }, &TAGGED_RESOURCE_SCHEMA);
    match stated_validation_exception(operation, &violations) {
        None => Ok(()),
        Some(refused) => Err(refused),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIvschatAmazoninteractivevideoservicechatState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIvschatAmazoninteractivevideoservicechatState>)
}

/// A `SystemTime` as the modelled `Time` shape, at the millisecond precision the service's
/// ISO 8601 timestamps carry.
pub fn as_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    ::aws_smithy_types::DateTime::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_room_ids_match_the_modelled_room_id_shape_and_never_repeat() {
        let mut state = ComAmazonawsIvschatAmazoninteractivevideoservicechatState::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..1_000 {
            let id = state.mint_room_id();
            assert_eq!(id.len(), ROOM_ID_LEN, "room id {id} must be 12 characters");
            assert!(
                id.bytes().all(|b| b.is_ascii_alphanumeric()),
                "room id {id} must match ^[a-zA-Z0-9]+$"
            );
            assert!(seen.insert(id.clone()), "room id {id} was minted twice");
        }
    }

    #[test]
    fn minted_chat_tokens_never_repeat() {
        let mut state = ComAmazonawsIvschatAmazoninteractivevideoservicechatState::default();
        let first = state.mint_chat_token();
        let second = state.mint_chat_token();
        assert_ne!(first, second, "two mints must not return the same token");
        assert!(!first.is_empty(), "a chat token must not be empty");
    }

    #[test]
    fn a_room_identifier_resolves_from_an_arn_or_a_bare_id() {
        for (identifier, expected) in [
            (
                "arn:aws:ivschat:us-east-1:000000000000:room/abc123ABC456",
                Some("abc123ABC456"),
            ),
            (
                "arn:aws:ivschat:eu-west-2:000000000000:room/zz",
                Some("zz"),
            ),
            ("abc123ABC456", Some("abc123ABC456")),
            ("arn:aws:ivschat:us-east-1:000000000000:room/", None),
            ("not an identifier", None),
            ("", None),
        ] {
            let actual =
                ComAmazonawsIvschatAmazoninteractivevideoservicechatState::room_id_of(identifier);
            assert_eq!(
                actual, expected,
                "case {identifier:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn pruning_drops_ended_sessions_and_keeps_the_token_store_bounded() {
        let mut state = ComAmazonawsIvschatAmazoninteractivevideoservicechatState::default();
        let now = SystemTime::now();

        // Rows differ in how far the session expiry sits from `now`: two already ended,
        // one still live.
        for (token, ended, offset_secs) in [
            ("ended-long-ago", true, 3_600_u64),
            ("ended-just-now", true, 1),
            ("still-live", false, 600),
        ] {
            let expires = if ended {
                now - Duration::from_secs(offset_secs)
            } else {
                now + Duration::from_secs(offset_secs)
            };
            state.chat_tokens.insert(
                token.to_string(),
                ChatToken {
                    token: token.to_string(),
                    room_id: "abc123ABC456".to_string(),
                    user_id: "alice".to_string(),
                    capabilities: Vec::new(),
                    attributes: BTreeMap::new(),
                    token_expiration_time: expires,
                    session_expiration_time: expires,
                },
            );
        }
        state.prune_chat_tokens(now);
        let kept: Vec<&str> = state.chat_tokens.keys().map(String::as_str).collect();
        assert_eq!(
            kept,
            vec!["still-live"],
            "case ended sessions: expected only the live token, got {kept:?}"
        );

        // At the cap, the oldest-expiring records go and the newest-expiring stay.
        for index in 0..MAX_RETAINED_CHAT_TOKENS {
            let token = format!("token-{index:06}");
            let expires = now + Duration::from_secs(60 + index as u64);
            state.chat_tokens.insert(
                token.clone(),
                ChatToken {
                    token,
                    room_id: "abc123ABC456".to_string(),
                    user_id: "alice".to_string(),
                    capabilities: Vec::new(),
                    attributes: BTreeMap::new(),
                    token_expiration_time: expires,
                    session_expiration_time: expires,
                },
            );
        }
        let before = state.chat_tokens.len();
        state.prune_chat_tokens(now);
        let after = state.chat_tokens.len();
        assert!(
            after < MAX_RETAINED_CHAT_TOKENS,
            "case at the cap: expected fewer than {MAX_RETAINED_CHAT_TOKENS} records after \
             pruning {before}, got {after}"
        );
        assert!(
            state
                .chat_tokens
                .contains_key(&format!("token-{:06}", MAX_RETAINED_CHAT_TOKENS - 1)),
            "case at the cap: the newest-expiring record must survive, got {after} records"
        );
        assert!(
            !state.chat_tokens.contains_key("token-000000"),
            "case at the cap: the oldest-expiring record must be the one dropped"
        );
    }

    #[test]
    fn the_room_store_reports_the_region_quota_as_reached() {
        let mut state = ComAmazonawsIvschatAmazoninteractivevideoservicechatState::default();
        let now = SystemTime::now();
        assert!(
            !state.rooms_at_quota(),
            "case empty store: the quota is not reached"
        );
        for index in 0..ROOMS_PER_REGION {
            let id = format!("{index:012}");
            state.rooms.insert(
                id.clone(),
                Room {
                    arn: format!("arn:aws:ivschat:us-east-1:000000000000:room/{id}"),
                    id,
                    name: None,
                    create_time: now,
                    update_time: now,
                    maximum_message_rate_per_second: DEFAULT_MAXIMUM_MESSAGE_RATE_PER_SECOND,
                    maximum_message_length: DEFAULT_MAXIMUM_MESSAGE_LENGTH,
                    message_review_handler: None,
                    logging_configuration_identifiers: Vec::new(),
                    tags: BTreeMap::new(),
                },
            );
        }
        assert!(
            state.rooms_at_quota(),
            "case {ROOMS_PER_REGION} rooms: the quota is reached, so a create must be refused"
        );
    }
}
