// SPDX-License-Identifier: CC0-1.0

//! A workspace's alert manager definition: the stored record, its lifecycle statuses and
//! the ONE rule that decides whether a `data` blob is an alert manager definition at all.
//! `crate::state` re-exports everything here, so a handler names `crate::state::…` for a
//! definition exactly as it does for a workspace.

use std::time::SystemTime;

/// The `resourceType` the modelled `ResourceNotFoundException` / `ConflictException`
/// report for a definition. CHOSEN, not modelled: the member is a free-form `String`
/// documented as "Type of the resource affected" and the model names no type for this
/// sub-resource (unlike `RuleGroupsNamespaceArn`'s `@arnReference`), so the emulator
/// reports the resource's name in the service's `AWS::APS::` namespace, as
/// [`crate::state::WORKSPACE_RESOURCE_TYPE`] does for a workspace.
pub const ALERT_MANAGER_DEFINITION_RESOURCE_TYPE: &str = "AWS::APS::AlertManagerDefinition";

/// The `message` the modelled `ConflictException` reports when a create is asked to add a
/// definition to a workspace that already holds one.
///
/// One CONSTANT sentence, exactly as [`crate::state::WORKSPACE_NOT_FOUND_MESSAGE`] is for
/// a workspace that does not exist: `ConflictException` models `resourceId` (@required)
/// beside `message`, so the workspace the refusal names has its own home on the wire and
/// the sentence carries nothing from the request. It states the condition the operation's
/// own documentation names — "If a workspace already has an alert manager definition,
/// don't use this operation to update it" — so every caller comparing it across calls
/// reads the same value.
pub const ALERT_MANAGER_DEFINITION_EXISTS_MESSAGE: &str =
    "This alert manager definition already exists for this workspace.";

/// `AlertManagerDefinitionStatusCode` values a definition moves through here, each an
/// entry of the modelled `com.amazonaws.amp#AlertManagerDefinitionStatusCode` enum, whose
/// documentation states the lifecycle rule this crate enforces: `CREATING` — "Definition
/// is being created. Update/Deletion is disallowed until definition is ACTIVE and
/// workspace status is ACTIVE."; `ACTIVE` — "Definition has been created/updated.";
/// `UPDATING` and `DELETING` repeat that same sentence.
pub const DEFINITION_STATUS_CREATING: &str = "CREATING";
/// See [`DEFINITION_STATUS_CREATING`].
pub const DEFINITION_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`DEFINITION_STATUS_CREATING`].
pub const DEFINITION_STATUS_UPDATING: &str = "UPDATING";
/// See [`DEFINITION_STATUS_CREATING`].
pub const DEFINITION_STATUS_DELETING: &str = "DELETING";

/// The largest definition blob this emulator reads and stores.
///
/// CHOSEN, not documented: `AlertManagerDefinitionData` carries no modelled length
/// constraint, so this is the emulator's own bound on one stored record, checked before
/// the document is read or held, and far above any configuration a workflow writes.
pub const ALERT_MANAGER_DEFINITION_MAX_BYTES: usize = 1 << 20;

/// How many parse events of a caller-supplied definition are read while checking it: the
/// reader is an iterative state machine, as `crate::state`'s rules-file reader is, so this
/// bounds the WORK one request can ask of it and the document's own nesting needs no bound.
///
/// It is a BACKSTOP rather than a working limit: every event consumes input, and the
/// densest YAML a blob within [`ALERT_MANAGER_DEFINITION_MAX_BYTES`] can carry stays below
/// this many events, so an accepted document is always read to the end. A document that
/// did hit it would be left half-read, which is why hitting it is a refusal, not a pass.
const MAX_DEFINITION_EVENTS: usize = 1 << 20;

/// The top-level key an alert manager definition document must carry.
///
/// `AlertManagerDefinitionData` is "The base-64 encoded blob that is alert manager
/// definition", pointing at the AlertManagedDefinitionData reference
/// (<https://docs.aws.amazon.com/prometheus/latest/APIReference/yaml-AlertManagerDefinitionData.html>),
/// which gives the document as a YAML mapping whose `alertmanager_config` entry holds the
/// Alertmanager configuration, with an optional `template_files` beside it.
pub const ALERT_MANAGER_CONFIG_KEY: &str = "alertmanager_config";

/// One workspace's alert manager definition: every member
/// `AlertManagerDefinitionDescription` reports, plus the bookkeeping its lifecycle needs.
#[derive(Clone)]
pub struct AlertManagerDefinition {
    /// The stored document, byte for byte as the blob member carried it.
    pub data: Vec<u8>,
    /// One of the `DEFINITION_STATUS_*` constants above.
    pub status_code: String,
    /// `AlertManagerDefinitionStatus$statusReason` — "If there is a failure, the reason for
    /// the failure", so the record holds none while nothing has failed and the read path
    /// publishes the service's no-failure reason for it
    /// ([`crate::state::published_status_reason`]).
    pub status_reason: Option<String>,
    /// When the definition was created — unchanged by a later update.
    pub created_at: SystemTime,
    /// When the definition was "most recently changed"
    /// (`AlertManagerDefinitionDescription$modifiedAt`), so a create and an update set it
    /// and a read never does.
    pub modified_at: SystemTime,
    /// When the clock owes the next transition, if one is pending:
    /// `CREATING`/`UPDATING` become `ACTIVE`, `DELETING` is removed.
    pub transition_due: Option<SystemTime>,
    /// The `clientToken` of the create that MINTED this definition, so replaying that
    /// exact call stays idempotent rather than conflicting with what it created.
    pub create_token: Option<String>,
    /// The `clientToken` of the update or delete that last CHANGED it, held apart from
    /// `create_token` so a replay is only ever matched against the operation that made it.
    pub change_token: Option<String>,
}

/// The debug rendering of a stored definition NEVER carries the document: `data` is
/// caller-supplied and an Alertmanager configuration routinely holds receiver credentials
/// (an SMTP password, a webhook token), which a derived `Debug` would put into any log
/// line or panic message that renders the store. The SIZE is what a debug rendering needs;
/// every other member carries no secret.
impl ::std::fmt::Debug for AlertManagerDefinition {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("AlertManagerDefinition")
            .field("data_bytes", &self.data.len())
            .field("status_code", &self.status_code)
            .field("status_reason", &self.status_reason)
            .field("created_at", &self.created_at)
            .field("modified_at", &self.modified_at)
            .field("transition_due", &self.transition_due)
            .field("create_token", &self.create_token)
            .field("change_token", &self.change_token)
            .finish()
    }
}

impl AlertManagerDefinition {
    /// Apply the transition the clock already owes this definition at `now`, and report
    /// whether the record is now GONE (a finished deletion), the caller's cue to drop it.
    ///
    /// The due instant was recorded by the call that started the transition, so it is owed
    /// whether or not anybody calls again: this materialises a change the clock made,
    /// never a side effect of a read. The idempotency tokens outlive it, so a replay that
    /// arrives afterwards is still the same call.
    pub fn settle_due(&mut self, now: SystemTime) -> bool {
        if self.transition_due.is_none_or(|due| now < due) {
            return false;
        }
        self.transition_due = None;
        if self.status_code == DEFINITION_STATUS_DELETING {
            return true;
        }
        self.status_code = DEFINITION_STATUS_ACTIVE.to_string();
        false
    }
}

/// The `AlertManagerDefinitionStatus` a stored definition reports.
///
/// ONE derivation for every path that publishes a definition's status, exactly as
/// [`crate::state::rule_groups_namespace_status`] is for a rule groups namespace: the
/// status code the record holds, and the reason it holds — the service's no-failure reason
/// while nothing has failed, never an absent member.
pub fn alert_manager_definition_status(
    definition: &AlertManagerDefinition,
) -> crate::ops::types::AlertManagerDefinitionStatus {
    crate::ops::types::AlertManagerDefinitionStatus {
        status_code: definition.status_code.clone(),
        status_reason: Some(crate::state::published_status_reason(
            definition.status_reason.as_deref(),
        )),
    }
}

/// The stored definition as the `AlertManagerDefinitionDescription`
/// `DescribeAlertManagerDefinition` reports — the document byte for byte as it was written,
/// its status, and the instants the record was stamped with.
pub fn alert_manager_definition_description(
    definition: &AlertManagerDefinition,
) -> crate::ops::types::AlertManagerDefinitionDescription {
    crate::ops::types::AlertManagerDefinitionDescription {
        status: alert_manager_definition_status(definition),
        data: definition.data.clone(),
        created_at: crate::state::wire_instant(definition.created_at),
        modified_at: crate::state::wire_instant(definition.modified_at),
    }
}

/// Why a blob is not an alert manager definition this service accepts: the two modelled
/// `ValidationExceptionReason` entries that fit — a document the parser cannot read is
/// `CANNOT_PARSE`, one that parses but breaks the member's rule is
/// `FIELD_VALIDATION_FAILED`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    /// The bytes are not one readable YAML document.
    CannotParse(String),
    /// The document parses, but is not an alert manager definition.
    FieldInvalid(String),
}

impl DefinitionError {
    /// The modelled `ValidationExceptionReason` this refusal reports.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::CannotParse(_) => "CANNOT_PARSE",
            Self::FieldInvalid(_) => "FIELD_VALIDATION_FAILED",
        }
    }

    /// What is wrong with the document, naming the rule it breaks.
    pub fn message(&self) -> &str {
        match self {
            Self::CannotParse(message) | Self::FieldInvalid(message) => message,
        }
    }
}

/// Check a `data` blob against what an alert manager definition is, so
/// `CreateAlertManagerDefinition` and `PutAlertManagerDefinition` apply ONE rule: the
/// blob's base64 is the wire encoding, so what arrives here is the document itself, and it
/// must be ONE readable YAML document whose root is a mapping carrying
/// [`ALERT_MANAGER_CONFIG_KEY`]. Anything else is refused with the modelled
/// `ValidationException`, never stored.
///
/// Read with `yaml-rust2` driven EVENT BY EVENT, the way `crate::state`'s rules-file
/// reader reads the other caller-supplied document this service takes: nothing recurses
/// over an untrusted document and an alias is never expanded, so the intake needs only the
/// blob's size bound and the reader's event budget. The document's own shape (how deeply it
/// nests, how it is indented, how many collections it opens) is left to YAML, so every
/// document the real service accepts is accepted here.
pub fn validate_alert_manager_definition(data: &[u8]) -> Result<(), DefinitionError> {
    use ::yaml_rust2::parser::Event;

    if data.is_empty() {
        return Err(DefinitionError::FieldInvalid(
            "the alert manager definition must not be empty".to_string(),
        ));
    }
    if data.len() > ALERT_MANAGER_DEFINITION_MAX_BYTES {
        return Err(DefinitionError::FieldInvalid(format!(
            "the alert manager definition must be at most {ALERT_MANAGER_DEFINITION_MAX_BYTES} bytes; this one is {} bytes",
            data.len()
        )));
    }
    let text = std::str::from_utf8(data).map_err(|error| {
        DefinitionError::CannotParse(format!(
            "the alert manager definition is not valid UTF-8: {error}"
        ))
    })?;

    let mut parser = ::yaml_rust2::parser::Parser::new_from_str(text);
    // How many collections are open: 0 is the document's root, 1 is inside the root
    // mapping, where this document's own key sits. In that mapping a key and its value
    // alternate, and a value that is itself a collection consumes the key when it opens.
    let mut depth = 0usize;
    let mut root_is_mapping = false;
    let mut documents = 0usize;
    let mut expecting_key = false;
    let mut carries_config = false;
    // The whole event stream must be read for the checks below to speak for the whole
    // document: a budget that runs out leaves the remainder unread.
    let mut read_to_the_end = false;

    for _ in 0..MAX_DEFINITION_EVENTS {
        let (event, _marker) = parser.next_token().map_err(|error| {
            DefinitionError::CannotParse(format!(
                "the alert manager definition is not valid YAML: {error}"
            ))
        })?;
        match event {
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return Err(DefinitionError::CannotParse(
                        "the alert manager definition must be exactly one YAML document"
                            .to_string(),
                    ));
                }
            }
            Event::MappingStart(..) | Event::SequenceStart(..) => {
                if depth == 0 {
                    root_is_mapping = matches!(event, Event::MappingStart(..));
                    expecting_key = root_is_mapping;
                } else if depth == 1 {
                    expecting_key = !expecting_key;
                }
                depth += 1;
            }
            Event::MappingEnd | Event::SequenceEnd => depth = depth.saturating_sub(1),
            // An alias refers to an anchored node. Nothing here expands one, so it costs
            // one event like any other: it stands for a key or a value at this level.
            Event::Scalar(..) | Event::Alias(..) => {
                if depth == 1 {
                    if expecting_key {
                        if let Event::Scalar(ref value, ..) = event {
                            carries_config |= value == ALERT_MANAGER_CONFIG_KEY;
                        }
                    }
                    expecting_key = !expecting_key;
                }
            }
            Event::StreamEnd => {
                read_to_the_end = true;
                break;
            }
            _ => {}
        }
    }

    if !read_to_the_end {
        // The reader stopped on its event budget, so what the document carries past that
        // point — a second document, an unterminated collection — was never seen. A
        // partial stream cannot decide the rules below, so the document is unreadable
        // here rather than accepted on what was read.
        return Err(DefinitionError::CannotParse(format!(
            "the alert manager definition must be readable within {MAX_DEFINITION_EVENTS} YAML parse events"
        )));
    }
    if !root_is_mapping {
        return Err(DefinitionError::FieldInvalid(format!(
            "the alert manager definition must be a YAML mapping carrying {ALERT_MANAGER_CONFIG_KEY}"
        )));
    }
    if !carries_config {
        return Err(DefinitionError::FieldInvalid(format!(
            "the alert manager definition must carry a top-level {ALERT_MANAGER_CONFIG_KEY} entry"
        )));
    }
    Ok(())
}
