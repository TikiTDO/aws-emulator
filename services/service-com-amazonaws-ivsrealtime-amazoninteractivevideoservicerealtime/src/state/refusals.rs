// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS and CLASSIFIES the failures it answers.
//!
//! Every message here is built from the REQUEST (the resource it named, the member it
//! sent, the quota it met) by one rule, stated once and used by every handler that
//! answers that failure, so two operations cannot word the same refusal two ways. The
//! error CODE and the HTTP status stay model-derived — on the modelled error shape the
//! operation declares (see [`crate::scaffolded::Scaffolded::stated`]) — except on the
//! surfaces the recordings show AWS answering a code its model does not declare, which
//! travel through [`crate::scaffolded::Scaffolded::unmodeled`].

use std::sync::LazyLock;

use super::*;

/// How this service states a request that named a resource it does not hold: the
/// identifier the request named, under a fixed template.
///
/// One rule for every operation that takes an identifier. It is the same sentence the
/// sibling IVS endpoints were recorded answering (`Resource: <arn> not found` for the
/// low-latency `ivs` service's `GetChannel` / `UpdateChannel` / `CreateStreamKey`, and for
/// `ivschat`), and the recorded IVS RealTime `CreateParticipantToken` not-found carries it
/// too, which is what makes it a rule of the family rather than wording of one operation.
pub fn resource_not_found_message(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// How this service states a quota refusal (`ServiceQuotaExceededException`, HTTP 402):
/// it names the resource the quota is on and the limit that was reached.
pub fn quota_exceeded_message(resource: &str, limit: usize) -> String {
    format!("Resource: {resource} quota exceeded limit: {limit}")
}

/// How this service states an import of public key material it already holds
/// (`ConflictException`, HTTP 409): one fixed sentence naming the member whose value is
/// already present, with nothing of the request in it.
///
/// It is the sentence the sibling IVS endpoint was recorded answering for the same
/// refusal on its own key import, with the resource named as this model's ARN spells
/// this one (`public-key`) — the same family rule [`resource_not_found_message`] follows.
pub const DUPLICATE_PUBLIC_KEY_MESSAGE: &str =
    "A public-key resource already exists with the given publicKeyMaterial";

/// How this service states a start of a composition it already holds one of under the same
/// request (`ConflictException`, HTTP 409): one fixed sentence, with nothing of the request
/// in it.
///
/// The recorded refusal of a `StartComposition` replayed under the `@idempotencyToken` of a
/// composition this account and Region already hold, worded as the family states a create
/// of a resource that already exists ([`DUPLICATE_PUBLIC_KEY_MESSAGE`]): the token is one of
/// the attributes a start asks for, so a replay of it names a composition that is already
/// there rather than a second one to mint.
pub const DUPLICATE_COMPOSITION_MESSAGE: &str =
    "A composition resource already exists with the given attributes";

/// How this service states a rejection of one member's VALUE for a reason the model
/// cannot express: it names the member and the constraint the value failed.
///
/// The form the sibling IVS endpoint was recorded answering for material it cannot
/// decode, so a refusal of the same member is worded the same across the IVS family
/// rather than in this endpoint's operation-prefixed form.
pub fn member_constraint_message(member: &str, constraint: &str) -> String {
    format!("Request member: {member} failed to satisfy the constraints: {constraint}")
}

/// How this service states a request whose member names a resource of this service that
/// this account holds none of: the operation, the dotted member path, the kind of
/// resource the member must name and the identifier the request sent, in the construction
/// every other member refusal of this service uses ([`violation_clause`]).
///
/// One rule for every member that carries a REFERENCE to another resource of this
/// service, so a dangling reference is refused in the same words whichever operation and
/// member carried it.
pub fn unresolved_reference_message(
    operation: &str,
    member: &str,
    resource: &str,
    identifier: &str,
) -> String {
    format!(
        "{operation}.{member} is required to name a {resource} this account holds, \
         found {identifier}"
    )
}

/// How this service states a `nextToken` it did not issue: the operation, the member
/// and the requirement, in the construction every other member refusal of this
/// service uses (`crate::state::violation_clause`).
///
/// One rule for every listing of this service — each pages over its own records under
/// its own token prefix — so a token from elsewhere is refused in the same words
/// whichever page it was offered to.
pub fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// How this service states a request that supplied more than one of a set of members
/// only one of which may be given: it names the members the request actually carried,
/// then the whole set, in the construction every other member refusal of this service
/// uses (`<Operation>.<member> …`).
///
/// One rule for every operation whose model documents such a set — the four
/// `ListParticipants` filters say it in those words ("Only one of filterByUserId,
/// filterByPublished, filterByState, or filterByRecordingState can be provided per
/// request") — so two operations cannot word the same refusal two ways. `supplied`
/// holds the members the request carried, in the order `exclusive` lists them, and
/// `exclusive` the whole set.
pub fn mutually_exclusive_members_message(
    operation: &str,
    supplied: &[&str],
    exclusive: &[&str],
) -> String {
    format!(
        "{operation} is required to have at most one of {}, found {}",
        exclusive.join(", "),
        supplied.join(", "),
    )
}

/// The DECISION [`mutually_exclusive_members_message`] words, held once for every
/// operation whose model documents such a set: a request satisfies the set unless it
/// carried more than one of its members, and one that carried more is refused with that
/// sentence. `Err` is the sentence to put on the `ValidationException`, the way
/// [`check_resource_arn`] and [`check_tag_count`] hand theirs back.
///
/// `members` is the WHOLE exclusive set, in the order the model declares it, each member
/// paired with whether this request carried it — so the set the sentence names and the
/// presence flags it is decided from cannot disagree. What "carried" means stays with the
/// call site, since it is the member's own: an absent `Option` for most, and for a member
/// carrying a documented default (`ListParticipants$filterByPublished` is `@default
/// false`) the value that actually asks for the member's effect.
pub fn check_at_most_one_member(operation: &str, members: &[(&str, bool)]) -> Result<(), String> {
    let carried = members.iter().filter(|(_, provided)| *provided);
    // The served answer needs no list at all, so the names are collected only for the
    // sentence a refusal states.
    if carried.clone().count() <= 1 {
        return Ok(());
    }
    let supplied: Vec<&str> = carried.map(|(member, _)| *member).collect();
    let exclusive: Vec<&str> = members.iter().map(|(member, _)| *member).collect();
    Err(mutually_exclusive_members_message(
        operation, &supplied, &exclusive,
    ))
}

/// How this service states a request that supplied the wrong number of a set of members
/// exactly one of which must be given: it names the members the request actually carried
/// (none, when it carried none) and then the whole set, in the construction every other
/// member refusal of this service uses (`<Operation>.<member> …`).
///
/// The companion of [`mutually_exclusive_members_message`], for a set whose members are
/// not merely exclusive but required as a choice — the place a destination publishes to,
/// say, where naming none describes no destination at all.
pub fn exactly_one_member_message(
    operation: &str,
    supplied: &[&str],
    exclusive: &[&str],
) -> String {
    format!(
        "{operation} is required to have exactly one of {}, found {}",
        exclusive.join(", "),
        if supplied.is_empty() {
            "none".to_string()
        } else {
            supplied.join(", ")
        },
    )
}

/// How this service states a LIST whose documented shape admits at most one element
/// naming each of a set of alternatives, but whose request named one alternative more
/// than once: it names the list member, the alternative that repeated and how many
/// elements named it, in the construction every other member refusal of this service
/// uses (`<Operation>.<member> …`).
///
/// The element-level companion of [`exactly_one_member_message`]: that one refuses an
/// element that names the wrong NUMBER of alternatives, this one refuses a list that
/// names one alternative too MANY times. `choice` is the repeated alternative, named as
/// the model names the member that selects it, and `found` the number of elements that
/// selected it.
pub fn at_most_one_element_per_choice_message(
    operation: &str,
    member: &str,
    choice: &str,
    found: usize,
) -> String {
    format!(
        "{operation}.{member} is required to have at most one element naming {choice}, \
         found {found}"
    )
}

/// What one start request asked for, as one comparable value.
///
/// A start carrying an `@idempotencyToken` a composition of this account and Region was
/// already started under is refused: the token is one of the attributes the start asks
/// for. WHICH refusal it is stated with depends on whether the rest of the request matches
/// too, so the token is stored beside a fingerprint of that rest — the stage, the effective
/// layout, the effective destination configurations in the order they were given, and the
/// tag set — and a token offered again with a different fingerprint is refused as a token
/// standing for two requests rather than as a composition that already exists.
///
/// The tag pairs are sorted first: a `Tags` map has no request order, so two spellings of
/// one tag set must fingerprint alike. Nothing else here is a map, so the rendering is
/// stable.
pub fn composition_fingerprint(
    stage_arn: &str,
    layout: &LayoutConfiguration,
    destinations: &[DestinationConfiguration],
    tags: &HashMap<String, String>,
) -> String {
    let mut pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    pairs.sort_unstable();
    format!("{stage_arn}|{layout:?}|{destinations:?}|{pairs:?}")
}

/// How this service states an `@idempotencyToken` offered again for a DIFFERENT request:
/// it names the operation, the member and the token, so the caller can see which token
/// already stands for something else.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request, the way every other refusal of this service is.
pub fn idempotency_token_reused_message(operation: &str, token: &str) -> String {
    format!(
        "{operation}.idempotencyToken is required to name the request it was first used \
         for, found {token} which already names a different request"
    )
}

/// The `ResourceNotFoundException` (HTTP 404) a handler answers when a request names a
/// resource this service does not hold. See [`resource_not_found_message`].
///
/// `variant` is the calling operation's OWN modelled error constructor —
/// `GetStageErrorModeled::ResourceNotFoundException`, say — so the compiler still refuses
/// a failure the operation's model does not declare, and the error code and HTTP status
/// stay derived from that modelled shape. Only the BODY is this service's own rendering:
/// AWS puts the text under the key `message`, which the modelled `exceptionMessage` member
/// is not (see `crate::scaffolded::STATED_ERROR_SCHEMA`). The modelled struct is filled in
/// with the same text, so the error is complete as a Rust value too.
pub fn resource_not_found<E>(
    variant: fn(ResourceNotFoundException) -> E,
    identifier: &str,
) -> Scaffolded<E> {
    let message = resource_not_found_message(identifier);
    Scaffolded::stated(
        variant(ResourceNotFoundException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// How this service names a resource a request addressed by a plain IDENTIFIER rather
/// than by an ARN: the kind of resource, then the identifier in quotes.
///
/// An ARN names its own kind, so [`resource_not_found_message`] needs nothing beside it;
/// a bare id (a stage session's `sessionId`, a participant's `participantId`) does not,
/// and a request that carries several of them — a stage, a session within it, a
/// participant within that — would otherwise be refused without saying WHICH of them
/// resolved to nothing. The recorded `GetStageSession` not-found names the kind and
/// quotes the id (`Resource: stage session "<sessionId>" not found`), so this is the
/// rule for every identifier of this service that is not an ARN.
pub fn named_resource(kind: &str, identifier: &str) -> String {
    format!("{kind} \"{identifier}\"")
}

/// Kind of resource a `sessionId` names, for [`named_resource`]: a stage SESSION, which
/// is what the model's `StageSession` is and what `GetStageSession` reads.
pub const STAGE_SESSION_RESOURCE: &str = "stage session";

/// Kind of resource a `participantId` names, for [`named_resource`]: one PARTICIPANT of
/// a stage session, which is what the model's `Participant` is.
pub const PARTICIPANT_RESOURCE: &str = "participant";

/// The wire error code this service's TAGGING surface answers for an ARN that names no
/// resource.
///
/// It is NOT the `ResourceNotFoundException` the rest of the service answers and the
/// model declares for these three operations: the recorded responses of
/// `ListTagsForResource` / `TagResource` / `UntagResource` carry the distinct code
/// `NotFoundException`, which is why it travels as
/// [`crate::scaffolded::Scaffolded::unmodeled`] rather than as a modelled variant. The
/// resource-specific reads of the very same resources keep the modelled spelling (see
/// [`resource_not_found`]).
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The HTTP status that recorded tagging not-found response carries.
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The text that recorded tagging not-found response carries: one fixed,
/// resource-agnostic sentence, with nothing of the request in it — not the ARN, and not
/// the kind of resource the ARN named.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The wire error code this service's TAGGING surface answers for a request it refuses
/// as malformed.
///
/// The recorded tag-limit refusal carries `BadRequestException`, not the
/// `ValidationException` the rest of this service answers for a malformed request — the
/// same split as [`TAGGING_NOT_FOUND_CODE`], and the reason this one is unmodelled too.
pub const TAGGING_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The HTTP status that recorded tagging bad-request response carries, the client-fault
/// status its code names.
pub const TAGGING_BAD_REQUEST_STATUS: u16 = 400;

/// The not-found the TAGGING surface answers for an ARN naming no resource:
/// [`TAGGING_NOT_FOUND_CODE`] with [`TAGGING_NOT_FOUND_STATUS`], which is not the
/// modelled error shape — hence the unmodelled channel.
pub fn tagging_not_found<E>() -> Scaffolded<E> {
    Scaffolded::unmodeled(
        TAGGING_NOT_FOUND_CODE,
        TAGGING_NOT_FOUND_STATUS,
        TAGGING_NOT_FOUND_MESSAGE,
    )
}

/// The refusal the TAGGING surface answers for a request it will not apply:
/// [`TAGGING_BAD_REQUEST_CODE`] with [`TAGGING_BAD_REQUEST_STATUS`], stating `message`.
pub fn tagging_bad_request<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::unmodeled(
        TAGGING_BAD_REQUEST_CODE,
        TAGGING_BAD_REQUEST_STATUS,
        message,
    )
}

/// The `ValidationException` (HTTP 400) a handler answers for a request it refuses for a
/// reason the model cannot express. The caller supplies the sentence, built from the
/// request by one of the rules above this line or by [`stated_violations`].
pub fn validation_failed<E>(
    variant: fn(ValidationException) -> E,
    message: impl Into<String>,
) -> Scaffolded<E> {
    let message = message.into();
    Scaffolded::stated(
        variant(ValidationException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ServiceQuotaExceededException` (HTTP 402) a handler answers when a create would
/// exceed a quota. See [`quota_exceeded_message`].
pub fn quota_exceeded<E>(
    variant: fn(ServiceQuotaExceededException) -> E,
    resource: &str,
    limit: usize,
) -> Scaffolded<E> {
    let message = quota_exceeded_message(resource, limit);
    Scaffolded::stated(
        variant(ServiceQuotaExceededException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ConflictException` (HTTP 409) a handler answers when the request would put the
/// store in a state contradicting one it already holds. The caller supplies the
/// sentence, built from the request by the rule beside the failure it states.
pub fn conflict<E>(
    variant: fn(ConflictException) -> E,
    message: impl Into<String>,
) -> Scaffolded<E> {
    let message = message.into();
    Scaffolded::stated(
        variant(ConflictException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// How this service states a stop of a composition that is not composing: fixed text
/// around the composition ARN the request named, and nothing else.
///
/// The model documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the wording comes from what
/// the service was recorded answering: one fixed sentence whose only variable part is the
/// ARN the request sent. The composition's own lifecycle state is NOT part of it — the
/// sentence a caller matches on cannot vary with a value this store keeps, so this takes
/// no state to interpolate.
pub fn composition_not_running_message(arn: &str) -> String {
    format!("Resource: {arn} is in a transitioning state")
}

/// How this service states a request to replicate a participant that is already
/// replicated into the stage it names: it names the participant and both stages, so the
/// caller can see which replication already holds.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request, the way every other refusal of this service is.
pub fn replication_already_active_message(
    participant_id: &str,
    source_stage_arn: &str,
    destination_stage_arn: &str,
) -> String {
    format!(
        "Resource: {} of {source_stage_arn} is already replicated to \
         {destination_stage_arn}",
        named_resource(PARTICIPANT_RESOURCE, participant_id),
    )
}

/// How this service states a request whose two ends of a replication are one stage: it
/// names the operation, the member that must differ and the stage the request sent, in the
/// construction every other member refusal of this service uses ([`violation_clause`]).
///
/// One rule for both operations that take the two ends of a replication: replication is
/// INTO another stage, and the model cannot say "these two members differ", so each
/// operation refuses it here in the same words.
pub fn same_stage_replication_message(operation: &str, destination_stage_arn: &str) -> String {
    format!(
        "{operation}.destinationStageArn is required to name a stage other than \
         sourceStageArn, found {destination_stage_arn}"
    )
}

/// How this service identifies ONE replication in a failure about it: the participant
/// being replicated and the two stages it is replicated between, which is what a request
/// names a replication by (this service stores no id for one).
///
/// Fed to [`resource_not_found`], so a stop of a replication this store does not hold
/// answers the one not-found sentence of this service ([`resource_not_found_message`])
/// rather than wording of its own. The participant is named the way every other
/// non-ARN identifier of this service is ([`named_resource`]).
pub fn replication_identifier(
    participant_id: &str,
    source_stage_arn: &str,
    destination_stage_arn: &str,
) -> String {
    format!(
        "{} of {source_stage_arn} replicated to {destination_stage_arn}",
        named_resource(PARTICIPANT_RESOURCE, participant_id),
    )
}

/// How this service states a delete of a resource a composition is still using: fixed text
/// around the ARN the request named, and nothing else.
///
/// One rule for every resource of this service a composition's destinations can name — the
/// encoder configuration `DeleteEncoderConfiguration`'s doc "ensures that no Compositions
/// are using", and the storage configuration `DeleteStorageConfiguration` says the same
/// about — so both are refused in the same words. It is the sentence the service was
/// recorded answering, whose only variable part is the ARN the request sent: neither the
/// composition using it nor that composition's state is part of it, so the sentence a caller
/// matches on cannot vary with a value this store keeps, and the refusal names no resource
/// identity the request did not already carry.
pub fn resource_in_use_by_composition_message(arn: &str) -> String {
    format!("Resource: {arn} is attached to a composition")
}

/// How this service states a request to replicate a participant the source stage reports no
/// publisher for: the participant id the request sent, then fixed text.
///
/// The sentence the service was recorded answering, and the one rule for the whole class:
/// `StartParticipantReplication` replicates "a publishing participant" (its own doc), so a
/// source participant that is not publishing in the stage the request names — whether that
/// stage holds no such participant at all or holds one that publishes nothing — is a state
/// conflict about what the stage is doing, not a lookup that failed.
pub fn participant_not_publishing_message(participant_id: &str) -> String {
    format!("{participant_id} is not publishing")
}

/// How this service states a storage configuration over a bucket Amazon S3 holds none of in
/// this Region (`ValidationException`, HTTP 400): one fixed sentence, with nothing of the
/// request in it — not the bucket name, and not the Region.
///
/// The sentence the service was recorded answering. A storage configuration records to the
/// bucket its `s3.bucketName` names, in the Region the configuration is created in, so a
/// name that resolves to no bucket there is a value the request has to fix.
pub const BUCKET_NOT_FOUND_MESSAGE: &str = "bucket not found in this region";

/// How this service states an update of a member that cannot be changed while a
/// recording is running: it names the stage, the participant whose recording is running
/// and the state that recording is in, so the caller can see what has to finish before
/// the update can succeed.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request and the store, the way every other refusal of this service is.
pub fn stage_recording_active_message(
    arn: &str,
    participant_id: &str,
    recording_state: &str,
) -> String {
    format!(
        "Resource: {arn} is in use: the recording of participant {participant_id} is \
         {recording_state}"
    )
}

/// How this service states a request to delete an ingest configuration a publisher is
/// broadcasting with: it names the configuration, the participant publishing with it
/// and the stage it publishes to, so the caller can see what is in use.
///
/// AWS documents no text for `ConflictException` beyond what it means ("Updating or
/// deleting a resource can cause an inconsistent state"), so the sentence is built from
/// the request and the store, the way every other refusal of this service is.
pub fn ingest_configuration_publishing_message(
    arn: &str,
    participant_id: &str,
    stage_arn: &str,
) -> String {
    format!(
        "Resource: {arn} is in use: participant {participant_id} is publishing with it to \
         {stage_arn}"
    )
}

// =============================================================================
// How this service states a CONSTRAINT refusal
// =============================================================================
//
// The framework's constraint walk runs between deserialize and `handle` and words a
// violation the same way for every service ("1 validation error detected: Value '…' at
// '…' failed to satisfy constraint: …"). IVS RealTime words it from the OPERATION and the
// member path instead ("CreateParticipantToken.duration is required to have a maximum
// value of 20160.0, found 20161.0" — recorded), and the operation is not reachable at the
// framework's refusal seam: `Service::override_refusal` is handed the violations alone.
// `crate::service`'s per-operation wrapper collects the framework's OWN violations and
// brings the operation with it; the sentences are here, beside the message rules above,
// and not one modelled constraint is restated — every bound and pattern a clause prints
// comes off the `Violation` the walk produced.

/// A numeric bound or submitted value as this service's validation layer renders it: the
/// decimal form, so an integral value carries a `.0` (`20160` reads as `20160.0`).
///
/// The bound arrives as the text the constraint trait displays with and the submitted
/// value as the text the request carried, so a value that already shows a fraction is
/// left exactly as it is.
pub(super) fn as_decimal(value: &str) -> String {
    if value.contains('.') {
        value.to_string()
    } else {
        format!("{value}.0")
    }
}

/// One violation as this service states it: the operation, the dotted member path and
/// the constraint the request violated.
///
/// The `@range` form is the recorded one. The remaining kinds are derived from it by the
/// same construction — the operation and member path, then the requirement — because
/// AWS's exact wording for those is not recorded; the ERROR CODE and status they travel
/// with are the modelled ones either way.
pub fn violation_clause(operation: &str, violation: &Violation) -> String {
    let path = violation.path.as_str();
    let found = violation.value.as_deref().unwrap_or_default();
    match &violation.kind {
        ViolationKind::MinValue(minimum) => format!(
            "{operation}.{path} is required to have a minimum value of {}, found {}",
            as_decimal(minimum),
            as_decimal(found),
        ),
        ViolationKind::MaxValue(maximum) => format!(
            "{operation}.{path} is required to have a maximum value of {}, found {}",
            as_decimal(maximum),
            as_decimal(found),
        ),
        ViolationKind::Pattern(pattern) => {
            format!("{operation}.{path} does not match the required pattern {pattern}")
        }
        ViolationKind::MinLength(minimum) => format!(
            "{operation}.{path} is required to have a minimum length of {minimum}, found {}",
            found_length(violation),
        ),
        ViolationKind::MaxLength(maximum) => format!(
            "{operation}.{path} is required to have a maximum length of {maximum}, found {}",
            found_length(violation),
        ),
        ViolationKind::MissingRequiredMember => format!("{operation}.{path} is required"),
        ViolationKind::EnumValue(allowed) => format!(
            "{operation}.{path} is required to be one of [{}]",
            allowed.join(", ")
        ),
        ViolationKind::UniqueItems => {
            format!("{operation}.{path} is required to have unique values")
        }
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class the framework gains
        // later is stated in the same construction rather than silently unworded.
        _ => format!("{operation}.{path} does not satisfy the constraint the model declares"),
    }
}

/// `violations` as this service states them for `operation`, or `None` when there is
/// nothing to refuse.
///
/// The recorded refusal carries ONE violation and states its sentence alone — no count
/// prefix, unlike the framework's envelope — so one violation is its clause; several are
/// joined, in the order the walk collected them.
pub fn stated_violations(operation: &str, violations: &[Violation]) -> Option<String> {
    match violations {
        [] => None,
        [only] => Some(violation_clause(operation, only)),
        many => Some(
            many.iter()
                .map(|violation| violation_clause(operation, violation))
                .collect::<Vec<String>>()
                .join("; "),
        ),
    }
}

/// The length the request actually sent, for a `@length` violation.
///
/// A `Violation` carries WHAT was sent, not its size, so the size is read off that
/// rendering: the framework renders a collection as `"<n> items"` and a string as the
/// string itself. A string longer than the framework's echo cap arrives elided, which is
/// the one case this count is short — it affects the number in the message and nothing
/// about the refusal.
pub(super) fn found_length(violation: &Violation) -> usize {
    let Some(value) = violation.value.as_deref() else {
        return 0;
    };
    value
        .strip_suffix(" items")
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| value.chars().count())
}

/// The violation for a `@required` member the request did not send.
///
/// Absence is found while READING the request, so it arrives as a codec error rather
/// than from the constraint walk; this is the same `Violation` the framework's own
/// `refusal_of` builds for it, so one refusal is not stated two ways.
pub fn missing_member_violation(member_name: String) -> Violation {
    Violation {
        path: member_name,
        kind: ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    }
}

// =============================================================================
// The tagging surface: the `resourceArn` it admits, and the resource it names
// =============================================================================
//
// `ListTagsForResource` / `TagResource` / `UntagResource` all carry the resource's ARN
// in the `/tags/{resourceArn}` path label, and all three target the same `ResourceArn`
// shape. Both halves of that surface live here: the admission rule for the label, and
// the one lookup that maps an ARN onto the stored resource whose tags it names, so the
// three operations cannot disagree about either.

/// The HTTP status a successful tagging call that has NO content to report answers with.
///
/// `TagResource` and `UntagResource` leave their success code defaulted in the model
/// (`@http` carries no `code:`, i.e. `codeDefaulted: true`), so the Smithy default of 200
/// is not a statement about this service — the status is a behavioural fact, and the
/// recorded AWS responses are what states it: both answer `204 No Content` with an empty
/// body, which is what their output structures carry (`TagResourceResponse` and
/// `UntagResourceResponse` declare no members). A 200 there promises a body the response
/// does not carry, so every client path gated on 204 — conditional body parsing, caches
/// and proxies that key on it — takes the wrong branch.
///
/// `ListTagsForResource` keeps the defaulted 200: it HAS content, the `tags` map.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// The modelled `ResourceArn` `@pattern` with its resource-type element read as
/// one-or-more of the characters the model's own character class names.
///
/// The model writes that element once (`[a-z-]`), which matches exactly ONE character,
/// so taken literally no IVS ARN that exists could satisfy it — every resource type in
/// this model's ARN shapes (`stage`, `encoder-configuration`, `ingest-configuration`,
/// `storage-configuration`, `composition`, `public-key`) is longer than one character,
/// and the recordings show AWS accepting `…:stage/<id>` on all three tagging operations
/// and round-tripping that stage's tags. Repeating the group is the only reading of the
/// model under which the recorded behaviour is possible; everything else about the
/// pattern — the literal `arn:aws:ivs:` prefix, the region, account and id classes — is
/// the model's, unchanged.
pub(super) const RESOURCE_ARN_PATTERN: &str = r"^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]+/[a-zA-Z0-9-]+$";

/// `ResourceArn`: the modelled `@length(1..128)` plus [`RESOURCE_ARN_PATTERN`].
pub(super) static RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(RESOURCE_ARN_PATTERN)));
    traits
});

/// The `resourceArn` member schema the framework's constraint walk reads: its name gives
/// the violation path and its trait map carries the constraints.
pub(super) static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.ivsrealtime#TaggedResource$resourceArn",
        "com.amazonaws.ivsrealtime",
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
pub(super) static TAGGED_RESOURCE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.ivsrealtime#TaggedResource",
        "com.amazonaws.ivsrealtime",
        "TaggedResource",
    ),
    ShapeType::Structure,
    &[],
);

/// One request's `resourceArn`, written for the validating walk the way the codec writes
/// a string member.
pub(super) struct TaggedResource<'a> {
    pub(super) resource_arn: &'a str,
}

impl SerializableStruct for TaggedResource<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_RESOURCE_ARN, self.resource_arn)
    }
}

/// Check a tagging call's `resourceArn` against the member's modelled `@length` and
/// [`RESOURCE_ARN_PATTERN`]. `Err` is the sentence to put on the `ValidationException`.
///
/// The check is run by the FRAMEWORK's own constraint walk over the member schema above,
/// and the refusal is worded by [`stated_violations`] — the same two steps every other
/// refusal of this service goes through — so an ARN refused here is refused and worded
/// exactly as one the boundary walk refuses. The boundary cannot do it itself: the
/// modelled pattern is unsatisfiable (see [`RESOURCE_ARN_PATTERN`]), so each tagging
/// input carries `skip_validation = ["resourceArn"]` and this takes its place.
pub fn check_resource_arn(operation: &str, resource_arn: &str) -> Result<(), String> {
    let violations: Vec<Violation> = validate(&TaggedResource { resource_arn }, &TAGGED_RESOURCE_SCHEMA);
    match stated_violations(operation, &violations) {
        None => Ok(()),
        Some(message) => Err(message),
    }
}

/// Tags one resource may carry, from the modelled `Tags` `@length(0..50)`
/// (`smithy_query(query="shape", shape="Tags")`).
///
/// The framework's walk bounds the map ONE REQUEST sent; the bound is on the RESOURCE's
/// tag set, so `TagResource` checks the set its call would leave behind — a rule about
/// stored state, which no input shape expresses.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How the tagging surface states a call that would leave a resource carrying more
/// than [`MAX_TAGS_PER_RESOURCE`] tags: one fixed sentence about the set the call would
/// leave, with the modelled bound as its only variable part and nothing of the request
/// in it.
///
/// The bound is read off [`MAX_TAGS_PER_RESOURCE`], which is the modelled `Tags`
/// `@length` maximum, so the sentence cannot state a limit this service does not
/// enforce.
pub fn tag_limit_exceeded_message() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags.")
}

/// Refuse a call that would leave a resource carrying more than
/// [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// `Err` is the whole refusal, not a sentence for the caller to classify: the bound
/// belongs to the tagging surface, which answers it as [`TAGGING_BAD_REQUEST_CODE`] and
/// not as the modelled `ValidationException` the rest of this service answers for a
/// malformed request (see [`tagging_bad_request`]).
pub fn check_tag_count<E>(found: usize) -> Result<(), Scaffolded<E>> {
    if found <= MAX_TAGS_PER_RESOURCE {
        return Ok(());
    }
    Err(tagging_bad_request(tag_limit_exceeded_message()))
}

/// The modelled name of the operation that WRITES a resource's tags, as
/// `smithy_server_runtime::Operation::NAME` carries it.
pub const TAG_RESOURCE_OPERATION: &str = "TagResource";

/// The member a tag write carries its map in.
pub const TAGS_MEMBER: &str = "tags";

/// Whether `violations` refuse `operation` because the `tags` map the request carried is
/// itself past the modelled `Tags` maximum.
///
/// That bound is the one modelled constraint of this service whose refusal is not this
/// service's constraint wording: it belongs to the tagging surface, which states it in its
/// own code and words ([`check_tag_count`]), so a request already over the bound — refused
/// by the framework's constraint walk before any handler runs — is answered exactly as a
/// composition over it is. Every other constraint of the surface, the `resourceArn`
/// included, keeps this service's constraint wording.
pub fn is_tag_limit_violation(operation: &str, violations: &[Violation]) -> bool {
    operation == TAG_RESOURCE_OPERATION
        && violations.iter().any(|violation| {
            violation.path == TAGS_MEMBER && matches!(violation.kind, ViolationKind::MaxLength(_))
        })
}

/// The tag-key prefix Amazon Web Services reserves for its OWN tags, on every resource
/// of every service.
///
/// Every `Tags`/`TagKeyList` member of this model defers to the AWS-wide tagging rules
/// ("See Best practices and strategies in Tagging AWS Resources and Tag Editor for
/// details, including restrictions that apply to tags and 'Tag naming limits and
/// requirements'; Amazon IVS has no constraints on tags beyond what is documented
/// there." — `smithy_query(query="doc", shape="TagResourceRequest", member="tags")`, and
/// the same sentence on `UntagResourceRequest$tagKeys`, `Stage$tags` and the create
/// requests). One of those restrictions is this prefix: it is reserved for AWS use, and a
/// caller can neither create, edit nor delete a tag key that claims it. `TagKey` carries
/// only `@length(1..128)`, so the rule is documented-but-unmodelled and belongs to the
/// handlers, exactly like the 1 KB `attributes` limit ([`MAX_ATTRIBUTES_BYTES`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` claims the reserved namespace.
///
/// Compared case-INSENSITIVELY: what AWS reserves is the namespace, not one spelling of
/// it, so `AWS:` and `Aws:` claim it as much as `aws:` does.
///
/// The comparison is over BYTES, because a tag key is modelled `@length(1..128)` with no
/// `@pattern` and so may be any UTF-8 text: slicing it at a byte offset could fall inside
/// a multi-byte character and panic, while taking the bytes answers `false` for such a
/// key — which is correct, since a key whose first four bytes are not `aws:` does not
/// claim the namespace.
pub(super) fn claims_reserved_tag_namespace(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}

/// Refuse a request that names a tag key in the reserved [`RESERVED_TAG_KEY_PREFIX`]
/// namespace. `Err` is the sentence to put on the `ValidationException`.
///
/// `member` is the dotted path of the member the keys arrived in (`tags` for a tagging or
/// create call, `tagKeys` for a removal), so the refusal reads in the construction every
/// other refusal of this service uses (`<Operation>.<member> …`, see
/// [`violation_clause`]). Called before anything is minted or written, so a refused call
/// leaves the store exactly as it was.
///
/// Every offending key is named, in sorted order: the keys of a `Tags` map have no request
/// order to report them in, so one refusal is worded one way whichever member carried them.
pub fn check_tag_keys_unreserved<'a>(
    operation: &str,
    member: &str,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| claims_reserved_tag_namespace(key))
        .collect();
    if reserved.is_empty() {
        return Ok(());
    }
    reserved.sort_unstable();
    reserved.dedup();
    Err(format!(
        "{operation}.{member} is required to name no key in the reserved \
         {RESERVED_TAG_KEY_PREFIX} namespace, found {}",
        reserved.join(", "),
    ))
}

/// Refuse a request whose `attributes` map is larger than the documented
/// [`MAX_ATTRIBUTES_BYTES`] total. `Err` is the sentence to put on the
/// `ValidationException`.
///
/// The limit is documented and carried by no modelled trait, so — like this service's
/// other documented-but-unmodelled rules ([`check_tag_keys_unreserved`]) — it lives here
/// once and every operation taking an `attributes` map maps the sentence onto its own
/// error. The measure is the one [`attributes_size`] computes.
///
/// `member` is the dotted path of the member the map arrived in (`attributes` for a
/// request carrying one directly, `participantTokenConfigurations.<n>.member.attributes`
/// for one inside a list), and the refusal is worded in the member-constraint envelope
/// this service states a rejection of one member's VALUE with
/// ([`member_constraint_message`]): the member, then the requirement it failed, with the
/// limit spelled as the model spells it ([`MAX_ATTRIBUTES_SIZE_LABEL`]) and the measured
/// size of the map the request sent.
///
/// The bound is a property of the request alone, so a caller runs this before any lookup
/// and before anything is minted or persisted: a request that is both oversized and names
/// no resource is answered as malformed, and a refused request leaves the store exactly as
/// it was.
pub fn check_attributes_size(
    member: &str,
    attributes: &HashMap<String, String>,
) -> Result<(), String> {
    let size = attributes_size(attributes);
    if size <= MAX_ATTRIBUTES_BYTES {
        return Ok(());
    }
    Err(member_constraint_message(
        member,
        &format!(
            "{member} is required to have a maximum size of {MAX_ATTRIBUTES_SIZE_LABEL}, \
             found {size} bytes"
        ),
    ))
}

/// Merge an `attributes` map a request carries over the one a stored identity already
/// holds, and refuse the composition when it passes the documented
/// [`MAX_ATTRIBUTES_BYTES`] total. `Ok` is the composed map, ready to persist; `Err` is
/// the sentence to put on the `ValidationException`.
///
/// The documented bound belongs to the map this service STORES and REPORTS — every shape
/// of this service carrying an `attributes` map documents it, including the ones a reader
/// answers with — not only to the half a caller filled in. So a map this service composes
/// from more than one source is measured AFTER it is composed, against the same measure
/// [`check_attributes_size`] uses: no operation persists, and so no reader reports, an
/// `attributes` map past the bound its own field documents. An overlapping key takes the
/// overriding map's value, so a composition can total less than the sum of its halves.
///
/// Like [`check_attributes_size`], this neither opens nor writes anything: a caller runs
/// it before it persists, so a refused request leaves the store exactly as it was.
pub fn merge_attributes_within_bound(
    member: &str,
    held: &HashMap<String, String>,
    overriding: HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut merged = held.clone();
    merged.extend(overriding);
    check_attributes_size(member, &merged)?;
    Ok(merged)
}
