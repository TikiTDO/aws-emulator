// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackage-mediapackage service.
//!
//! The records here are the single source of truth for every MediaPackage response:
//! a create stores a record, and every later read (`Describe*`, `List*`) is rendered
//! from the stored record, never from the request that asked for it.
//!
//! MediaPackage is a REGIONAL control plane, so each of the three resource maps is keyed
//! by the Region the resource was created in together with its id (see [`ResourceKey`]):
//! the same id in two Regions names two independent resources, and every operation
//! resolves an id only among the records of the Region ITS OWN request is served in
//! ([`request_region`]).
//!
//! Modelled shapes are reused as-is from [`crate::ops::types`] rather than mirrored
//! into parallel structs, so a stored packaging configuration is byte-for-byte what a
//! later read renders.

pub mod data_plane;
pub mod harvest_destination;
pub mod harvest_export;

use crate::ops::types::{
    Authorization, Channel, CmafPackage, CmafPackageCreateOrUpdateParameters, DashPackage,
    EgressAccessLogs, HarvestJob, HlsIngest, HlsManifest, HlsPackage, IngestEndpoint,
    IngressAccessLogs, MssPackage, OriginEndpoint, S3Destination, SpekeKeyProvider,
    StreamSelection,
};
use crate::scaffolded::ServiceError;
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, SystemTime};

/// This service's SigV4 signing name and ARN namespace (`OPERATIONS.md`, "Wire protocol").
pub const SERVICE_NAME: &str = "mediapackage";

/// The region an ARN or an endpoint hostname carries when the caller's request is
/// unsigned, so no SigV4 credential scope names one. `emulator_arn`'s documentation
/// names `us-east-1` as the emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ENDPOINT-level `ManifestName`'s documented default: "A short string that will be
/// used as the filename of the OriginEndpoint URL (defaults to \"index\")".
///
/// This is the only place the literal belongs. A CMAF child manifest's own `ManifestName`
/// documents a different default — "defaults to the manifestName for the OriginEndpoint" —
/// so a child inherits the endpoint's effective value (see [`stored_cmaf_package`]) and
/// reaches this literal only through an endpoint that took it itself.
pub const DEFAULT_MANIFEST_NAME: &str = "index";

/// How many ingest endpoints a Channel's `HlsIngest` carries. MediaPackage provisions a
/// redundant pair, so an encoder can push the same source to two ingest URLs.
pub const INGEST_ENDPOINT_COUNT: usize = 2;

/// Width, in BYTES, of every value MediaPackage system-generates for an ingest endpoint —
/// its `Id`, its `Username` and its `Password` — rendered as lowercase hex, so each is
/// `2 * INGEST_GENERATED_BYTES` = 32 characters.
///
/// `IngestEndpoint.Id` / `Username` / `Password` are documented as "system generated"
/// and carry no `length` or `pattern` constraint in the Smithy model
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#IngestEndpoint")`
/// reports only `@sensitive` on the two credentials), so the shape is the one recorded
/// MediaPackage responses show: a 32-character lowercase-hexadecimal token for each of
/// the three generated members. One constant, because the service mints all three with
/// the same generator and width.
pub const INGEST_GENERATED_BYTES: usize = 16;

/// The largest page MediaPackage serves, and the largest `MaxResults` it accepts.
///
/// NARROWER than the modelled `MaxResults` range (`@range` min 1 max 1000 — see
/// `smithy_query(query="shape",shape="MaxResults",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
/// because the service enforces its own page cap on top of the modelled range: a
/// recorded `ListChannels` refuses an out-of-range `maxResults` with
/// "maxResults must be an integer between 1 and 50". The modelled range is what a
/// client-side validator checks; THIS is what the service applies, so it is the bound
/// [`page_size_violation`] refuses past and the page an absent `MaxResults` takes.
pub const MAX_PAGE_SIZE: usize = 50;

/// The smallest `MaxResults` MediaPackage accepts — the modelled `@range` minimum, which
/// the service's own bound shares.
pub const MIN_PAGE_SIZE: usize = 1;

/// The query parameter the paginated operations bind their page size to: `MaxResults` is
/// `@httpQuery("maxResults")` and the `@paginated` trait's `pageSize` on every `List*`
/// operation (`smithy_query(query="operation",operation="ListChannels",…)`).
pub const PAGE_SIZE_QUERY: &str = "maxResults";

// ---------------------------------------------------------------------------
// Caller-driven growth bounds
//
// Both stores below live for the life of the process and a create is the only thing
// that adds to them, so each needs a maximum ENTRY COUNT: neither a TTL nor an eviction
// policy is available to this service, because `DescribeChannel`/`DescribeOriginEndpoint`
// must still answer what the matching create created. The two operations that grow a
// store refuse past its cap with the service's own modelled `TooManyRequestsException`
// (HTTP 429, "The client has exceeded their resource or throttling limits"), which
// MediaPackage declares on both of them and on no read.
//
// The per-record caller-sized collections and strings are bounded too, with
// `UnprocessableEntityException` (HTTP 422, "The parameters sent in the request are not
// valid"), so one request cannot decide how much memory its record occupies. The product
// is finite: at most MAX_CHANNELS channels and MAX_ORIGIN_ENDPOINTS endpoints, each with
// at most MAX_TAGS tags, MAX_WHITELIST_ENTRIES whitelist entries,
// MAX_CMAF_HLS_MANIFESTS child CMAF manifests and MAX_PACKAGE_LIST_ENTRIES entries in
// every other list a packaging configuration carries, every caller-supplied string
// capped.
// ---------------------------------------------------------------------------

/// The most channels this store holds. The emulator's own resource guard, not a
/// published AWS quota: AWS publishes "Channels per account per Region" as an adjustable
/// quota, so no single value is the contract, and this is set well above any ordinary
/// workflow.
pub const MAX_CHANNELS: usize = 5_000;

/// The most origin endpoints this store holds — see [`MAX_CHANNELS`] for why a maximum
/// entry count is the bound available, and why the number is the emulator's own.
pub const MAX_ORIGIN_ENDPOINTS: usize = 20_000;

/// The most harvest jobs this store holds — see [`MAX_CHANNELS`]. `CreateHarvestJob` is
/// the only thing that adds one and `DescribeHarvestJob` must still answer what it
/// created, so the bound is an entry count, refused past with `TooManyRequestsException`.
pub const MAX_HARVEST_JOBS: usize = 20_000;

/// The most harvests ONE call materialises ([`ComAmazonawsMediapackageMediapackageState::take_due_harvests`]).
///
/// Carrying a harvest out writes an object to the destination, so how many a single
/// request would do that for is the number of jobs a caller has submitted — up to
/// [`MAX_HARVEST_JOBS`] of them, every one due at the same instant. This caps the work one
/// request pays for. Nothing is lost: a harvest this call does not take keeps the due
/// instant it was owed at, so the next call materialises it, and each is still carried out
/// exactly once.
pub const MAX_HARVESTS_PER_CALL: usize = 64;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit: a tag key is at most 128 characters.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit: a tag value is at most 256 characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The tag-key prefix AWS reserves for its OWN tags, which no caller may create, modify
/// or delete.
///
/// AWS's universal tag restrictions state it for every tagged resource: "Do not use
/// `aws:` ... as a prefix for keys, as it is reserved for AWS use. You cannot edit or
/// delete tag keys with this prefix", and such a tag "does not count against your tags
/// per resource limit". It is NOT a modelled constraint —
/// `smithy_query(query="operation",operation="TagResource",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`
/// types `Tags` as a plain `map<string,string>` with no key constraint — so the SERVICE
/// enforces it, not the framework's constraint walk.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The punctuation a tag key or value may carry, besides letters, numbers and spaces, as
/// AWS's tag restrictions spell the set. One constant, because it appears only inside the
/// refusal that names it.
const TAG_PUNCTUATION_CHARACTERS: &str = "_ . : / = + - @";

/// Emulator resource guard, NOT an AWS quota: the most `Whitelist` CIDR entries stored
/// for one origin endpoint.
pub const MAX_WHITELIST_ENTRIES: usize = 100;

/// Emulator resource guard, NOT an AWS quota: the most child HLS manifests stored for
/// one `CmafPackage`.
pub const MAX_CMAF_HLS_MANIFESTS: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the most entries stored for any OTHER
/// caller-sized list inside an origin endpoint's packaging configuration — a SPEKE key
/// provider's `systemIds`, and the `adTriggers` / `periodTriggers` trigger lists of a
/// packaging block or of a CMAF child manifest.
///
/// One constant for all of them, because the model publishes no `@length` for any of them
/// and they are the same kind of value: a list whose length is the caller's to choose and
/// whose entries the record keeps verbatim.
pub const MAX_PACKAGE_LIST_ENTRIES: usize = 100;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied string this
/// service stores for a member whose length the model publishes no limit for.
pub const MAX_STRING_CHARS: usize = 1024;

// ---------------------------------------------------------------------------
// How MediaPackage states a refusal
//
// A MediaPackage error body carries `message` AND an `errors` object keyed by the
// thing that was wrong — a RESOURCE TYPE when the request addressed a resource that is
// missing or in the way, a REQUEST MEMBER when a value is one the service will not
// accept — reporting that key's reasons (`crate::scaffolded::Reasons` describes the two
// shapes a recorded body uses). The sentence is built from the same reasons
// (`<key> <reason>`), so it is derived from the request rather than written twice; see
// [`crate::scaffolded::ServiceError`], which owns the envelope.
//
// A few classes the service words as ONE sentence of its own, against no key at all
// ([`invalid_page_token`]): the service's sentence is the message, and the body carries no
// `errors` entry, because the service reports none.
//
// Either way the wording is the SERVICE's, not this emulator's: a message a client reads
// is the text the real service states for that error class — its fixed part verbatim, and
// its variable parts (the member path, a bound, an id) taken from the request and the
// store. Every constructor below returns one of those envelopes, so this is the one home
// of this service's refusal text: a handler names the rule, never the sentence.
// ---------------------------------------------------------------------------

/// How a MediaPackage error names a Channel in `errors` — the resource type, lowerCamel,
/// as the wire names a member.
pub const CHANNEL_KIND: &str = "channel";

/// How a MediaPackage error names an OriginEndpoint in `errors`.
pub const ORIGIN_ENDPOINT_KIND: &str = "originEndpoint";

/// How a MediaPackage error names a HarvestJob in `errors`.
pub const HARVEST_JOB_KIND: &str = "harvestJob";

/// The Smithy shape name of the resource an OriginEndpoint is, as the dependency refusal
/// below names the children blocking a delete.
pub const ORIGIN_ENDPOINT_SHAPE: &str = "OriginEndpoint";

/// How MediaPackage NAMES an ingest endpoint inside the refusal of a lookup scoped to its
/// channel — the resource in prose, as the recorded channel-scoped `NotFoundException`
/// words it (trace `mediapackage.rotate-ingest-endpoint-credentials-invalid-endpoint-id-rejected`).
pub const INGEST_ENDPOINT_LABEL: &str = "Ingest Endpoint";

/// `NotFoundException` (404) for a resource the request addressed BY ID that this service
/// does not hold: keyed by the resource type, because what is missing is the resource and
/// not a value in the request.
pub fn resource_not_found(kind: &str, id: &str) -> ServiceError {
    ServiceError::not_found(kind, format!("with id={id} not found"))
}

/// [`resource_not_found`] for a Channel.
pub fn channel_not_found(id: &str) -> ServiceError {
    resource_not_found(CHANNEL_KIND, id)
}

/// [`resource_not_found`] for an OriginEndpoint.
pub fn origin_endpoint_not_found(id: &str) -> ServiceError {
    resource_not_found(ORIGIN_ENDPOINT_KIND, id)
}

/// [`resource_not_found`] for a HarvestJob.
pub fn harvest_job_not_found(id: &str) -> ServiceError {
    resource_not_found(HARVEST_JOB_KIND, id)
}

/// `NotFoundException` (404) for a CHILD resource the request addressed INSIDE its
/// parent's path, when the parent resolved but holds no such child.
///
/// 404 and not a 422 reference refusal for the same reason [`resource_not_found`] is: both
/// ids travel as `@httpLabel`s, so the request ADDRESSES the child rather than referring
/// to it from a body member.
///
/// Keyed by the PARENT, and reporting BOTH ids: the lookup was scoped to that parent, so
/// what the service can say is that this parent does not hold a child by that id — a
/// caller or an operator reading the refusal needs the scope as much as the id, because
/// the same child id may well exist under another parent. `child` names the child resource
/// as the refusal words it (e.g. [`INGEST_ENDPOINT_LABEL`]).
///
/// The sentence ends after the child id with a trailing space, which is what MediaPackage
/// itself emits for this refusal (recorded 2026-09-20, us-west-2), so error-mapping code
/// keyed on the service's own form matches here too.
pub fn child_not_found(
    parent_kind: &str,
    parent_id: &str,
    child: &str,
    child_id: &str,
) -> ServiceError {
    ServiceError::not_found(
        parent_kind,
        format!("with id={parent_id} does not have {child} with guid = {child_id} "),
    )
}

/// [`child_not_found`] for an IngestEndpoint addressed under its channel.
pub fn ingest_endpoint_not_found(channel_id: &str, ingest_endpoint_id: &str) -> ServiceError {
    child_not_found(
        CHANNEL_KIND,
        channel_id,
        INGEST_ENDPOINT_LABEL,
        ingest_endpoint_id,
    )
}

/// `UnprocessableEntityException` (422) for a create whose id is already in use: the
/// request cannot be processed against the state this service holds, and the resource
/// type is what is in the way.
pub fn resource_already_exists(kind: &str, id: &str) -> ServiceError {
    ServiceError::unprocessable_state(kind, format!("with id={id} already exists"))
}

/// `UnprocessableEntityException` (422) for a REQUEST MEMBER naming a resource this
/// service does not hold.
///
/// Keyed by the member, not by the resource type: on a create the unresolvable reference
/// is a value in the request the caller has to fix, which is a different answer from
/// "the resource you addressed is gone" ([`resource_not_found`], 404). The reason is
/// reported the way everything decided against STATE is, because resolving the reference
/// is what failed — not reading it.
pub fn member_not_found(member: &str) -> ServiceError {
    ServiceError::unprocessable_state(member, "not found")
}

/// `UnprocessableEntityException` (422) for a rule about the resource AS A WHOLE — a
/// combination of members no single member can be keyed by — reported like every other
/// reason the service decides for that resource.
pub fn resource_invalid(kind: &str, reason: impl Into<String>) -> ServiceError {
    ServiceError::unprocessable_state(kind, reason)
}

/// `UnprocessableEntityException` (422) from the request validator: the requirement
/// `member` failed, reported as that member's single requirement.
pub fn member_invalid(member: &str, requirement: impl Into<String>) -> ServiceError {
    ServiceError::unprocessable_member(member, requirement)
}

/// [`member_invalid`] for one constraint the model declares and the request violated, as
/// the framework's own constraint walk reported it: keyed by the member PATH the walk
/// names (lowerCamel, as the wire names it), stating the requirement that member failed.
///
/// This is how a violation of a declared constraint reaches the same envelope as a rule
/// the service enforces itself, so one service does not answer its two input-failure paths
/// in two different bodies.
pub fn member_violation(violation: &Violation) -> ServiceError {
    member_invalid(&violation.path, constraint_requirement(&violation.kind))
}

/// The requirement a violated constraint states, worded the way every other refused
/// member here is worded: the rule, with the member supplied by the envelope's key.
pub fn constraint_requirement(kind: &ViolationKind) -> String {
    match kind {
        ViolationKind::MissingRequiredMember => "is required".to_string(),
        ViolationKind::MinLength(min) => {
            format!("must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => format!("must have length less than or equal to {max}"),
        ViolationKind::MinValue(min) => format!("must be greater than or equal to {min}"),
        ViolationKind::MaxValue(max) => format!("must be less than or equal to {max}"),
        ViolationKind::Pattern(pattern) => {
            format!("must satisfy the pattern {pattern}")
        }
        ViolationKind::EnumValue(values) => {
            format!("must be one of {}", values.join(", "))
        }
        ViolationKind::UniqueItems => "must not carry a repeated entry".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class added to the
        // framework later says what little the envelope can say about it, rather than
        // failing to report the refusal at all.
        _ => "is not valid".to_string(),
    }
}

/// `UnprocessableEntityException` (422) for a delete refused because children are still
/// attached: the blocked resource type, how many children hold it, and what they are.
pub fn has_attached_children(kind: &str, child_shape: &str, count: usize) -> ServiceError {
    let children = if count == 1 {
        format!("{count} {child_shape} is")
    } else {
        format!("{count} {child_shape}s are")
    };
    ServiceError::unprocessable_state(
        kind,
        format!("cannot be deleted because {children} attached to it"),
    )
}

/// `TooManyRequestsException` (429) for a store that has reached its entry cap.
pub fn resource_limit_reached(kind: &str, max: usize) -> ServiceError {
    ServiceError::too_many_requests(
        kind,
        format!("limit of {max} for this account has been reached"),
    )
}

/// The whole sentence MediaPackage answers a `nextToken` it did not issue with.
///
/// FIXED text for the error class: the service states this one sentence for every refused
/// token, naming neither the offending value nor any other part of the request
/// (recorded 2026-10-29, `ListOriginEndpoints`). So it is written here once, verbatim,
/// rather than paraphrased per listing.
pub const INVALID_PAGE_TOKEN_MESSAGE: &str = "Invalid NextToken";

/// `UnprocessableEntityException` (422) for a `nextToken` this service did not issue.
///
/// Worded as the service's own sentence ([`INVALID_PAGE_TOKEN_MESSAGE`]) rather than as a
/// requirement reported against `nextToken`, which is how MediaPackage answers it.
pub fn invalid_page_token() -> ServiceError {
    ServiceError::unprocessable_sentence(INVALID_PAGE_TOKEN_MESSAGE)
}

/// `UnprocessableEntityException` (422) for two request members the service treats as
/// MUTUALLY EXCLUSIVE, both of which the request specified.
///
/// Keyed by `member` — the one the refusal is reported against, named as the wire names it
/// — and stating the other by its MODELLED member name, which is how MediaPackage words
/// this refusal. The service refuses the combination outright instead of applying both
/// rules together, so a request can never be served by a reading of two contradictory
/// arguments the caller did not intend.
pub fn mutually_exclusive_members(member: &str, other_member: &str) -> ServiceError {
    member_invalid(
        member,
        format!("cannot be specified at the same time as {other_member}"),
    )
}

/// The query parameter `ListHarvestJobs` binds `IncludeChannelId` to
/// (`smithy_query(query="operation",operation="ListHarvestJobs")`:
/// `IncludeChannelId` is `@httpQuery("includeChannelId")`).
pub const INCLUDE_CHANNEL_ID_QUERY: &str = "includeChannelId";

/// That member's modelled name — the other half of the refusal above.
pub const INCLUDE_CHANNEL_ID_MEMBER: &str = "IncludeChannelId";

/// The query parameter `ListHarvestJobs` binds `IncludeStatus` to
/// (`@httpQuery("includeStatus")`).
pub const INCLUDE_STATUS_QUERY: &str = "includeStatus";

/// The refusal `ListHarvestJobs` answers a request carrying BOTH of its filters with:
/// `IncludeChannelId` and `IncludeStatus` select harvest jobs by different properties and
/// MediaPackage does not combine them, so specifying both is refused rather than
/// intersected (recorded 2026-09-20, us-west-2).
pub fn harvest_job_filters_mutually_exclusive() -> ServiceError {
    mutually_exclusive_members(INCLUDE_STATUS_QUERY, INCLUDE_CHANNEL_ID_MEMBER)
}

/// The requirement an out-of-bound page size failed, worded as the service words it.
///
/// One sentence for every rejected value — too small, too large, or not a number at all —
/// because all three fail the same requirement.
pub fn page_size_requirement() -> String {
    format!("must be an integer between {MIN_PAGE_SIZE} and {MAX_PAGE_SIZE}")
}

/// `Some(error)` when `query` carries a page size outside the bound this service enforces
/// ([`MIN_PAGE_SIZE`]..=[`MAX_PAGE_SIZE`]), `None` when it carries an acceptable one or
/// none at all.
///
/// Reads the parameter AS SENT (decoded), so a value the modelled `@range` would never
/// admit — and a value that is not a number at all — is refused with the bound the service
/// applies rather than with the wider modelled one. The decoded value is read exactly as
/// the framework's HTTP binding reads it afterwards, so the gate refuses precisely what
/// the operation could not have read as a page size.
pub fn page_size_violation(query: &str) -> Option<ServiceError> {
    let sent = query_value(query, PAGE_SIZE_QUERY)?;
    let accepted = sent
        .parse::<usize>()
        .is_ok_and(|size| (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&size));
    (!accepted).then(|| member_invalid(PAGE_SIZE_QUERY, page_size_requirement()))
}

/// The query string of `uri` — everything after the first `?`, empty when it has none.
pub fn request_query(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The value `query` carries for the parameter `name`, read the way the framework's HTTP
/// binding reads one, so a rule applied here and the value the handler later sees cannot
/// disagree:
///
/// * the value is percent-decoded (`smithy_server_runtime::percent_decode`), so the rule
///   is applied to what the caller sent rather than to its encoding;
/// * a parameter sent with no `=` carries the EMPTY value, exactly as the binding's pair
///   reader does (`aws_smithy_schema_server::http_binding`), rather than being read as
///   absent;
/// * a parameter sent more than once answers with the FIRST value, which is the one the
///   binding hands a non-list member.
pub fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (!key.is_empty() && ::smithy_server_runtime::percent_decode(key) == name)
            .then(|| ::smithy_server_runtime::percent_decode(value))
    })
}

/// Refuse a caller-sized collection that would grow a record past `max` entries.
///
/// `Err` is the service's own envelope, keyed by the member and stating the bound it
/// broke, so the caller can fix the request from the error alone.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), ServiceError> {
    if count > max {
        return Err(member_invalid(
            member,
            format!("may carry at most {max} entries, got {count}"),
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), ServiceError> {
    bound_part_chars(member, "", value, max)
}

/// [`bound_chars`] for a string INSIDE a member — one entry of a list, one tag key — where
/// `part` names which part of the member the bound is about.
pub fn bound_part_chars(
    member: &str,
    part: &str,
    value: &str,
    max: usize,
) -> Result<(), ServiceError> {
    let length = value.chars().count();
    if length > max {
        let subject = if part.is_empty() {
            String::new()
        } else {
            format!("{part} ")
        };
        return Err(member_invalid(
            member,
            format!("{subject}may be at most {max} characters, got {length}"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Resource identifiers
//
// Every resource this service holds is keyed by the `Id` its create carried, and that
// same id is the `{Id}` http label of that resource's OWN reads and writes
// (`smithy_query(query="operation",operation="DescribeChannel,DeleteChannel,DescribeOriginEndpoint,DescribeHarvestJob")`
// bind `GET/PUT/DELETE /<collection>/{Id}`) as well as the last segment of its ARN. So an
// id the service could not address afterwards is not an id at all: the record would be
// listed forever and reachable by nothing. The v1 model targets each of them at the plain
// `__string` shape and publishes no `@pattern`, so the framework's constraint walk cannot
// refuse one — the rule is the service's, enforced here for every create that NAMES the
// resource it stores, and answered with `UnprocessableEntityException` (422) keyed by the
// member, storing nothing.
// ---------------------------------------------------------------------------

/// The non-alphanumeric characters a resource id may carry, as the refusal names them.
const ID_PUNCTUATION_CHARACTERS: &str = "_ -";

/// The shortest id a resource can be addressed by. One, not zero: the empty string
/// leaves the `{Id}` path segment of that resource's reads empty, which addresses the
/// collection rather than a member of it, so a record stored under it is unreachable.
pub const MIN_ID_CHARS: usize = 1;

/// The wire name of the member every create carries its new resource's id in
/// (`Id` is `@jsonName("id")` on all three create requests).
pub const ID_MEMBER: &str = "id";

/// Whether `c` is a character a MediaPackage resource id may carry.
///
/// The identifier alphabet: ASCII letters, digits, and the characters
/// [`ID_PUNCTUATION_CHARACTERS`] names. ASCII-only, like a log group name's set
/// ([`is_log_group_character`]) and unlike a tag's ([`is_tag_character`]), because the
/// MediaPackage v2 model pins the same identifier as `^[a-zA-Z0-9_-]+$` — and because
/// every character outside this set either has to be percent-encoded to survive the
/// `{Id}` label or splits the path entirely.
fn is_id_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

/// Whether `value` is an id a resource of this service can be addressed by: at least
/// [`MIN_ID_CHARS`] character, every one of them in [`is_id_character`]'s set.
pub fn is_resource_id(value: &str) -> bool {
    value.chars().count() >= MIN_ID_CHARS && value.chars().all(is_id_character)
}

/// The requirement a refused id failed: the member's whole accepted domain in one
/// sentence, so a blank id and one carrying a character outside the alphabet are answered
/// with the same rule.
///
/// Built here rather than written at each call site, so the sentence and the set it states
/// cannot disagree and the suite asserting it reads the service's own words.
pub fn resource_id_requirement() -> String {
    format!(
        "must be {MIN_ID_CHARS} or more characters of letters, numbers and \
         `{ID_PUNCTUATION_CHARACTERS}`"
    )
}

/// Refuse a member whose value is not an id this service could address a resource by,
/// keyed by the member path the wire names.
pub fn check_resource_id(member: &str, value: &str) -> Result<(), ServiceError> {
    if is_resource_id(value) {
        return Ok(());
    }
    Err(member_invalid(member, resource_id_requirement()))
}

/// Refuse the id a create names its new resource with — by SIZE, then by what an id may
/// say — before anything is stored.
///
/// The one function all three creates call, so the id a channel, an origin endpoint and a
/// harvest job may be stored under cannot drift apart: each of those ids is the http label
/// of its own resource's reads, so each has to satisfy the same rule.
pub fn check_created_id(value: &str) -> Result<(), ServiceError> {
    bound_chars(ID_MEMBER, value, MAX_STRING_CHARS)?;
    check_resource_id(ID_MEMBER, value)
}

/// Whether `c` is a character a tag key or a tag value may carry.
///
/// AWS's tag restrictions publish the set for every tagged resource: a key or a value
/// holds letters, numbers, spaces, and the characters
/// [`TAG_PUNCTUATION_CHARACTERS`] names. The letters-and-numbers half is UNICODE-wide
/// (the restrictions say "Unicode letters, digits"), which is what
/// [`char::is_alphanumeric`] answers — so an accented or non-Latin key is accepted and a
/// symbol outside the set is not.
fn is_tag_character(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ' ' | '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
}

/// The first character of `text` outside [`is_tag_character`]'s set, as the refusal that
/// names it — `None` when every character is one a tag may carry.
fn offending_tag_character(text: &str) -> Option<char> {
    text.chars().find(|c| !is_tag_character(*c))
}

/// Refuse a tag key a caller may not name — whatever it would do with it.
///
/// THREE rules, all AWS's universal tag restrictions rather than a modelled constraint:
/// the key is at most [`MAX_TAG_KEY_CHARS`] characters, it is built from
/// [`is_tag_character`]'s set, and it does not carry the reserved
/// [`RESERVED_TAG_KEY_PREFIX`]. This is the one place a tag KEY is judged, so every
/// operation that names one — a create's tag map, `TagResource`'s and `UntagResource`'s
/// key list — decides the same keys well-formed: a key no write path could ever have
/// stored is refused by the door that would DELETE it too, and a key carrying the
/// reserved prefix names a tag a caller may neither create, nor modify, nor delete.
///
/// `member` is the member the wire named, so one request is refused by the member it
/// actually carried: `tags` for a tag map, `tagKeys` for `UntagResource`'s key list.
pub fn validate_tag_key(member: &str, key: &str) -> Result<(), ServiceError> {
    bound_part_chars(member, &format!("key {key:?}"), key, MAX_TAG_KEY_CHARS)?;
    if let Some(character) = offending_tag_character(key) {
        return Err(member_invalid(
            member,
            format!(
                "key {key:?} carries {character:?}, which is not one of the letters, numbers, \
                 spaces and `{TAG_PUNCTUATION_CHARACTERS}` a tag key may use"
            ),
        ));
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Err(member_invalid(
            member,
            format!(
                "key {key:?} uses the reserved `{RESERVED_TAG_KEY_PREFIX}` prefix, which only \
                 Amazon Web Services may set"
            ),
        ));
    }
    Ok(())
}

/// Refuse a tag VALUE outside [`is_tag_character`]'s set; see [`validate_tag_key`], whose
/// character rule this is the other half of. Keyed by the key it belongs to, because a
/// map's value has no member path of its own.
fn validate_tag_value(member: &str, key: &str, value: &str) -> Result<(), ServiceError> {
    if let Some(character) = offending_tag_character(value) {
        return Err(member_invalid(
            member,
            format!(
                "the value of {key:?} carries {character:?}, which is not one of the letters, \
                 numbers, spaces and `{TAG_PUNCTUATION_CHARACTERS}` a tag value may use"
            ),
        ));
    }
    Ok(())
}

/// Refuse a tag set that would leave a resource carrying more than [`MAX_TAGS`] tags.
///
/// The half of [`bound_tag_entries`] that cannot be decided from a request alone: the
/// ceiling is about the set the resource is LEFT holding, so an operation that MERGES its
/// tags into a stored set judges it against that merged result — while what each tag SAYS,
/// which depends on no stored record, is judged from the request before any lookup.
pub fn bound_tag_count(count: usize) -> Result<(), ServiceError> {
    bound_entries("tags", count, MAX_TAGS)
}

/// Refuse a tag set that breaks AWS's tagging rules: by entry count, by the length of each
/// key and value, and by what each key and value is ALLOWED TO SAY — the character set,
/// and the reserved `aws:` key prefix only AWS itself may use.
///
/// This is the one place those rules are decided, so a create's request-shaped tag map
/// and a `TagResource`'s merged result cannot be judged differently. Entries are sorted
/// first, so the part named in the refusal does not depend on map ordering.
pub fn bound_tag_entries<'a, I>(tags: I) -> Result<(), ServiceError>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut entries: Vec<(&str, &str)> = tags.into_iter().collect();
    entries.sort_unstable();
    bound_tag_count(entries.len())?;
    for (key, value) in entries {
        // The key by the whole key rule — length, character set and reserved prefix —
        // through the one function every door that names a key goes through, then the
        // value by its own two.
        validate_tag_key("tags", key)?;
        bound_part_chars(
            "tags",
            &format!("the value of {key:?}"),
            value,
            MAX_TAG_VALUE_CHARS,
        )?;
        validate_tag_value("tags", key, value)?;
    }
    Ok(())
}

/// Refuse a tag map on an input that breaks AWS's tagging rules; see
/// [`bound_tag_entries`].
pub fn bound_tags(
    tags: &Option<std::collections::HashMap<String, String>>,
) -> Result<(), ServiceError> {
    let Some(tags) = tags else { return Ok(()) };
    bound_tag_entries(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

/// Refuse a caller-supplied list of strings that would grow a record past `max_entries`
/// entries, or past [`MAX_STRING_CHARS`] characters in any one entry.
///
/// A list lets a caller decide how much memory a record occupies in two ways — how many
/// entries it carries, and how long each entry is — so both are bounded, here, for every
/// such list of this service: one place, so no two lists are judged differently, and the
/// refusal names the member and which of the two bounds it broke.
fn bound_string_list(
    member: &str,
    entries: &[String],
    max_entries: usize,
) -> Result<(), ServiceError> {
    bound_entries(member, entries.len(), max_entries)?;
    for entry in entries {
        bound_part_chars(member, "an entry", entry, MAX_STRING_CHARS)?;
    }
    Ok(())
}

/// Refuse the caller-sized members an OriginEndpoint create and update share.
///
/// Both handlers call this, so the bounds one enforces cannot drift from the other's: an
/// update must not be able to grow a record past what the create would have refused. Each
/// refusal is keyed by the member path the wire names, as every other refused member is.
///
/// The members nested inside an `Authorization` or a packaging configuration are bounded by
/// [`bound_origin_endpoint_packaging_members`], which both handlers call beside this one.
pub fn bound_origin_endpoint_members(
    description: Option<&str>,
    manifest_name: Option<&str>,
    whitelist: Option<&[String]>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
) -> Result<(), ServiceError> {
    for (member, value) in [
        ("description", description),
        ("manifestName", manifest_name),
    ] {
        if let Some(value) = value {
            bound_chars(member, value, MAX_STRING_CHARS)?;
        }
    }
    if let Some(whitelist) = whitelist {
        bound_string_list(WHITELIST_MEMBER, whitelist, MAX_WHITELIST_ENTRIES)?;
    }
    if let Some(manifests) = cmaf_package.and_then(|package| package.hls_manifests.as_ref()) {
        bound_entries(
            "cmafPackage.hlsManifests",
            manifests.len(),
            MAX_CMAF_HLS_MANIFESTS,
        )?;
        for manifest in manifests {
            bound_part_chars(
                "cmafPackage.hlsManifests",
                "an id",
                &manifest.id,
                MAX_STRING_CHARS,
            )?;
            if let Some(name) = &manifest.manifest_name {
                bound_part_chars(
                    "cmafPackage.hlsManifests",
                    "a manifestName",
                    name,
                    MAX_STRING_CHARS,
                )?;
            }
        }
    }
    Ok(())
}

/// Refuse the caller-sized strings and lists INSIDE the `Authorization` and the packaging
/// configuration an OriginEndpoint create and update share.
///
/// The same rule [`bound_origin_endpoint_members`] applies to the request's top-level
/// members, applied at every depth a packaging configuration reaches: the store copies each
/// of these values verbatim onto the record and every later projection of the endpoint
/// echoes them, so one request must not be able to decide how much memory its record
/// occupies. Both handlers call this, before anything is written, so the bounds one
/// enforces cannot drift from the other's and a refused request persists nothing.
///
/// Every member judged here targets the unconstrained `__string` shape, or a list of one,
/// and the model publishes no `@length` for any of them
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization,com.amazonaws.mediapackage#SpekeKeyProvider,com.amazonaws.mediapackage#HlsEncryption,com.amazonaws.mediapackage#CmafEncryption,com.amazonaws.mediapackage#CmafPackageCreateOrUpdateParameters,com.amazonaws.mediapackage#DashPackage",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
/// so the framework's constraint walk cannot bound them. A member targeting an `enum` shape
/// is NOT judged here: its value set is closed, so the longest value it could carry is the
/// longest one the model declares. What a caller still decides about a LIST of such values
/// is how many entries it carries, so each of those lists is bounded by entry count.
///
/// Members are judged in a fixed order — the authorization block, then each packaging
/// block's own strings, then the trigger lists, then the SPEKE key provider — so one
/// request is always refused by the same member, and a member's PATH is built only when
/// that member is the one being refused.
pub fn bound_origin_endpoint_packaging_members(
    authorization: Option<&Authorization>,
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Result<(), ServiceError> {
    let hls_encryption = hls_package.and_then(|package| package.encryption.as_ref());
    let cmaf_encryption = cmaf_package.and_then(|package| package.encryption.as_ref());
    for (member, value) in [
        (
            "authorization.cdnIdentifierSecret",
            authorization.map(|block| block.cdn_identifier_secret.as_str()),
        ),
        (
            "authorization.secretsRoleArn",
            authorization.map(|block| block.secrets_role_arn.as_str()),
        ),
        (
            "hlsPackage.encryption.constantInitializationVector",
            hls_encryption.and_then(|encryption| {
                encryption.constant_initialization_vector.as_deref()
            }),
        ),
        (
            "cmafPackage.encryption.constantInitializationVector",
            cmaf_encryption.and_then(|encryption| {
                encryption.constant_initialization_vector.as_deref()
            }),
        ),
        (
            "cmafPackage.segmentPrefix",
            cmaf_package.and_then(|package| package.segment_prefix.as_deref()),
        ),
        (
            "dashPackage.utcTimingUri",
            dash_package.and_then(|package| package.utc_timing_uri.as_deref()),
        ),
    ] {
        if let Some(value) = value {
            bound_chars(member, value, MAX_STRING_CHARS)?;
        }
    }

    // The trigger lists a packaging block carries: every entry is a member of a modelled
    // enum, so what the caller decides is how MANY of them the record keeps.
    for (member, entries) in [
        (
            "hlsPackage.adTriggers",
            hls_package.and_then(|package| package.ad_triggers.as_deref()),
        ),
        (
            "dashPackage.adTriggers",
            dash_package.and_then(|package| package.ad_triggers.as_deref()),
        ),
        (
            "dashPackage.periodTriggers",
            dash_package.and_then(|package| package.period_triggers.as_deref()),
        ),
    ] {
        if let Some(entries) = entries {
            bound_string_list(member, entries, MAX_PACKAGE_LIST_ENTRIES)?;
        }
    }

    // A CMAF child manifest carries a trigger list of its own, under a path naming the
    // entry the way the wire indexes a list member. Its `id` and `manifestName` are bounded
    // with the manifest list itself, by [`bound_origin_endpoint_members`].
    for (index, manifest) in cmaf_package
        .and_then(|package| package.hls_manifests.as_ref())
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(entries) = manifest.ad_triggers.as_deref() {
            bound_string_list(
                &format!("cmafPackage.hlsManifests[{index}].adTriggers"),
                entries,
                MAX_PACKAGE_LIST_ENTRIES,
            )?;
        }
    }

    // The SPEKE key provider of whichever packaging block carries an encryption
    // configuration: the same four strings and one list under each block's own wire path,
    // read from the one walk of the blocks the ARN rules read ([`speke_key_providers`]).
    for (block, provider) in
        speke_key_providers(hls_package, dash_package, cmaf_package, mss_package)
    {
        let path = format!("{block}.encryption.spekeKeyProvider");
        for (member, value) in [
            ("resourceId", Some(provider.resource_id.as_str())),
            ("roleArn", Some(provider.role_arn.as_str())),
            ("url", Some(provider.url.as_str())),
            ("certificateArn", provider.certificate_arn.as_deref()),
        ] {
            if let Some(value) = value {
                bound_chars(&format!("{path}.{member}"), value, MAX_STRING_CHARS)?;
            }
        }
        bound_string_list(
            &format!("{path}.systemIds"),
            &provider.system_ids,
            MAX_PACKAGE_LIST_ENTRIES,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// What an endpoint's access-control and packaging members have to NAME
//
// The bounds above judge a member by SIZE; the rules below judge it by CONTENT. Each of
// these members names something OUTSIDE the request that the egress tier or the packager
// has to act on — an address range to admit, a secret to read, a role to assume, the
// manifests to publish — so a value this service could never act on is refused with
// `UnprocessableEntityException` (422) before anything is stored, instead of being kept
// and reported as live configuration that silently matches, reads or publishes nothing.
//
// The model targets every one of them at the bare `__string` (or at an unbounded list)
// and publishes no `@pattern` on any of them
// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization,com.amazonaws.mediapackage#SpekeKeyProvider,com.amazonaws.mediapackage#CmafPackageCreateOrUpdateParameters",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
// so the framework's constraint walk cannot refuse any of it: the rule is the service's
// own, enforced here for EVERY member of each class rather than for the one a client
// happened to get wrong, and by the one function both the create and the update call.
// ---------------------------------------------------------------------------

/// Whether `value` is an IP CIDR block: an IP address, `/`, and a prefix length no wider
/// than that address family allows (32 bits for IPv4, 128 for IPv6).
///
/// `Whitelist` is "a list of source IP CIDR blocks that will be allowed to access the
/// OriginEndpoint" (`smithy_query(query="operation",operation="CreateOriginEndpoint")`),
/// and that documentation names no address family, so an address of EITHER family names a
/// source range the egress tier could match and is admitted. Host bits are not required to
/// be zero: `10.0.0.1/32` and `10.0.0.1/8` each name a range that contains the address,
/// which is how AWS writes a single-host rule.
///
/// The address itself is parsed by `std::net::IpAddr`, so no hand-written address grammar
/// decides what an address is — which is what refuses an octet out of range, an address
/// with too few octets, and a prefix length that is not a number or is wider than the
/// family. A value carrying no `/` at all names one address rather than a range, so it is
/// not a CIDR block either, and the prefix length has to be DECIMAL DIGITS: a sign or a
/// leading zero is not how a CIDR block is written, and `u8::from_str` alone would admit
/// both.
pub fn is_cidr_block(value: &str) -> bool {
    let Some((address, prefix_length)) = value.split_once('/') else {
        return false;
    };
    let written_as_digits = !prefix_length.is_empty()
        && prefix_length.bytes().all(|byte| byte.is_ascii_digit())
        && (prefix_length == "0" || !prefix_length.starts_with('0'));
    if !written_as_digits {
        return false;
    }
    let Ok(address) = address.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix_length) = prefix_length.parse::<u8>() else {
        return false;
    };
    let bits = match address {
        std::net::IpAddr::V4(_) => 32,
        std::net::IpAddr::V6(_) => 128,
    };
    prefix_length <= bits
}

/// The member a refused `Whitelist` entry is reported against, as the wire names it.
pub const WHITELIST_MEMBER: &str = "whitelist";

/// Refuse a `Whitelist` carrying an entry that is not an address range at all.
///
/// An entry is an access-control rule the egress tier matches a request's source address
/// against, so a value that is not a CIDR block ([`is_cidr_block`]) could never admit
/// anybody: it is refused rather than stored as a rule that matches nothing. The refusal
/// is keyed by the member and NAMES the entry that failed, so the caller can fix the
/// request from the error alone; entries are judged in the request's own order, so one
/// request is always refused by the same entry.
pub fn check_whitelist(whitelist: Option<&[String]>) -> Result<(), ServiceError> {
    for entry in whitelist.into_iter().flatten() {
        if !is_cidr_block(entry) {
            return Err(member_invalid(
                WHITELIST_MEMBER,
                format!("must carry source IP CIDR blocks; {entry:?} is not one"),
            ));
        }
    }
    Ok(())
}

/// What a member DOCUMENTED as the ARN of a particular AWS resource has to name.
///
/// MediaPackage resolves such a member up front — it READS the secret and ASSUMES the role
/// the value names — so a value that is not an ARN of the documented service, or names
/// another kind of resource in it, names nothing this service could act on: the request is
/// a parameter violation (422, keyed by the member) rather than a resource that happens to
/// be missing.
///
/// The type, the rule and the constants below are the SHARED ones
/// ([`::service_common::arn::Requirement`]): every service that binds a caller-supplied
/// resource ARN reads it against the same requirement, so no two of them disagree about
/// what "the ARN of an IAM role" is. What stays here is which of THIS model's members are
/// ARNs ([`origin_endpoint_arn_members`]) and the envelope the refusal travels in.
pub type ArnRequirement = ::service_common::arn::Requirement;

/// An IAM role MediaPackage assumes: `Authorization$SecretsRoleArn` is "the ARN for the IAM
/// role that allows MediaPackage to communicate with AWS Secrets Manager",
/// `SpekeKeyProvider$RoleArn` "an ARN of an IAM role that AWS Elemental MediaPackage will
/// assume when accessing the key provider service", and `S3Destination$RoleArn` "the IAM
/// role used to write to the specified S3 bucket".
pub const IAM_ROLE_ARN: ArnRequirement = ::service_common::arn::IAM_ROLE;

/// A secret in AWS Secrets Manager: `Authorization$CdnIdentifierSecret` is "the ARN for the
/// secret in Secrets Manager that your Content Distribution Network (CDN) uses for
/// authorization to access your endpoint".
pub const SECRETS_MANAGER_SECRET_ARN: ArnRequirement =
    ::service_common::arn::SECRETS_MANAGER_SECRET;

/// A certificate in AWS Certificate Manager: `SpekeKeyProvider$CertificateArn` is "an ARN
/// of a Certificate Manager certificate that MediaPackage will use for enforcing secure
/// end-to-end data transfer with the key provider service".
pub const ACM_CERTIFICATE_ARN: ArnRequirement = ::service_common::arn::ACM_CERTIFICATE;

/// Refuse a member whose value is not the ARN its documentation requires, keyed by the
/// member path the wire names.
pub fn check_arn_member(
    member: &str,
    value: &str,
    requirement: ArnRequirement,
) -> Result<(), ServiceError> {
    if requirement.admits(value) {
        return Ok(());
    }
    Err(member_invalid(member, requirement.phrase()))
}

/// One ARN-documented member of a request: the wire path it travels under, the value it
/// carries, and the ARN it has to be.
type ArnMember<'a> = (String, &'a str, ArnRequirement);

/// The ARN-documented members of one packaging block's SPEKE key provider, under that
/// block's own wire path.
///
/// The provider sits at the same place under all four packaging formats, so the same two
/// requirements are read under each one's path rather than once per format.
fn speke_arn_members<'a>(block: &str, provider: &'a SpekeKeyProvider) -> Vec<ArnMember<'a>> {
    let path = format!("{block}.encryption.spekeKeyProvider");
    let mut members = vec![(
        format!("{path}.roleArn"),
        provider.role_arn.as_str(),
        IAM_ROLE_ARN,
    )];
    if let Some(certificate_arn) = provider.certificate_arn.as_deref() {
        members.push((
            format!("{path}.certificateArn"),
            certificate_arn,
            ACM_CERTIFICATE_ARN,
        ));
    }
    members
}

/// Every SPEKE key provider an OriginEndpoint create or update request carries, each under
/// the wire path of the packaging block that carries it.
///
/// The provider sits at the same place under all four packaging formats, so this is the ONE
/// walk of the four blocks: the ARN rules below and the size bounds above read the same set
/// in the same order, so neither can judge a provider the other does not see.
fn speke_key_providers<'a>(
    hls_package: Option<&'a HlsPackage>,
    dash_package: Option<&'a DashPackage>,
    cmaf_package: Option<&'a CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&'a MssPackage>,
) -> Vec<(&'static str, &'a SpekeKeyProvider)> {
    [
        hls_package
            .and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("hlsPackage", &encryption.speke_key_provider)),
        dash_package
            .and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("dashPackage", &encryption.speke_key_provider)),
        cmaf_package
            .and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("cmafPackage", &encryption.speke_key_provider)),
        mss_package
            .and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("mssPackage", &encryption.speke_key_provider)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// EVERY member of an OriginEndpoint create or update request that the model documents as
/// an ARN, with the requirement each one has to satisfy.
///
/// The complete set, from
/// `smithy_query(query="shape",shape="com.amazonaws.mediapackage#Authorization,com.amazonaws.mediapackage#SpekeKeyProvider",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`:
/// the two members of a CDN `Authorization` block, and the role and certificate of the
/// SPEKE key provider inside whichever packaging block carries an `Encryption`. Both the
/// create and the update read the same list, so neither can store an ARN the other would
/// have refused.
fn origin_endpoint_arn_members<'a>(
    authorization: Option<&'a Authorization>,
    hls_package: Option<&'a HlsPackage>,
    dash_package: Option<&'a DashPackage>,
    cmaf_package: Option<&'a CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&'a MssPackage>,
) -> Vec<ArnMember<'a>> {
    let mut members: Vec<ArnMember<'a>> = Vec::new();
    if let Some(authorization) = authorization {
        members.push((
            "authorization.cdnIdentifierSecret".to_string(),
            authorization.cdn_identifier_secret.as_str(),
            SECRETS_MANAGER_SECRET_ARN,
        ));
        members.push((
            "authorization.secretsRoleArn".to_string(),
            authorization.secrets_role_arn.as_str(),
            IAM_ROLE_ARN,
        ));
    }
    for (block, provider) in
        speke_key_providers(hls_package, dash_package, cmaf_package, mss_package)
    {
        members.extend(speke_arn_members(block, provider));
    }
    members
}

/// The fewest child HLS manifests a CMAF package may publish.
///
/// `CmafPackage$HlsManifests` is "a list of HLS manifest configurations" and an endpoint's
/// CMAF playback URL ends in one of those child manifests
/// ([`cmaf_manifest_url`]), so a package carrying none publishes nothing for the playback
/// URL to serve. One is therefore the minimum, and the model's own absence of a `@length`
/// on the list is why this is the service's rule rather than a modelled constraint.
pub const MIN_CMAF_HLS_MANIFESTS: usize = 1;

/// The member a refused CMAF manifest list is reported against, as the wire names it.
pub const CMAF_HLS_MANIFESTS_MEMBER: &str = "cmafPackage.hlsManifests";

/// The requirement a CMAF manifest list carrying `count` manifests failed: the minimum, in
/// the wording every other bounded collection here uses.
///
/// Built here rather than written at the one call site, so the sentence and the minimum
/// cannot disagree and the suite asserting it reads the service's own words. The noun agrees
/// with the minimum, so raising [`MIN_CMAF_HLS_MANIFESTS`] does not produce
/// "at least 2 manifest".
pub fn cmaf_manifests_requirement(count: usize) -> String {
    let manifests = if MIN_CMAF_HLS_MANIFESTS == 1 {
        "manifest"
    } else {
        "manifests"
    };
    format!("must carry at least {MIN_CMAF_HLS_MANIFESTS} {manifests}, got {count}")
}

/// Refuse a `CmafPackage` that publishes no child manifest at all — an empty
/// `HlsManifests` list, or none.
///
/// Judged only when the request names a CMAF package: a request that names another
/// packaging format carries no CMAF manifest list to be short.
pub fn require_cmaf_hls_manifests(
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
) -> Result<(), ServiceError> {
    let Some(package) = cmaf_package else {
        return Ok(());
    };
    let count = package.hls_manifests.as_ref().map_or(0, Vec::len);
    if count < MIN_CMAF_HLS_MANIFESTS {
        return Err(member_invalid(
            CMAF_HLS_MANIFESTS_MEMBER,
            cmaf_manifests_requirement(count),
        ));
    }
    Ok(())
}

/// The requirement a list of child configurations failed by naming the same child id more
/// than once: the rule, and how many entries shared the repeated id.
///
/// Built here rather than written at the call site, so the sentence and the rule cannot
/// disagree and the suite asserting it reads the service's own words.
pub fn unique_child_id_requirement(id: &str, count: usize) -> String {
    format!("may carry each id only once, got {count} entries with an id of {id:?}")
}

/// Refuse a caller-supplied list of child configurations that names the same child id more
/// than once, keyed by the member path the wire names.
///
/// A child id the model documents as unique within its parent resource is what a client
/// addresses that child by and what this service derives the child's own server-supplied
/// values from, so a list repeating an id names two children nothing can tell apart. The
/// one function every such list is judged by, so no list of children can be stored with a
/// repeated id while another is refused for it. The refusal names the FIRST id that
/// repeats, in the order the request sent them, so the same request is always answered the
/// same way.
pub fn require_unique_child_ids<'a, I>(member: &str, ids: I) -> Result<(), ServiceError>
where
    I: IntoIterator<Item = &'a str>,
{
    let ids: Vec<&str> = ids.into_iter().collect();
    let mut seen: HashSet<&str> = HashSet::with_capacity(ids.len());
    for id in &ids {
        if !seen.insert(id) {
            let count = ids.iter().filter(|other| *other == id).count();
            return Err(member_invalid(member, unique_child_id_requirement(id, count)));
        }
    }
    Ok(())
}

/// Refuse a `CmafPackage` that publishes two child manifests under one id.
///
/// `HlsManifestCreateOrUpdateParameters$Id` is "the ID of the manifest. The ID must be
/// unique within the OriginEndpoint", so the child ids of one package are a SET; judged by
/// [`require_unique_child_ids`] like every other documented-unique child id, and only when
/// the request names a CMAF package at all.
pub fn require_unique_cmaf_hls_manifest_ids(
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
) -> Result<(), ServiceError> {
    let Some(package) = cmaf_package else {
        return Ok(());
    };
    require_unique_child_ids(
        CMAF_HLS_MANIFESTS_MEMBER,
        package
            .hls_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.id.as_str()),
    )
}

// ---------------------------------------------------------------------------
// Members that are only acceptable TOGETHER with a particular value of another member
//
// A few packaging members state a requirement on ANOTHER member of the same
// configuration: selecting one option of an enum requires a second member to carry a value
// in a narrower domain, and specifying one block requires a second member to carry a
// particular value. The model states each of these in the MEMBER'S OWN prose rather than as
// a constraint, so neither the framework's constraint walk nor any per-member rule above
// can refuse the pair — the combination has to be judged as a pair, which is what the
// functions below do.
//
// Each pair is judged for EVERY packaging block that can carry it, by the one function both
// the create and the update call, so a combination refused under one block or one operation
// cannot be stored under another. The refusal is this service's usual
// `UnprocessableEntityException` (422), keyed by the member the caller has to change, and
// nothing is stored.
// ---------------------------------------------------------------------------

/// The `AdMarkers` option that signals ad and program transitions with EXT-X-DATERANGE
/// tags — one of the values the member's enum admits
/// (`crate::ops::types::AD_MARKERS_VALUES`), and the only one whose documentation states a
/// requirement on another member.
pub const DATERANGE_AD_MARKERS: &str = "DATERANGE";

/// The value a `ProgramDateTimeIntervalSeconds` has to EXCEED in a manifest configuration
/// that selects [`DATERANGE_AD_MARKERS`].
///
/// `HlsPackage$AdMarkers` and `HlsManifestCreateOrUpdateParameters$AdMarkers` carry the
/// same sentence — for the DATERANGE option "you must set a
/// programDateTimeIntervalSeconds value that is greater than 0"
/// (`smithy_query(query="doc",shape="com.amazonaws.mediapackage#HlsPackage",member="AdMarkers",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`)
/// — because the EXT-X-DATERANGE tags that option writes are positioned by the
/// EXT-X-PROGRAM-DATE-TIME tags that interval inserts, and an interval of zero inserts
/// none at all.
pub const DATERANGE_PROGRAM_DATE_TIME_INTERVAL_FLOOR: i32 = 0;

/// The requirement a manifest configuration selecting [`DATERANGE_AD_MARKERS`] failed: the
/// value its interval has to exceed and the ad-marker option that requires it, in the
/// wording every other refused numeric member here uses.
///
/// Built here rather than written at the call site, so the sentence and the floor it states
/// cannot disagree and the suite asserting it reads the service's own words.
pub fn daterange_interval_requirement() -> String {
    format!(
        "must be greater than {DATERANGE_PROGRAM_DATE_TIME_INTERVAL_FLOOR} when adMarkers is \
         {DATERANGE_AD_MARKERS}"
    )
}

/// Refuse ONE manifest configuration that selects [`DATERANGE_AD_MARKERS`] without a
/// date-time interval above [`DATERANGE_PROGRAM_DATE_TIME_INTERVAL_FLOOR`], keyed by the
/// interval member under `prefix` — the member the caller has to change.
///
/// An interval the request OMITS is refused on the same footing as an explicit zero,
/// because an omitted one is stored and reported as
/// [`DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS`]: what the service would keep for such a
/// request is exactly the pair the member's documentation forbids. `prefix` is anything
/// that renders itself, so a caller with a list index passes `format_args!`, and the path
/// costs nothing until this is the member being refused.
fn check_daterange_interval(
    prefix: impl std::fmt::Display,
    ad_markers: Option<&str>,
    program_date_time_interval_seconds: Option<i32>,
) -> Result<(), ServiceError> {
    if ad_markers != Some(DATERANGE_AD_MARKERS) {
        return Ok(());
    }
    let interval =
        program_date_time_interval_seconds.unwrap_or(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS);
    if interval > DATERANGE_PROGRAM_DATE_TIME_INTERVAL_FLOOR {
        return Ok(());
    }
    Err(member_invalid(
        &format!("{prefix}programDateTimeIntervalSeconds"),
        daterange_interval_requirement(),
    ))
}

/// Refuse an OriginEndpoint create or update whose HLS manifest configuration selects
/// DATERANGE ad markers without the date-time interval that option requires.
///
/// Judged on EVERY manifest configuration the request carries: the `HlsPackage` and each
/// child manifest of a `CmafPackage` are the complete set of members targeting `AdMarkers`
/// on this operation's input (a DASH or MSS package has no such member), and they are
/// judged in the request's own member order, so one request is always refused by the same
/// member.
pub fn check_origin_endpoint_ad_markers(
    hls_package: Option<&HlsPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
) -> Result<(), ServiceError> {
    if let Some(package) = hls_package {
        check_daterange_interval(
            "hlsPackage.",
            package.ad_markers.as_deref(),
            package.program_date_time_interval_seconds,
        )?;
    }
    if let Some(package) = cmaf_package {
        for (index, manifest) in package.hls_manifests.iter().flatten().enumerate() {
            check_daterange_interval(
                format_args!("cmafPackage.hlsManifests[{index}]."),
                manifest.ad_markers.as_deref(),
                manifest.program_date_time_interval_seconds,
            )?;
        }
    }
    Ok(())
}

/// The `KeyRotationIntervalSeconds` value that DISABLES key rotation, which is what an
/// endpoint carrying a SPEKE 2.0 encryption contract has to ask for.
pub const DISABLED_KEY_ROTATION_INTERVAL_SECONDS: i32 = 0;

/// The wire path of a packaging block's key-rotation member, under that block.
const KEY_ROTATION_MEMBER: &str = "encryption.keyRotationIntervalSeconds";

/// The wire path of a packaging block's SPEKE 2.0 encryption contract, under that block.
const ENCRYPTION_CONTRACT_MEMBER: &str =
    "encryption.spekeKeyProvider.encryptionContractConfiguration";

/// The requirement a key-rotation interval failed by being asked for beside a SPEKE 2.0
/// encryption contract.
///
/// `EncryptionContractConfiguration`'s documentation states "You must disable key rotation
/// for this endpoint by setting keyRotationIntervalSeconds to 0"
/// (`smithy_query(query="doc",shape="com.amazonaws.mediapackage#EncryptionContractConfiguration",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
/// so the contract and a live rotation cannot be stored together: the keys the contract
/// names are the keys the endpoint encrypts with, and a rotation would replace them.
pub fn disabled_key_rotation_requirement() -> String {
    format!(
        "must be {DISABLED_KEY_ROTATION_INTERVAL_SECONDS} when \
         encryptionContractConfiguration is specified"
    )
}

/// The requirement a SPEKE 2.0 encryption contract failed by being specified on a
/// packaging format that does not use SPEKE 2.0.
///
/// The same documentation states that the block "can be used for DASH or CMAF endpoints
/// that use SPEKE 2.0": the contract names the audio and video keys a CPIX 2.3 key request
/// asks for, which is a request only those two formats' packagers make, so an HLS or MSS
/// endpoint has nothing to act on it with. The member exists on every format's provider
/// only because all four share the one `SpekeKeyProvider` shape.
pub fn encryption_contract_packaging_requirement() -> String {
    "may be specified only for a DASH or CMAF endpoint that uses SPEKE 2.0".to_string()
}

/// One packaging block's encryption, flattened to what the encryption-contract rules read:
/// which block it is, whether its SPEKE key provider names a 2.0 encryption contract, the
/// key rotation the block asks for, and whether the block's format uses SPEKE 2.0 at all.
///
/// Each format declares its own `Encryption` shape — and only three of the four carry a
/// key-rotation member (`MssEncryption` has none, so an MSS endpoint can never ask for a
/// rotation) — so the four are read into this one view and judged by the one rule, rather
/// than each format's block being judged on its own.
#[derive(Debug)]
struct EncryptionBlock {
    /// The wire name of the packaging block, which a refusal's member path starts with.
    block: &'static str,
    /// Whether the block's SPEKE key provider carries an `EncryptionContractConfiguration`.
    encryption_contract: bool,
    /// The `KeyRotationIntervalSeconds` the block asks for, or `None` when it asks for no
    /// rotation — either because the request omitted the member or because the format has
    /// none.
    key_rotation_interval_seconds: Option<i32>,
    /// Whether this block's packaging format is one SPEKE 2.0 serves; see
    /// [`encryption_contract_packaging_requirement`].
    speke_2_0: bool,
}

/// The encryption block of every packaging format an OriginEndpoint create or update
/// request carries, in the request's own member order.
///
/// The one walk of the four formats' encryption blocks, so no rule below can judge a block
/// another does not see. A format the request does not name, and one named without an
/// `Encryption`, carries no block.
fn origin_endpoint_encryption_blocks(
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Vec<EncryptionBlock> {
    let mut blocks: Vec<EncryptionBlock> = Vec::new();
    if let Some(encryption) = hls_package.and_then(|package| package.encryption.as_ref()) {
        blocks.push(EncryptionBlock {
            block: "hlsPackage",
            encryption_contract: has_encryption_contract(&encryption.speke_key_provider),
            key_rotation_interval_seconds: encryption.key_rotation_interval_seconds,
            speke_2_0: false,
        });
    }
    if let Some(encryption) = dash_package.and_then(|package| package.encryption.as_ref()) {
        blocks.push(EncryptionBlock {
            block: "dashPackage",
            encryption_contract: has_encryption_contract(&encryption.speke_key_provider),
            key_rotation_interval_seconds: encryption.key_rotation_interval_seconds,
            speke_2_0: true,
        });
    }
    if let Some(encryption) = cmaf_package.and_then(|package| package.encryption.as_ref()) {
        blocks.push(EncryptionBlock {
            block: "cmafPackage",
            encryption_contract: has_encryption_contract(&encryption.speke_key_provider),
            key_rotation_interval_seconds: encryption.key_rotation_interval_seconds,
            speke_2_0: true,
        });
    }
    if let Some(encryption) = mss_package.and_then(|package| package.encryption.as_ref()) {
        blocks.push(EncryptionBlock {
            block: "mssPackage",
            encryption_contract: has_encryption_contract(&encryption.speke_key_provider),
            // `MssEncryption` declares no key-rotation member at all.
            key_rotation_interval_seconds: None,
            speke_2_0: false,
        });
    }
    blocks
}

/// Whether `provider` names a SPEKE 2.0 encryption contract.
fn has_encryption_contract(provider: &SpekeKeyProvider) -> bool {
    provider.encryption_contract_configuration.is_some()
}

/// Refuse an OriginEndpoint create or update whose SPEKE 2.0 encryption contract is asked
/// for in a configuration that cannot serve it: on a packaging format that does not use
/// SPEKE 2.0, or beside key rotation the same block leaves enabled.
///
/// Every encryption block the request carries is judged
/// ([`origin_endpoint_encryption_blocks`]), in the request's own member order, so one
/// request is always refused by the same member. A block carrying no contract has nothing
/// to judge: the key-rotation interval it asks for is bounded by the shared duration floor
/// alone.
pub fn check_origin_endpoint_encryption_contracts(
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Result<(), ServiceError> {
    for block in
        origin_endpoint_encryption_blocks(hls_package, dash_package, cmaf_package, mss_package)
    {
        if !block.encryption_contract {
            continue;
        }
        if !block.speke_2_0 {
            return Err(member_invalid(
                &format!("{}.{ENCRYPTION_CONTRACT_MEMBER}", block.block),
                encryption_contract_packaging_requirement(),
            ));
        }
        if block
            .key_rotation_interval_seconds
            .is_some_and(|seconds| seconds != DISABLED_KEY_ROTATION_INTERVAL_SECONDS)
        {
            return Err(member_invalid(
                &format!("{}.{KEY_ROTATION_MEMBER}", block.block),
                disabled_key_rotation_requirement(),
            ));
        }
    }
    Ok(())
}

/// Refuse an OriginEndpoint create or update whose access-control or packaging members name
/// something this service could not act on, or combine two members the service does not
/// serve together.
///
/// Both handlers call this, so the rules one enforces cannot drift from the other's: an
/// update must not be able to store a whitelist entry, an ARN or a packaging block a create
/// of it would have refused. Nothing here reads state, so a refused request is refused
/// before anything is written.
pub fn check_origin_endpoint_members(
    whitelist: Option<&[String]>,
    authorization: Option<&Authorization>,
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Result<(), ServiceError> {
    check_whitelist(whitelist)?;
    for (member, value, requirement) in origin_endpoint_arn_members(
        authorization,
        hls_package,
        dash_package,
        cmaf_package,
        mss_package,
    ) {
        check_arn_member(&member, value, requirement)?;
    }
    require_cmaf_hls_manifests(cmaf_package)?;
    require_unique_cmaf_hls_manifest_ids(cmaf_package)?;
    check_origin_endpoint_ad_markers(hls_package, cmaf_package)?;
    check_origin_endpoint_encryption_contracts(hls_package, dash_package, cmaf_package, mss_package)
}

/// Every IAM role an OriginEndpoint create or update asks MediaPackage to ASSUME, by the
/// wire path of the member naming it.
///
/// The CDN authorization secrets role (MediaPackage reads the CDN secret as it) and the
/// SPEKE key provider's role (MediaPackage reaches the key provider as it) — the subset of
/// [`origin_endpoint_arn_members`] whose requirement is [`IAM_ROLE_ARN`], so a role member
/// added to the list above is pre-flighted here without being named twice. Owned values, so
/// the caller can hold them across taking the IAM store's guard.
pub fn origin_endpoint_assumed_roles(
    authorization: Option<&Authorization>,
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Vec<(String, String)> {
    origin_endpoint_arn_members(
        authorization,
        hls_package,
        dash_package,
        cmaf_package,
        mss_package,
    )
    .into_iter()
    .filter(|(_, _, requirement)| *requirement == IAM_ROLE_ARN)
    .map(|(member, value, _)| (member, value.to_string()))
    .collect()
}

// ---------------------------------------------------------------------------
// What a log-group member has to NAME
//
// A member that names a CloudWatch Logs log group is an instruction to WRITE access logs
// into that group, so the value has to be a name CloudWatch Logs can actually hold: the
// empty string, a name past CloudWatch's ceiling and a name carrying a character outside
// its set each name no group this service could ever write to. Such a request is refused
// with `UnprocessableEntityException` (422, keyed by the member) before anything is
// stored, instead of being kept and reported by every later read as a live log
// subscription that collects nothing.
//
// The model targets the member at the bare `__string` and publishes no `@length` or
// `@pattern`
// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#IngressAccessLogs,com.amazonaws.mediapackage#EgressAccessLogs",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`
// reports a single `LogGroupName` member on each, documented as "Customize the log group
// name"), so the framework's constraint walk cannot refuse any of it: the rule is the
// service's own, enforced here for EVERY member of the class by the one function.
// ---------------------------------------------------------------------------

/// The shortest log group name CloudWatch Logs holds — so the empty string names no group.
///
/// CloudWatch Logs' own log-group-name rule: a name is 1 to
/// [`MAX_LOG_GROUP_NAME_CHARS`] characters long.
pub const MIN_LOG_GROUP_NAME_CHARS: usize = 1;

/// The longest log group name CloudWatch Logs holds; see [`MIN_LOG_GROUP_NAME_CHARS`].
///
/// This is the ceiling of the service MediaPackage writes into, NOT this emulator's
/// generic string guard ([`MAX_STRING_CHARS`]): a longer name is one CloudWatch Logs
/// would never accept, whatever memory it would occupy here.
pub const MAX_LOG_GROUP_NAME_CHARS: usize = 512;

/// The non-alphanumeric characters a log group name may carry, as the refusal names them.
const LOG_GROUP_PUNCTUATION_CHARACTERS: &str = ". - _ / #";

/// Whether `c` is a character a CloudWatch Logs log group name may carry.
///
/// CloudWatch Logs' documented set: letters, digits, and the characters
/// [`LOG_GROUP_PUNCTUATION_CHARACTERS`] names. The letters-and-digits half is ASCII-ONLY
/// here — unlike a tag's ([`is_tag_character`]), whose restrictions say "Unicode letters,
/// digits" — because the log-group rule publishes the set as `a-z`, `A-Z` and `0-9`, so an
/// accented or non-Latin name is not one CloudWatch Logs holds.
fn is_log_group_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '#')
}

/// Whether `value` is a name CloudWatch Logs can hold: between
/// [`MIN_LOG_GROUP_NAME_CHARS`] and [`MAX_LOG_GROUP_NAME_CHARS`] characters, every one of
/// them in [`is_log_group_character`]'s set.
pub fn is_log_group_name(value: &str) -> bool {
    let length = value.chars().count();
    (MIN_LOG_GROUP_NAME_CHARS..=MAX_LOG_GROUP_NAME_CHARS).contains(&length)
        && value.chars().all(is_log_group_character)
}

/// The requirement a refused log group name failed: the member's WHOLE accepted domain, in
/// one sentence, so a name refused for its length and one refused for a character it
/// carries are answered with the same rule.
///
/// Built here rather than written at each call site, so the sentence and the bounds it
/// states cannot disagree and the suite asserting it reads the service's own words.
pub fn log_group_name_requirement() -> String {
    format!(
        "must be {MIN_LOG_GROUP_NAME_CHARS} to {MAX_LOG_GROUP_NAME_CHARS} characters of \
         letters, numbers and `{LOG_GROUP_PUNCTUATION_CHARACTERS}`"
    )
}

/// Refuse a member whose value is not a log group name CloudWatch Logs could hold, keyed by
/// the member path the wire names.
pub fn check_log_group_name(member: &str, value: &str) -> Result<(), ServiceError> {
    if is_log_group_name(value) {
        return Ok(());
    }
    Err(member_invalid(member, log_group_name_requirement()))
}

/// The wire path of the ingress access-log group member, which its refusal is keyed by
/// (`IngressAccessLogs` is `@jsonName("ingressAccessLogs")` and carries `LogGroupName` as
/// `@jsonName("logGroupName")`).
pub const INGRESS_LOG_GROUP_MEMBER: &str = "ingressAccessLogs.logGroupName";

/// The wire path of the egress access-log group member; see [`INGRESS_LOG_GROUP_MEMBER`].
pub const EGRESS_LOG_GROUP_MEMBER: &str = "egressAccessLogs.logGroupName";

/// Refuse a channel's access-log subscription when either direction names a log group
/// CloudWatch Logs could not hold.
///
/// The two members carry the same kind of value, so both are judged by the one rule, in the
/// request's own member order — ingress then egress — so one request is always refused by
/// the same member. A direction the request leaves out, or sends without a `LogGroupName`,
/// names no group and is nothing to judge: it unsubscribes that direction.
pub fn check_access_log_group_names(
    ingress: Option<&str>,
    egress: Option<&str>,
) -> Result<(), ServiceError> {
    for (member, value) in [
        (INGRESS_LOG_GROUP_MEMBER, ingress),
        (EGRESS_LOG_GROUP_MEMBER, egress),
    ] {
        if let Some(value) = value {
            check_log_group_name(member, value)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Durations in seconds
//
// Several members of an OriginEndpoint are a NUMBER OF SECONDS: a window of content to
// retain or to publish, a segment or buffer length, a delay, an interval between
// rotations or tags. Each is a count of seconds of media, so none of them can be
// negative — a window of minus one hundred seconds is not a window — and MediaPackage
// refuses a request carrying one with `UnprocessableEntityException` (422), storing
// nothing. The model targets every one of them at the plain `__integer` shape and
// declares no `@range`
// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#HlsPackage",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
// so the floor is not a modelled constraint the framework's own walk could refuse: it is
// the service's rule, enforced here for EVERY such member rather than for the one a
// client happened to get wrong.
//
// A member for which the service publishes a minimum of its OWN is bounded by that
// minimum instead of by the shared floor, so the accepted domain of each member is the
// one MediaPackage accepts: `StartoverWindowSeconds` is the one such member
// ([`bound_startover_window`], minimum [`MIN_ENABLED_STARTOVER_WINDOW_SECONDS`]).
// ---------------------------------------------------------------------------

/// The smallest value a duration-in-seconds member with no published minimum of its own
/// may carry. Zero is a real value for each of them — `TimeDelaySeconds` 0 is no delay,
/// which is what [`apply_origin_endpoint_defaults`] fills an absent one with — so the
/// floor is zero and not one.
pub const MIN_DURATION_SECONDS: i32 = 0;

/// The refusal a negative duration-in-seconds member answers with: the floor it broke,
/// keyed by that member, in the wording every other refused numeric member here uses.
fn duration_refusal(member: &str) -> ServiceError {
    member_invalid(
        member,
        format!("must be greater than or equal to {MIN_DURATION_SECONDS}"),
    )
}

/// The wire name of the startover-window member, which its refusal is keyed by
/// (`StartoverWindowSeconds` is `@jsonName("startoverWindowSeconds")` —
/// `smithy_query(query="operation",operation="CreateOriginEndpoint")`).
pub const STARTOVER_WINDOW_MEMBER: &str = "startoverWindowSeconds";

/// Refuse a `StartoverWindowSeconds` MediaPackage will not store.
///
/// The member is the "maximum duration (seconds) of content to retain for startover
/// playback", and unlike the other durations above it has a published minimum of its own:
/// an ENABLED startover window must be at least
/// [`MIN_ENABLED_STARTOVER_WINDOW_SECONDS`] seconds, so the whole span 1..299 — which the
/// shared floor would admit — is refused with `UnprocessableEntityException` (422) stating
/// that minimum (recorded 2026-09-20, us-west-2, for a negative value).
///
/// [`DEFAULT_STARTOVER_WINDOW_SECONDS`] (zero) is the one smaller value accepted, because
/// it is not a window at all: the model documents an unspecified member as DISABLING
/// startover playback, and that is the value this service fills in and renders back for an
/// absent one — so a caller round-tripping an endpoint it read must be able to send it
/// again. Every other value below the minimum is refused, whatever its sign, with the one
/// requirement sentence built from the member and that minimum.
pub fn bound_startover_window(seconds: Option<i32>) -> Result<(), ServiceError> {
    let accepted = seconds.is_none_or(|seconds| {
        seconds == DEFAULT_STARTOVER_WINDOW_SECONDS
            || seconds >= MIN_ENABLED_STARTOVER_WINDOW_SECONDS
    });
    if accepted {
        return Ok(());
    }
    Err(member_invalid(
        STARTOVER_WINDOW_MEMBER,
        format!("must be greater than or equal to {MIN_ENABLED_STARTOVER_WINDOW_SECONDS}"),
    ))
}

/// Refuse a negative value in any duration-in-seconds member of an OriginEndpoint create
/// or update request, keyed by the member path the wire names.
///
/// Both handlers call this, so the floor one enforces cannot drift from the other's: an
/// update must not be able to store a window a create of it would have refused. Members
/// are judged in a fixed order (the request's own member order, outermost first), so one
/// request is always refused by the same member — and a member's PATH is built only when
/// that member is the one being refused, so an accepted request allocates nothing.
///
/// `StartoverWindowSeconds` is judged by [`bound_startover_window`] instead of by the
/// shared floor: it is the one member of the set whose own minimum the service publishes.
pub fn bound_origin_endpoint_durations(
    startover_window_seconds: Option<i32>,
    time_delay_seconds: Option<i32>,
    hls_package: Option<&HlsPackage>,
    dash_package: Option<&DashPackage>,
    cmaf_package: Option<&CmafPackageCreateOrUpdateParameters>,
    mss_package: Option<&MssPackage>,
) -> Result<(), ServiceError> {
    /// One group of members sharing a path PREFIX: refuse the first one below the floor,
    /// keyed by the whole path the wire names it by (`<prefix><member>`).
    ///
    /// This is the one place the floor is compared and the one place its refusal is built,
    /// for every group — including a group nested in a LIST, whose prefix carries the
    /// entry's index: `prefix` is anything that renders itself, so a caller with an index
    /// passes `format_args!` and the prefix, like the path, costs nothing until a member
    /// of that group is the one refused.
    fn judge(
        prefix: impl std::fmt::Display,
        members: &[(&str, Option<i32>)],
    ) -> Result<(), ServiceError> {
        for (member, seconds) in members {
            if seconds.is_some_and(|seconds| seconds < MIN_DURATION_SECONDS) {
                return Err(duration_refusal(&format!("{prefix}{member}")));
            }
        }
        Ok(())
    }

    bound_startover_window(startover_window_seconds)?;
    judge("", &[("timeDelaySeconds", time_delay_seconds)])?;
    if let Some(package) = hls_package {
        judge(
            "hlsPackage.",
            &[
                ("playlistWindowSeconds", package.playlist_window_seconds),
                (
                    "programDateTimeIntervalSeconds",
                    package.program_date_time_interval_seconds,
                ),
                ("segmentDurationSeconds", package.segment_duration_seconds),
                (
                    "encryption.keyRotationIntervalSeconds",
                    package
                        .encryption
                        .as_ref()
                        .and_then(|encryption| encryption.key_rotation_interval_seconds),
                ),
            ],
        )?;
    }
    if let Some(package) = dash_package {
        judge(
            "dashPackage.",
            &[
                ("manifestWindowSeconds", package.manifest_window_seconds),
                ("minBufferTimeSeconds", package.min_buffer_time_seconds),
                ("minUpdatePeriodSeconds", package.min_update_period_seconds),
                ("segmentDurationSeconds", package.segment_duration_seconds),
                (
                    "suggestedPresentationDelaySeconds",
                    package.suggested_presentation_delay_seconds,
                ),
                (
                    "encryption.keyRotationIntervalSeconds",
                    package
                        .encryption
                        .as_ref()
                        .and_then(|encryption| encryption.key_rotation_interval_seconds),
                ),
            ],
        )?;
    }
    if let Some(package) = cmaf_package {
        judge(
            "cmafPackage.",
            &[
                ("segmentDurationSeconds", package.segment_duration_seconds),
                (
                    "encryption.keyRotationIntervalSeconds",
                    package
                        .encryption
                        .as_ref()
                        .and_then(|encryption| encryption.key_rotation_interval_seconds),
                ),
            ],
        )?;
        // A CMAF package's child HLS manifests carry the same two window members an
        // `HlsPackage` does, so the same floor holds for each of them — judged by the same
        // helper, under a prefix naming the entry the way the wire indexes a list member.
        for (index, manifest) in package.hls_manifests.iter().flatten().enumerate() {
            judge(
                format_args!("cmafPackage.hlsManifests[{index}]."),
                &[
                    ("playlistWindowSeconds", manifest.playlist_window_seconds),
                    (
                        "programDateTimeIntervalSeconds",
                        manifest.program_date_time_interval_seconds,
                    ),
                ],
            )?;
        }
    }
    if let Some(package) = mss_package {
        judge(
            "mssPackage.",
            &[
                ("manifestWindowSeconds", package.manifest_window_seconds),
                ("segmentDurationSeconds", package.segment_duration_seconds),
            ],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Server-filled origin-endpoint defaults
//
// MediaPackage does not store an origin endpoint as the request wrote it: every optional
// configuration member the request leaves unset is filled with that member's service
// default, stored, and returned on EVERY projection of the endpoint (the create response,
// `DescribeOriginEndpoint`, each entry of `ListOriginEndpoints`). The model declares no
// `@default` on any of them, so each value below carries its own source:
//
// * RECORDED for this service (trace set `simparity-wire-aws-v1`, fixtures
//   `mediapackage-origin-endpoint-lifecycle`, `mediapackage-conflict-error-paths`,
//   `mediapackage-harvest-job-workflow`): `origination` ALLOW, `startoverWindowSeconds`
//   and `timeDelaySeconds` 0, and in `hlsPackage` `adMarkers` NONE, the five-entry
//   `adTriggers` list, `adsOnDeliveryRestrictions` RESTRICTED, `playlistType` EVENT,
//   `playlistWindowSeconds` 60, `programDateTimeIntervalSeconds` 0 and the three booleans
//   `includeIframeOnlyStream` / `useAudioRenditionGroup` / `includeDvbSubtitles` false.
// * RECORDED for the sibling MediaPackage VOD service, whose packaging shapes carry the
//   same member names and defaults (same trace set, fixture
//   `mediapackage-vod-packaging-configuration-multi-format`): a CMAF child `HlsManifest`
//   takes `adMarkers` NONE, `programDateTimeIntervalSeconds` 0 and
//   `includeIframeOnlyStream` false, and a DASH package takes `profile` NONE,
//   `minBufferTimeSeconds` 30, `segmentTemplateFormat` NUMBER_WITH_TIMELINE,
//   `periodTriggers` [] and `includeIframeOnlyStream` false.
// * PUBLISHED by MediaPackage as the DASH packaging configuration a live endpoint is
//   created with, which the VOD recording above cannot speak to because the VOD packaging
//   shape has no live-manifest members at all: in `DashPackage` `manifestLayout` FULL,
//   `utcTiming` NONE, `manifestWindowSeconds` 60, `minUpdatePeriodSeconds` 15,
//   `suggestedPresentationDelaySeconds` 25 and `segmentDurationSeconds` 2 — the live
//   siblings of the same block's already-filled `minBufferTimeSeconds` and `profile`.
//   Corroboration for that source rather than for each value alone: the four DASH defaults
//   the VOD recording DOES carry (`minBufferTimeSeconds` 30, `profile` NONE,
//   `segmentTemplateFormat` NUMBER_WITH_TIMELINE, `periodTriggers` []) are the entries of
//   the same published table, so the entries only a live endpoint has are read from it too
//   rather than being left absent. `tests/all/dash_live_manifest_defaults.rs` pins all six
//   values on the wire, so a change to any of them is a test failure and not a silent drift.
//
// * PUBLISHED by MediaPackage as the stream selection every packaging block it stores is
//   created with: `streamOrder` ORIGINAL (the source order is kept), with no bitrate floor
//   or ceiling, reported as `minVideoBitsPerSecond` 0 and `maxVideoBitsPerSecond` the
//   largest value the modelled `__integer` holds. `StreamSelection` is a member of all four
//   packaging shapes, so the block is materialised on each of them.
//
// The defaults are keyed by MEMBER, so the same member takes the same default wherever a
// published default covers it (`HlsPackage`, a CMAF child `HlsManifest`, `DashPackage`,
// `MssPackage`); where a default belongs to one packaging format it is named for that
// format. A member whose default neither the model, a recording nor the service's published
// configuration publishes is left exactly as the request sent it —
// `HlsPackage$SegmentDurationSeconds`, the `MssPackage` window and segment members, and
// every encryption member except the one named below — because inventing a value for it
// would be a fabrication, not a default.
//
// Separately from the table above: where the MODEL states that the service itself CREATES a
// member's value when the request omits it, that member is not a default but a value
// MediaPackage supplies, so it is materialised at write time like the ARN and the playback
// URLs ([`created_initialization_vector`], called from [`stored_cmaf_package`]).
// ---------------------------------------------------------------------------

/// `Origination`'s default: a new endpoint may be requested unless the caller denies it.
pub const DEFAULT_ORIGINATION: &str = "ALLOW";

/// The `Origination` value that switches origination OFF for an endpoint — "control
/// whether origination of video is permitted for this OriginEndpoint … can be helpful for
/// temporarily disabling origination" — which is what the playback plane refuses while it
/// is stored ([`data_plane::permits_origination`]).
pub const ORIGINATION_DENY: &str = "DENY";

/// `StartoverWindowSeconds`' default. The model documents "If not specified, startover
/// playback will be disabled for the OriginEndpoint" — a window of zero seconds.
pub const DEFAULT_STARTOVER_WINDOW_SECONDS: i32 = 0;

/// The smallest `StartoverWindowSeconds` with startover playback ENABLED, and so the
/// smallest value a request may carry for it beyond the "disabled" zero.
///
/// Five minutes. The member is the "maximum duration (seconds) of content to retain for
/// startover playback" and the model declares no `@range` on it
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#CreateOriginEndpointRequest",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`
/// reports the plain `__integer`), so the bound is the service's own: a recorded
/// `CreateOriginEndpoint` refuses an out-of-range window with
/// `UnprocessableEntityException` stating "startoverWindowSeconds must be greater than or
/// equal to 300" (trace `mediapackage.origin-endpoint-startover-window-negative-rejected`,
/// measured 2026-09-20 in us-west-2).
///
/// This is both the bound [`bound_startover_window`] refuses below and the bound
/// [`OriginEndpointRecord::startover_enabled`] reads, so no stored endpoint can carry a
/// window the service would not have accepted, and nothing else decides what "startover
/// enabled" means for this service.
pub const MIN_ENABLED_STARTOVER_WINDOW_SECONDS: i32 = 300;

/// `TimeDelaySeconds`' default. The model documents "If not specified, there will be no
/// time delay in effect for the OriginEndpoint" — a delay of zero seconds.
pub const DEFAULT_TIME_DELAY_SECONDS: i32 = 0;

/// `AdMarkers`' default: no SCTE-35 ad markers are written into the manifest.
pub const DEFAULT_AD_MARKERS: &str = "NONE";

/// `AdsOnDeliveryRestrictions`' default: only SCTE-35 messages that carry delivery
/// restrictions become ads.
pub const DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS: &str = "RESTRICTED";

/// `AdTriggers`' default: the SCTE-35 message types MediaPackage treats as ad markers when
/// the request names none. Every entry is a value `__AdTriggersElement` accepts
/// (`crate::ops::types::__AD_TRIGGERS_ELEMENT_VALUES`).
pub const DEFAULT_AD_TRIGGERS: &[&str] = &[
    "SPLICE_INSERT",
    "PROVIDER_ADVERTISEMENT",
    "DISTRIBUTOR_ADVERTISEMENT",
    "PROVIDER_PLACEMENT_OPPORTUNITY",
    "DISTRIBUTOR_PLACEMENT_OPPORTUNITY",
];

/// `PlaylistType`'s default: the media playlist carries `EXT-X-PLAYLIST-TYPE:EVENT`.
pub const DEFAULT_PLAYLIST_TYPE: &str = "EVENT";

/// `PlaylistWindowSeconds`' default: the time window each parent manifest contains.
pub const DEFAULT_PLAYLIST_WINDOW_SECONDS: i32 = 60;

/// `ProgramDateTimeIntervalSeconds`' default: no `EXT-X-PROGRAM-DATE-TIME` tags are
/// inserted, which the member reports as an interval of zero.
///
/// This is why a manifest configuration selecting [`DATERANGE_AD_MARKERS`] has to SEND an
/// interval above [`DATERANGE_PROGRAM_DATE_TIME_INTERVAL_FLOOR`]
/// ([`check_origin_endpoint_ad_markers`]): filling this default into such a request would
/// store the combination the member's documentation forbids.
pub const DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS: i32 = 0;

/// `IncludeIframeOnlyStream`'s default: no I-Frame-only stream is added to the output.
pub const DEFAULT_INCLUDE_IFRAME_ONLY_STREAM: bool = false;

/// `UseAudioRenditionGroup`'s default: audio streams are not placed in rendition groups.
pub const DEFAULT_USE_AUDIO_RENDITION_GROUP: bool = false;

/// `IncludeDvbSubtitles`' default: DVB subtitles are not passed through.
pub const DEFAULT_INCLUDE_DVB_SUBTITLES: bool = false;

/// `DashPackage.Profile`'s default: no profile-specific (HbbTV, DVB-DASH) output.
pub const DEFAULT_DASH_PROFILE: &str = "NONE";

/// `DashPackage.MinBufferTimeSeconds`' default.
pub const DEFAULT_DASH_MIN_BUFFER_TIME_SECONDS: i32 = 30;

/// `DashPackage.SegmentTemplateFormat`'s default.
pub const DEFAULT_DASH_SEGMENT_TEMPLATE_FORMAT: &str = "NUMBER_WITH_TIMELINE";

/// `DashPackage.PeriodTriggers`' default: the MPD is not partitioned into periods, which
/// the member reports as the empty list.
pub const DEFAULT_DASH_PERIOD_TRIGGERS: &[&str] = &[];

/// `DashPackage.ManifestLayout`'s default: every Representation of the MPD carries the
/// tags the member positions, which the enum names FULL
/// (`crate::ops::types::MANIFEST_LAYOUT_VALUES`). Source: the published live-DASH
/// configuration table named above.
pub const DEFAULT_DASH_MANIFEST_LAYOUT: &str = "FULL";

/// `DashPackage.UtcTiming`'s default: the MPD carries no UTCTiming element, which the enum
/// names NONE (`crate::ops::types::UTC_TIMING_VALUES`) — and why no `UtcTimingUri` is
/// filled in beside it: the URI is only "the value attribute of the UTCTiming field when
/// utcTiming is set to HTTP-ISO, HTTP-HEAD or HTTP-XSDATE". Source: the published
/// live-DASH configuration table named above.
pub const DEFAULT_DASH_UTC_TIMING: &str = "NONE";

/// `DashPackage.ManifestWindowSeconds`' default: the "time window (in seconds) contained
/// in each manifest" of a live DASH endpoint. Source: the published live-DASH
/// configuration table named above.
pub const DEFAULT_DASH_MANIFEST_WINDOW_SECONDS: i32 = 60;

/// `DashPackage.MinUpdatePeriodSeconds`' default: the "minimum duration (in seconds)
/// between potential changes" to the MPD. Source: the published live-DASH configuration
/// table named above.
pub const DEFAULT_DASH_MIN_UPDATE_PERIOD_SECONDS: i32 = 15;

/// `DashPackage.SuggestedPresentationDelaySeconds`' default: the "duration (in seconds) to
/// delay live content before presentation". Source: the published live-DASH configuration
/// table named above.
pub const DEFAULT_DASH_SUGGESTED_PRESENTATION_DELAY_SECONDS: i32 = 25;

/// `DashPackage.SegmentDurationSeconds`' default: the "duration (in seconds) of each
/// segment" a DASH package publishes, from the published live-DASH configuration table
/// named above. This default belongs to the DASH block alone — the identically named
/// `HlsPackage` member publishes none and so stays as sent.
pub const DEFAULT_DASH_SEGMENT_DURATION_SECONDS: i32 = 2;

/// `StreamSelection.StreamOrder`'s default: the output keeps the order the source stream
/// published, which the enum names ORIGINAL
/// (`crate::ops::types::STREAM_ORDER_VALUES`).
pub const DEFAULT_STREAM_ORDER: &str = "ORIGINAL";

/// `StreamSelection.MinVideoBitsPerSecond`' default: no video bitrate floor, so every
/// rendition of the source is eligible for the output.
pub const DEFAULT_MIN_VIDEO_BITS_PER_SECOND: i32 = 0;

/// `StreamSelection.MaxVideoBitsPerSecond`' default: no video bitrate ceiling. "No
/// ceiling" is reported as the largest value the member's modelled `__integer` can hold,
/// because the member is a bitrate rather than a nullable switch.
pub const DEFAULT_MAX_VIDEO_BITS_PER_SECOND: i32 = i32::MAX;

/// An owned list for one of the `&[&str]` default lists above.
fn default_list(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// Materialise the `StreamSelection` of a packaging block: the service stores a complete
/// selection on every block it keeps, so an absent one becomes the all-defaults block and
/// a partly supplied one keeps each member it carries and gains the rest.
///
/// `StreamSelection` is a member of all four packaging shapes, so this one function is what
/// fills it on each of them.
fn fill_stream_selection(selection: &mut Option<StreamSelection>) {
    let selection = selection.get_or_insert_with(StreamSelection::default);
    selection.stream_order = selection
        .stream_order
        .take()
        .or_else(|| Some(DEFAULT_STREAM_ORDER.to_string()));
    selection.min_video_bits_per_second = selection
        .min_video_bits_per_second
        .or(Some(DEFAULT_MIN_VIDEO_BITS_PER_SECOND));
    selection.max_video_bits_per_second = selection
        .max_video_bits_per_second
        .or(Some(DEFAULT_MAX_VIDEO_BITS_PER_SECOND));
}

/// Fill the server defaults of the HLS packaging members `package` left unset, leaving
/// every supplied value — including an explicitly empty `AdTriggers` list — alone.
fn fill_hls_package(package: &mut HlsPackage) {
    package.ad_markers = package
        .ad_markers
        .take()
        .or_else(|| Some(DEFAULT_AD_MARKERS.to_string()));
    package.ad_triggers = package
        .ad_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_AD_TRIGGERS)));
    package.ads_on_delivery_restrictions = package
        .ads_on_delivery_restrictions
        .take()
        .or_else(|| Some(DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS.to_string()));
    package.include_dvb_subtitles = package
        .include_dvb_subtitles
        .or(Some(DEFAULT_INCLUDE_DVB_SUBTITLES));
    package.include_iframe_only_stream = package
        .include_iframe_only_stream
        .or(Some(DEFAULT_INCLUDE_IFRAME_ONLY_STREAM));
    package.playlist_type = package
        .playlist_type
        .take()
        .or_else(|| Some(DEFAULT_PLAYLIST_TYPE.to_string()));
    package.playlist_window_seconds = package
        .playlist_window_seconds
        .or(Some(DEFAULT_PLAYLIST_WINDOW_SECONDS));
    package.program_date_time_interval_seconds = package
        .program_date_time_interval_seconds
        .or(Some(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS));
    package.use_audio_rendition_group = package
        .use_audio_rendition_group
        .or(Some(DEFAULT_USE_AUDIO_RENDITION_GROUP));
    fill_stream_selection(&mut package.stream_selection);
}

/// Fill the server defaults of the manifest members a CMAF child `HlsManifest` left unset.
/// The member set it shares with [`HlsPackage`] takes the same defaults.
fn fill_hls_manifest(manifest: &mut HlsManifest) {
    manifest.ad_markers = manifest
        .ad_markers
        .take()
        .or_else(|| Some(DEFAULT_AD_MARKERS.to_string()));
    manifest.ad_triggers = manifest
        .ad_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_AD_TRIGGERS)));
    manifest.ads_on_delivery_restrictions = manifest
        .ads_on_delivery_restrictions
        .take()
        .or_else(|| Some(DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS.to_string()));
    manifest.include_iframe_only_stream = manifest
        .include_iframe_only_stream
        .or(Some(DEFAULT_INCLUDE_IFRAME_ONLY_STREAM));
    manifest.playlist_type = manifest
        .playlist_type
        .take()
        .or_else(|| Some(DEFAULT_PLAYLIST_TYPE.to_string()));
    manifest.playlist_window_seconds = manifest
        .playlist_window_seconds
        .or(Some(DEFAULT_PLAYLIST_WINDOW_SECONDS));
    manifest.program_date_time_interval_seconds = manifest
        .program_date_time_interval_seconds
        .or(Some(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS));
}

/// Fill the server defaults of the DASH packaging members `package` left unset. The
/// SCTE-35 members it shares with [`HlsPackage`] take the same defaults.
fn fill_dash_package(package: &mut DashPackage) {
    package.ad_triggers = package
        .ad_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_AD_TRIGGERS)));
    package.ads_on_delivery_restrictions = package
        .ads_on_delivery_restrictions
        .take()
        .or_else(|| Some(DEFAULT_ADS_ON_DELIVERY_RESTRICTIONS.to_string()));
    package.include_iframe_only_stream = package
        .include_iframe_only_stream
        .or(Some(DEFAULT_INCLUDE_IFRAME_ONLY_STREAM));
    package.manifest_layout = package
        .manifest_layout
        .take()
        .or_else(|| Some(DEFAULT_DASH_MANIFEST_LAYOUT.to_string()));
    package.manifest_window_seconds = package
        .manifest_window_seconds
        .or(Some(DEFAULT_DASH_MANIFEST_WINDOW_SECONDS));
    package.min_buffer_time_seconds = package
        .min_buffer_time_seconds
        .or(Some(DEFAULT_DASH_MIN_BUFFER_TIME_SECONDS));
    package.min_update_period_seconds = package
        .min_update_period_seconds
        .or(Some(DEFAULT_DASH_MIN_UPDATE_PERIOD_SECONDS));
    package.period_triggers = package
        .period_triggers
        .take()
        .or_else(|| Some(default_list(DEFAULT_DASH_PERIOD_TRIGGERS)));
    package.profile = package
        .profile
        .take()
        .or_else(|| Some(DEFAULT_DASH_PROFILE.to_string()));
    package.segment_duration_seconds = package
        .segment_duration_seconds
        .or(Some(DEFAULT_DASH_SEGMENT_DURATION_SECONDS));
    package.segment_template_format = package
        .segment_template_format
        .take()
        .or_else(|| Some(DEFAULT_DASH_SEGMENT_TEMPLATE_FORMAT.to_string()));
    package.suggested_presentation_delay_seconds = package
        .suggested_presentation_delay_seconds
        .or(Some(DEFAULT_DASH_SUGGESTED_PRESENTATION_DELAY_SECONDS));
    package.utc_timing = package
        .utc_timing
        .take()
        .or_else(|| Some(DEFAULT_DASH_UTC_TIMING.to_string()));
    fill_stream_selection(&mut package.stream_selection);
}

/// Materialise the server-filled members of an origin endpoint built from a create or
/// update request, BEFORE it is stored.
///
/// Every write of an endpoint goes through this, so the stored record — and therefore
/// every projection of it — carries the same member set whatever the request omitted.
pub fn apply_origin_endpoint_defaults(record: &mut OriginEndpointRecord) {
    record.origination = record
        .origination
        .take()
        .or_else(|| Some(DEFAULT_ORIGINATION.to_string()));
    record.startover_window_seconds = record
        .startover_window_seconds
        .or(Some(DEFAULT_STARTOVER_WINDOW_SECONDS));
    record.time_delay_seconds = record.time_delay_seconds.or(Some(DEFAULT_TIME_DELAY_SECONDS));
    if let Some(package) = record.hls_package.as_mut() {
        fill_hls_package(package);
    }
    if let Some(package) = record.dash_package.as_mut() {
        fill_dash_package(package);
    }
    if let Some(package) = record.cmaf_package.as_mut() {
        for manifest in package.hls_manifests.iter_mut().flatten() {
            fill_hls_manifest(manifest);
        }
        fill_stream_selection(&mut package.stream_selection);
    }
    if let Some(package) = record.mss_package.as_mut() {
        fill_stream_selection(&mut package.stream_selection);
    }
}

// ---------------------------------------------------------------------------
// How a resource is keyed: the Region it was created in, then its id
//
// MediaPackage is a REGIONAL control plane: every resource it holds belongs to the one
// Region the request that created it was served in — the Region its ARN and its ingest
// and playback URLs name — and an id only has to be unique within that Region. So the
// Region is part of the identity of every resource this service stores, and every
// lookup, listing, mutation, delete and tagging operation resolves an id only among the
// records of the Region its own request is served in ([`request_region`]).
// ---------------------------------------------------------------------------

/// How every resource map in this state is keyed: the Region the resource was created
/// in, then the caller-chosen id the REST routes address it by.
///
/// The Region comes FIRST, so one Region's records are a contiguous range of the
/// `BTreeMap` read in id order ([`region_slice`]) — which is what lets a listing page
/// one Region's resources in one stable order. The same id in two Regions is two
/// distinct keys, so it names two independent resources.
pub type ResourceKey = (String, String);

/// The key `id` is stored under when the request that created it was served in `region`.
pub fn resource_key(region: &str, id: &str) -> ResourceKey {
    (region.to_string(), id.to_string())
}

/// The records of ONE Region, in id order.
///
/// A `BTreeMap` keyed by [`ResourceKey`] orders by Region first, so a Region's records
/// are the contiguous range that starts at its smallest possible key; the scan stops at
/// the first key naming another Region.
fn region_slice<'map, V>(
    map: &'map BTreeMap<ResourceKey, V>,
    region: &str,
) -> impl Iterator<Item = &'map V> {
    let region = region.to_string();
    map.range(resource_key(&region, "")..)
        .take_while(move |((stored_region, _), _)| *stored_region == region)
        .map(|(_, record)| record)
}

/// A Channel, exactly as `CreateChannel` stored it.
#[derive(Debug, Clone, Default)]
pub struct ChannelRecord {
    pub id: String,
    /// The Region this channel was created in — the one its ARN and its ingest URLs
    /// carry, and the only one it resolves in. See [`ResourceKey`].
    pub region: String,
    pub arn: String,
    pub description: Option<String>,
    /// ISO-8601, minted once at creation and never recomputed on a read.
    pub created_at: String,
    pub ingest_endpoints: Vec<IngestEndpoint>,
    /// The live window of media an encoder has PUSHED into this channel through either of
    /// its ingest endpoints, which is what its origin endpoints serve
    /// ([`data_plane`]). Empty for a channel nothing has been pushed to.
    pub ingest: data_plane::ChannelIngest,
    pub egress_log_group_name: Option<String>,
    pub ingress_log_group_name: Option<String>,
    pub tags: BTreeMap<String, String>,
}

impl ChannelRecord {
    /// Where in [`Self::ingest_endpoints`] the endpoint whose `Id` is `id` sits, or `None`
    /// when this channel carries no such endpoint.
    ///
    /// A POSITION rather than a reference, so a caller that has to mint through the store
    /// (which needs the store mutably) can resolve the target first and write to it after.
    /// An ingest endpoint exists only as part of its channel, so this is the only way it
    /// is ever addressed.
    pub fn ingest_endpoint_position(&self, id: &str) -> Option<usize> {
        self.ingest_endpoints
            .iter()
            .position(|endpoint| endpoint.id.as_deref() == Some(id))
    }

    /// The modelled `Channel` shape for this record. Every channel-returning operation
    /// renders its output from this one projection, so they cannot disagree.
    pub fn to_channel(&self) -> Channel {
        Channel {
            arn: Some(self.arn.clone()),
            created_at: Some(self.created_at.clone()),
            description: self.description.clone(),
            egress_access_logs: Some(EgressAccessLogs {
                log_group_name: self.egress_log_group_name.clone(),
            }),
            hls_ingest: Some(HlsIngest {
                ingest_endpoints: Some(self.ingest_endpoints.clone()),
            }),
            id: Some(self.id.clone()),
            ingress_access_logs: Some(IngressAccessLogs {
                log_group_name: self.ingress_log_group_name.clone(),
            }),
            tags: Some(tags_to_wire(&self.tags)),
        }
    }
}

/// An OriginEndpoint, exactly as `CreateOriginEndpoint` stored it.
#[derive(Debug, Clone, Default)]
pub struct OriginEndpointRecord {
    pub id: String,
    /// The Region this endpoint was created in — the one its ARN and its playback URL
    /// carry, and the only one it resolves in. See [`ResourceKey`].
    pub region: String,
    pub arn: String,
    pub channel_id: String,
    pub description: Option<String>,
    pub manifest_name: Option<String>,
    /// The playback URL, derived from the endpoint's effective [`Self::manifest_name`] and
    /// its packaging format by [`playback_url`], and re-derived by every write of the
    /// record, so every read returns the URL the endpoint serves NOW.
    pub url: String,
    pub created_at: String,
    pub hls_package: Option<HlsPackage>,
    pub dash_package: Option<DashPackage>,
    /// The stored CMAF package, every member of it that the SERVICE derives included —
    /// which is why it is not the request's own shape but the materialisation of
    /// [`Self::cmaf_package_parameters`] ([`stored_cmaf_package`]).
    pub cmaf_package: Option<CmafPackage>,
    /// The CMAF packaging configuration exactly as the write that stored it sent it.
    ///
    /// A child HLS manifest sent without a `ManifestName` of its own takes the endpoint's
    /// effective one, and its playback `Url` is derived from whichever name it ends up
    /// with, so the stored package is a value DERIVED from these parameters and the
    /// endpoint's own `ManifestName`. Keeping the parameters is what lets a later write
    /// that moves the endpoint's `ManifestName` re-derive the children from what the
    /// caller actually sent ([`rederive_cmaf_package`]) instead of guessing, from a
    /// materialised child, whether its name was sent or inherited. It costs no unbounded
    /// memory: these are the members a write already had bounded
    /// ([`bound_origin_endpoint_members`]) before it was stored.
    pub cmaf_package_parameters: Option<CmafPackageCreateOrUpdateParameters>,
    pub mss_package: Option<MssPackage>,
    pub authorization: Option<Authorization>,
    pub origination: Option<String>,
    pub startover_window_seconds: Option<i32>,
    pub time_delay_seconds: Option<i32>,
    pub whitelist: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

impl OriginEndpointRecord {
    /// Which packaging configuration this endpoint is stored with, and so which manifest
    /// its playback URL ends in.
    ///
    /// Exactly one of the four is stored (a create that names none or several is
    /// refused), so this answers `Some` for every record this service holds.
    pub fn packaging_kind(&self) -> Option<PackagingKind> {
        if self.hls_package.is_some() {
            Some(PackagingKind::Hls)
        } else if self.dash_package.is_some() {
            Some(PackagingKind::Dash)
        } else if self.cmaf_package.is_some() {
            Some(PackagingKind::Cmaf)
        } else if self.mss_package.is_some() {
            Some(PackagingKind::Mss)
        } else {
            None
        }
    }

    /// Whether this endpoint retains a window of content for STARTOVER (time-shift)
    /// playback — the already-published content a later read of the endpoint can reach
    /// back into.
    ///
    /// Read off the STORED record, so it answers for the window the endpoint carries now,
    /// whichever write (a create or an update) last set it, and an absent member answers
    /// the service default the way every projection of the record does.
    pub fn startover_enabled(&self) -> bool {
        self.startover_window_seconds
            .unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS)
            >= MIN_ENABLED_STARTOVER_WINDOW_SECONDS
    }

    /// The modelled `OriginEndpoint` shape for this record.
    pub fn to_origin_endpoint(&self) -> OriginEndpoint {
        OriginEndpoint {
            arn: Some(self.arn.clone()),
            authorization: self.authorization.clone(),
            channel_id: Some(self.channel_id.clone()),
            cmaf_package: self.cmaf_package.clone(),
            created_at: Some(self.created_at.clone()),
            dash_package: self.dash_package.clone(),
            description: self.description.clone(),
            hls_package: self.hls_package.clone(),
            id: Some(self.id.clone()),
            manifest_name: self.manifest_name.clone(),
            mss_package: self.mss_package.clone(),
            origination: self.origination.clone(),
            startover_window_seconds: self.startover_window_seconds,
            tags: Some(tags_to_wire(&self.tags)),
            time_delay_seconds: self.time_delay_seconds,
            url: Some(self.url.clone()),
            whitelist: Some(self.whitelist.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// A harvest job's status, and the clock that moves it
//
// `CreateHarvestJob` submits a job that exports a window of an origin endpoint's
// packaged content to an S3 bucket. The submitted job is `IN_PROGRESS` — a NON-TERMINAL
// status — so the service owes it a later status without anybody calling again. That
// owed work is stored as a DUE INSTANT on the record and materialised by
// [`harvest_export::settle_due_harvests`], which every harvest-job handler calls first: a
// read reports what the clock has already made due and never advances a job because it
// was polled.
//
// What that materialisation DOES is the harvest: the job's top-level manifest is written
// to the destination the submission named, and the terminal status is whichever of
// `SUCCEEDED` / `FAILED` that write's outcome decides (see [`harvest_export`]). Nothing
// here fixes the outcome in advance.
// ---------------------------------------------------------------------------

/// The `Status` values the model declares for a HarvestJob, so no handler spells one as a
/// bare literal. The full set is `ops::types::STATUS_VALUES`
/// (`smithy_query(query="shape",shape="com.amazonaws.mediapackage#Status",…)` reports
/// `IN_PROGRESS | SUCCEEDED | FAILED`).
pub const HARVEST_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`HARVEST_IN_PROGRESS`].
pub const HARVEST_SUCCEEDED: &str = "SUCCEEDED";
/// See [`HARVEST_IN_PROGRESS`].
pub const HARVEST_FAILED: &str = "FAILED";

/// The statuses a harvest job never leaves: the harvest finished, or it failed.
pub const HARVEST_TERMINAL_STATUSES: &[&str] = &[HARVEST_SUCCEEDED, HARVEST_FAILED];

/// How long after `CreateHarvestJob` the clock owes the submitted job its harvest, and
/// so its terminal status.
///
/// AWS documents no duration — how long a harvest takes depends on the window harvested
/// — so this span is CHOSEN, not modelled: 500 ms, inside the project's 250-750 ms band
/// for a transition the clock owes. It is deliberately non-zero, so a caller reading
/// straight after the create still observes the submitted job `IN_PROGRESS` and the
/// destination still empty. Nothing sleeps for it: the due instant is stored and compared
/// against the clock.
pub const HARVEST_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// A HarvestJob, exactly as `CreateHarvestJob` stored it.
#[derive(Debug, Clone, Default)]
pub struct HarvestJobRecord {
    pub id: String,
    /// The Region this job was submitted in — the one its ARN carries, and the only one
    /// it resolves in. See [`ResourceKey`].
    pub region: String,
    pub arn: String,
    /// The channel the harvested endpoint packages, read from the STORED endpoint at
    /// submission time rather than from the request, which carries no channel id.
    pub channel_id: String,
    pub origin_endpoint_id: String,
    pub s3_destination: S3Destination,
    pub start_time: String,
    pub end_time: String,
    /// ISO-8601, minted once at submission and never recomputed on a read.
    pub created_at: String,
    /// One of [`HARVEST_IN_PROGRESS`] or [`HARVEST_TERMINAL_STATUSES`].
    pub status: String,
    /// The instant the clock makes this job's harvest due. `None` for a job whose harvest
    /// has been carried out — which is every job in a terminal status — so nothing is owed
    /// twice.
    pub next_status_due: Option<SystemTime>,
}

impl HarvestJobRecord {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        HARVEST_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// The modelled `HarvestJob` shape for this record. Every harvest-job-returning
    /// operation renders its output from this one projection, so they cannot disagree.
    pub fn to_harvest_job(&self) -> HarvestJob {
        HarvestJob {
            arn: Some(self.arn.clone()),
            channel_id: Some(self.channel_id.clone()),
            created_at: Some(self.created_at.clone()),
            end_time: Some(self.end_time.clone()),
            id: Some(self.id.clone()),
            origin_endpoint_id: Some(self.origin_endpoint_id.clone()),
            s3_destination: Some(self.s3_destination.clone()),
            start_time: Some(self.start_time.clone()),
            status: Some(self.status.clone()),
        }
    }
}

/// The window a stored job harvests, as `(the instant it starts, its length)` in
/// NANOSECONDS — the unit [`parse_instant`] reads a bound to, because a bound may carry a
/// fractional second.
///
/// `None` when the bounds do not name an interval of media. `CreateHarvestJob` refuses
/// such a submission, so a STORED job's bounds always do; a job that somehow carries
/// others names no window to export, which is a harvest that produces nothing.
pub fn harvest_window(job: &HarvestJobRecord) -> Option<(i128, i128)> {
    let start = parse_instant(&job.start_time)?;
    let end = parse_instant(&job.end_time)?;
    (end > start).then_some((start, end - start))
}

/// Per-service state for com-amazonaws-mediapackage-mediapackage. A handler reaches it through [`service_state`].
///
/// Every collection is a `BTreeMap` keyed by a [`ResourceKey`] — the Region the resource
/// was created in, then the resource id the API itself keys on — so one Region's records
/// are a contiguous range read in id order, and `List*` output and its paging tokens are
/// in a stable, caller-independent order within that Region. Each map is bounded by a
/// total entry count — [`MAX_CHANNELS`], [`MAX_ORIGIN_ENDPOINTS`] and
/// [`MAX_HARVEST_JOBS`] — because the memory the process holds is the sum over every
/// Region.
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackageMediapackageState {
    pub channels: BTreeMap<ResourceKey, ChannelRecord>,
    pub origin_endpoints: BTreeMap<ResourceKey, OriginEndpointRecord>,
    pub harvest_jobs: BTreeMap<ResourceKey, HarvestJobRecord>,
    /// How many ids this store has minted. Part of every minted id's seed, so no two
    /// minted ids collide even when two calls land in the same nanosecond.
    pub minted: u64,
}

/// The ONE resolution of a tagging operation's `ResourceArn` to the stored tag map of the
/// resource that ARN names, written once over the BORROW KIND.
///
/// [`ComAmazonawsMediapackageMediapackageState::tags`] and
/// [`ComAmazonawsMediapackageMediapackageState::tags_mut`] are this one rule read-only and
/// read-write: each expands it with the record accessors of its own borrow kind
/// (`channel`/`origin_endpoint` against `channel_mut`/`origin_endpoint_mut`) and with the
/// projection of such a borrow to the record's `tags`. So which resource types a
/// `ResourceArn` can name, and the condition under which one resolves — the record is held
/// by the request's own Region AND carries exactly the ARN the caller sent — are stated in
/// this single place: a third taggable resource type, or a change to the ARN-match rule, is
/// one edit here, and the reporting path and the writing path cannot come to disagree about
/// which resource an ARN resolves to.
macro_rules! resolve_tag_map {
    (
        $state:expr, $region:expr, $arn:expr, $target:expr,
        channel = $channel:ident,
        origin_endpoint = $origin_endpoint:ident,
        tags = $tags:expr
    ) => {
        match $target {
            TaggedResource::Channel(id) => $state
                .$channel($region, id)
                .filter(|record| record.arn == $arn)
                .map($tags),
            TaggedResource::OriginEndpoint(id) => $state
                .$origin_endpoint($region, id)
                .filter(|record| record.arn == $arn)
                .map($tags),
        }
    };
}

impl ComAmazonawsMediapackageMediapackageState {
    /// The Channel `id` names IN `region`, for a reporting operation. `None` for an id
    /// only another Region holds: that channel does not exist here.
    pub fn channel(&self, region: &str, id: &str) -> Option<&ChannelRecord> {
        self.channels.get(&resource_key(region, id))
    }

    /// The Channel `id` names in `region`, for an operation the spec says mutates it
    /// (`UpdateChannel`, `ConfigureLogs`, the credential rotations).
    pub fn channel_mut(&mut self, region: &str, id: &str) -> Option<&mut ChannelRecord> {
        self.channels.get_mut(&resource_key(region, id))
    }

    /// Whether `region` holds a Channel under `id`.
    pub fn has_channel(&self, region: &str, id: &str) -> bool {
        self.channels.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the Channel `id` of its own Region.
    pub fn insert_channel(&mut self, record: ChannelRecord) {
        let key = resource_key(&record.region, &record.id);
        self.channels.insert(key, record);
    }

    /// Remove the Channel `id` names in `region`, returning what was stored.
    pub fn remove_channel(&mut self, region: &str, id: &str) -> Option<ChannelRecord> {
        self.channels.remove(&resource_key(region, id))
    }

    /// Every Channel stored in `region`, in id order.
    pub fn channels_in(&self, region: &str) -> impl Iterator<Item = &ChannelRecord> {
        region_slice(&self.channels, region)
    }

    /// The OriginEndpoint `id` names in `region`.
    pub fn origin_endpoint(&self, region: &str, id: &str) -> Option<&OriginEndpointRecord> {
        self.origin_endpoints.get(&resource_key(region, id))
    }

    /// The OriginEndpoint `id` names in `region`, for `UpdateOriginEndpoint`.
    pub fn origin_endpoint_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut OriginEndpointRecord> {
        self.origin_endpoints.get_mut(&resource_key(region, id))
    }

    /// Whether `region` holds an OriginEndpoint under `id`.
    pub fn has_origin_endpoint(&self, region: &str, id: &str) -> bool {
        self.origin_endpoints.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the OriginEndpoint `id` of its own Region.
    pub fn insert_origin_endpoint(&mut self, record: OriginEndpointRecord) {
        let key = resource_key(&record.region, &record.id);
        self.origin_endpoints.insert(key, record);
    }

    /// Remove the OriginEndpoint `id` names in `region`, returning what was stored.
    pub fn remove_origin_endpoint(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<OriginEndpointRecord> {
        self.origin_endpoints.remove(&resource_key(region, id))
    }

    /// Every OriginEndpoint stored in `region`, in id order.
    pub fn origin_endpoints_in(&self, region: &str) -> impl Iterator<Item = &OriginEndpointRecord> {
        region_slice(&self.origin_endpoints, region)
    }

    /// The HarvestJob `id` names in `region`.
    pub fn harvest_job(&self, region: &str, id: &str) -> Option<&HarvestJobRecord> {
        self.harvest_jobs.get(&resource_key(region, id))
    }

    /// Whether `region` holds a HarvestJob under `id`.
    pub fn has_harvest_job(&self, region: &str, id: &str) -> bool {
        self.harvest_jobs.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the HarvestJob `id` of its own Region.
    pub fn insert_harvest_job(&mut self, record: HarvestJobRecord) {
        let key = resource_key(&record.region, &record.id);
        self.harvest_jobs.insert(key, record);
    }

    /// Every HarvestJob stored in `region`, in id order.
    pub fn harvest_jobs_in(&self, region: &str) -> impl Iterator<Item = &HarvestJobRecord> {
        region_slice(&self.harvest_jobs, region)
    }

    /// A fresh lowercase-hex id of `bytes` bytes (so `2 * bytes` characters).
    ///
    /// `minted` advances on every call and is hashed into the seed together with the wall
    /// clock and the process id, so repeated calls never return the same value. This is
    /// how the sibling services in this workspace mint a system-generated id or secret.
    pub fn mint_hex(&mut self, label: &str, bytes: usize) -> String {
        use sha2::Digest;
        self.minted = self.minted.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{label}|{}|{}|{nanos}", self.minted, std::process::id());
        let digest = sha2::Sha256::digest(seed.as_bytes());
        hex::encode(&digest[..bytes.min(digest.len())])
    }

    /// The redundant pair of ingest endpoints MediaPackage provisions for a new Channel.
    ///
    /// Each endpoint's id and its ingest credentials — the model documents `Id`,
    /// `Username` and `Password` as "system generated" — are minted once, here, with
    /// MediaPackage's generated width ([`INGEST_GENERATED_BYTES`], i.e. 32 lowercase-hex
    /// characters each), and stored on the channel record, so every later read of the
    /// channel returns exactly these values. The URL embeds the endpoint id, so it stays
    /// valid for exactly that endpoint.
    pub fn mint_ingest_endpoints(&mut self, region: &str, channel_id: &str) -> Vec<IngestEndpoint> {
        let hostname = media_hostname(region);
        let channel_label = channel_path_label(channel_id, region);
        (0..INGEST_ENDPOINT_COUNT)
            .map(|_| {
                let id = self.mint_hex("ingest-endpoint", INGEST_GENERATED_BYTES);
                let (username, password) = self.mint_ingest_credentials();
                IngestEndpoint {
                    url: Some(format!(
                        "https://{hostname}{}",
                        data_plane::ingest_path(&channel_label, &id)
                    )),
                    id: Some(id),
                    username: Some(username),
                    password: Some(password),
                }
            })
            .collect()
    }

    /// One freshly minted ingest credential pair — `(username, password)` — for an ingest
    /// endpoint to carry.
    ///
    /// The generator and the width are the ones a brand-new endpoint's credentials are
    /// minted with ([`INGEST_GENERATED_BYTES`]), because a rotated credential is a
    /// system-generated ingest credential like any other: the model documents `Username`
    /// and `Password` as "system generated" and constrains neither. Minting goes through
    /// [`Self::mint_hex`], so a rotation can never hand back the pair it replaced.
    ///
    /// Shared by the credential-rotating operations, so a rotated endpoint is
    /// indistinguishable from a newly provisioned one.
    pub fn mint_ingest_credentials(&mut self) -> (String, String) {
        let username = self.mint_hex("ingest-username", INGEST_GENERATED_BYTES);
        let password = self.mint_hex("ingest-password", INGEST_GENERATED_BYTES);
        (username, password)
    }

    /// The stored tag set of the resource a tagging operation's `ResourceArn` named, for
    /// a REPORTING operation to render. `None` when this request resolves no such
    /// resource, which is what tells `ListTagsForResource` to report a 404 instead of a
    /// tag set.
    ///
    /// Resolved like every other read of this regional control plane: only among the
    /// resources `region` holds, and only when `arn` is the ARN the stored record
    /// carries — so an ARN naming another Region, another account or another partition
    /// resolves to nothing rather than to this Region's resource of the same id.
    ///
    /// The same lookup as [`tags_mut`](Self::tags_mut) — the one [`resolve_tag_map`]
    /// states — without the write capability: a read must not be able to change what a
    /// later caller sees.
    pub fn tags(
        &self,
        region: &str,
        arn: &str,
        target: &TaggedResource,
    ) -> Option<&BTreeMap<String, String>> {
        resolve_tag_map!(
            self,
            region,
            arn,
            target,
            channel = channel,
            origin_endpoint = origin_endpoint,
            tags = |record| &record.tags
        )
    }

    /// The stored tag set of the resource a tagging operation's `ResourceArn` named, for
    /// that operation to change in place. `None` when this request resolves no such
    /// resource, which is what tells `TagResource`/`UntagResource` to report a 404
    /// instead of writing.
    ///
    /// Scoped exactly as [`tags`](Self::tags) is, because both expand the one
    /// [`resolve_tag_map`] — this one over the mutable borrow of the same record.
    pub fn tags_mut(
        &mut self,
        region: &str,
        arn: &str,
        target: &TaggedResource,
    ) -> Option<&mut BTreeMap<String, String>> {
        resolve_tag_map!(
            self,
            region,
            arn,
            target,
            channel = channel_mut,
            origin_endpoint = origin_endpoint_mut,
            tags = |record| &mut record.tags
        )
    }

    /// How many stored origin endpoints OF `region` package `channel_id`.
    /// `DeleteChannel` reads it: a channel that still has endpoints cannot be deleted,
    /// which is what keeps the two stores consistent (no endpoint ever names a channel
    /// this service no longer holds). An endpoint of another Region packages that
    /// Region's channel of the same id, so it is not counted here.
    pub fn origin_endpoints_of(&self, region: &str, channel_id: &str) -> usize {
        self.origin_endpoints_in(region)
            .filter(|record| record.channel_id == channel_id)
            .count()
    }

    /// Every harvest the clock has already made due, as the export each one owes.
    ///
    /// Called under this store's guard by [`harvest_export::settle_due_harvests`], which
    /// then carries each export out with NO guard held. Only a harvest that was already due
    /// at `now` is reported — the work was owed at that instant whether or not anybody
    /// called.
    ///
    /// A reported harvest KEEPS the instant it was owed at until its outcome is recorded
    /// ([`Self::record_harvest_outcome`], which is what clears it), so an export whose call
    /// never finished — the request serving it was cancelled — is still owed and is carried
    /// out by the next call, rather than leaving the job reporting work in progress that
    /// nothing will ever finish. The cost of that choice is that two calls racing the same
    /// due harvest may both carry it out: both write the same manifest to the same key, so
    /// the destination holds what it would have held either way.
    ///
    /// A due job with nothing to export produces no output at all, so it reaches the
    /// modelled failure status here rather than being reported: the endpoint it harvests
    /// stores no packaging configuration, or its window names no interval of media (see
    /// [`harvest_export::harvested_manifest`]). Every job reported is still
    /// [`HARVEST_IN_PROGRESS`] until its outcome is recorded.
    ///
    /// At most [`MAX_HARVESTS_PER_CALL`] are reported, which is what bounds the work one
    /// request pays for; the rest keep the instant they were owed at and are taken by the
    /// next call.
    pub fn take_due_harvests(&mut self, now: SystemTime) -> Vec<harvest_export::DueHarvest> {
        let due: Vec<ResourceKey> = self
            .harvest_jobs
            .iter()
            .filter(|(_, job)| job.next_status_due.is_some_and(|instant| now >= instant))
            .map(|(key, _)| key.clone())
            .take(MAX_HARVESTS_PER_CALL)
            .collect();
        let mut owed = Vec::new();
        for key in due {
            // The export is worked out while only this store is read; the record is then
            // taken mutably to settle a job with nothing to export, so no two borrows of it
            // overlap.
            let export = {
                let Some(job) = self.harvest_jobs.get(&key) else {
                    continue;
                };
                self.origin_endpoint(&job.region, &job.origin_endpoint_id)
                    .and_then(|endpoint| harvest_export::harvested_manifest(endpoint, job))
                    .map(|(manifest, media_type)| harvest_export::DueHarvest {
                        job: key.clone(),
                        region: job.region.clone(),
                        bucket: job.s3_destination.bucket_name.clone(),
                        manifest_key: job.s3_destination.manifest_key.clone(),
                        manifest,
                        media_type,
                    })
            };
            match export {
                // Still owed until its outcome is recorded, so it is not lost if the call
                // carrying it out never finishes.
                Some(export) => owed.push(export),
                None => {
                    if let Some(job) = self.harvest_jobs.get_mut(&key) {
                        job.status = HARVEST_FAILED.to_string();
                        job.next_status_due = None;
                    }
                }
            }
        }
        owed
    }

    /// Record what a harvest this service carried out did: the modelled terminal status
    /// the outcome of its export decides, and nothing further owed — this is what clears
    /// the due instant the harvest kept while it ran.
    ///
    /// `exported` is whether the object store took the job's top-level manifest, which is
    /// the one thing the status reports (see [`harvest_export`]).
    ///
    /// A job the clock moved on while its export ran — its source endpoint was removed or
    /// stopped retaining content, so [`Self::fail_harvests_of_origin_endpoint`] already
    /// failed it — keeps the status that gave it: a job never leaves a terminal status, and
    /// that path already cleared what it owed.
    pub fn record_harvest_outcome(&mut self, job: &ResourceKey, exported: bool) {
        let Some(record) = self.harvest_jobs.get_mut(job) else {
            return;
        };
        if record.is_terminal() {
            return;
        }
        record.status = if exported {
            HARVEST_SUCCEEDED
        } else {
            HARVEST_FAILED
        }
        .to_string();
        record.next_status_due = None;
    }

    /// Fail every harvest job OF `region` that has not finished harvesting the origin
    /// endpoint `origin_endpoint_id`, because that endpoint no longer holds content for
    /// it to export.
    ///
    /// A harvest exports content the origin endpoint has already published, which the
    /// endpoint keeps only for as long as its startover window
    /// ([`OriginEndpointRecord::startover_enabled`]) retains it, so a job that has not
    /// reached a terminal status when a write takes that source away — the endpoint
    /// removed, or its startover window left retaining nothing — can never carry that
    /// harvest out: it reaches [`HARVEST_FAILED`] — the modelled status for a harvest that
    /// did not produce its output — and owes no further transition, instead of reporting
    /// work in progress, and later an outcome, for content this service no longer holds.
    ///
    /// Called by every write that can take the source away, so the two paths cannot
    /// disagree about what becomes of the jobs.
    ///
    /// Only the unfinished jobs move. A job that already reached a terminal status
    /// finished while the endpoint still held its content, and the record of a submitted
    /// harvest is HISTORY, not a reference this service resolves on a read: the stored
    /// `origin_endpoint_id` "cannot be changed after the HarvestJob is submitted", no
    /// operation of this service removes a stored job, and every read keeps reporting the
    /// ids the job was submitted under.
    ///
    /// A job of another Region harvests that Region's endpoint of the same id, so it is
    /// not touched here — the same scope every other cross-store read of this regional
    /// control plane uses ([`origin_endpoints_of`](Self::origin_endpoints_of)).
    pub fn fail_harvests_of_origin_endpoint(&mut self, region: &str, origin_endpoint_id: &str) {
        for job in self.harvest_jobs.values_mut() {
            if job.region != region
                || job.origin_endpoint_id != origin_endpoint_id
                || job.is_terminal()
            {
                continue;
            }
            job.status = HARVEST_FAILED.to_string();
            job.next_status_due = None;
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediapackageMediapackageState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediapackageMediapackageState>)
}

// ---------------------------------------------------------------------------
// What `CreateHarvestJob` decides about a submission before it stores one
//
// Both rules below are about the REQUEST, not about the record: the window has to name an
// interval of media, and the destination role has to be one this service could write as.
// They live here, beside the store the handler guards, rather than inside the harvest-job
// record's own section, because neither is a property of a stored job.
// ---------------------------------------------------------------------------

/// The service principal MediaPackage assumes a CALLER-SUPPLIED role as — a harvest job's
/// `S3Destination.RoleArn`, a CDN authorization's `SecretsRoleArn`, a SPEKE key provider's
/// `RoleArn`.
///
/// The service acts as that role (writing to the caller's bucket, reading the CDN secret,
/// reaching the key provider), so the role's trust policy has to let this principal assume
/// it — the same up-front check every service that acts as a caller-supplied role makes.
pub const SERVICE_PRINCIPAL: &str = "mediapackage.amazonaws.com";

/// The instant an ISO-8601 date-time names, in NANOSECONDS since the epoch.
///
/// `StartTime` and `EndTime` are modelled as `__string`, so the FRAMEWORK does not parse
/// them: the handler does, because the service compares them. Both zone spellings
/// RFC 3339 admits are read — a `Z` instant and a numeric offset, which is what
/// `datetime.isoformat()` produces — so two requests naming the same instant are judged
/// the same way; a bound carrying NO zone suffix is read as UTC (see the body). The offset
/// is resolved through the ONE shared reader every service with a string-typed timestamp
/// uses (`service_common::rfc3339`), so nothing about reading a zone is written here; the
/// calendar conversion stays in the maintained library.
///
/// The unit is nanoseconds and not seconds because RFC 3339 admits a fractional second
/// and the comparison this feeds is a STRICT one: truncating to whole seconds would make
/// a half-second window compare as empty and refuse a window the caller can harvest.
///
/// `None` for text that is not an ISO-8601 instant at all — which the caller is told
/// about ([`instant_invalid`]) rather than silently accepted — for EVERY input, including
/// arbitrary non-ASCII text: the text comes straight off the wire, so this must never
/// panic.
pub fn parse_instant(text: &str) -> Option<i128> {
    // A bound carrying no zone suffix at all is read as UTC rather than refused: the
    // members are modelled plain `__string` with no `@pattern`, nothing published says a
    // zone is required, and a client sending `datetime.isoformat()` output for a naive
    // datetime sends exactly this form. BOTH bounds are read the same way, so the ordering
    // comparison stays well defined. Text that is not a date-time at all still fails the
    // parse below and is refused.
    let (civil, offset) = ::service_common::rfc3339::split_utc_offset(text).unwrap_or((text, 0));
    let instant = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    let secs = i128::from(instant.secs().checked_sub(offset)?);
    Some(secs * 1_000_000_000 + i128::from(instant.subsec_nanos()))
}

/// `UnprocessableEntityException` (422) for a timestamp member this service cannot read as
/// an instant.
pub fn instant_invalid(member: &str) -> ServiceError {
    member_invalid(member, "must be an ISO 8601 date-time")
}

/// `UnprocessableEntityException` (422) for a harvest window whose end does not come after
/// its start.
///
/// A harvest window is an interval of media, so its end has to be LATER than its start: an
/// empty or inverted window names no content to harvest. Keyed by `endTime`, which is the
/// member the caller has to fix (`startTime` names a real instant).
pub fn harvest_window_invalid() -> ServiceError {
    member_invalid("endTime", "must be later than the start time")
}

/// `UnprocessableEntityException` (422) for a submission whose origin endpoint retains no
/// startover window.
///
/// A harvest reads already-published content back out of the endpoint's startover
/// (time-shift) window, so an endpoint with startover playback disabled
/// ([`OriginEndpointRecord::startover_enabled`]) holds nothing a harvest could export.
/// Decided against the STORED endpoint, so it is reported the way everything this service
/// decides against state is — a list of reasons for the member the caller has to fix,
/// which for this rule is the one naming the endpoint.
pub fn startover_required(member: &str) -> ServiceError {
    ServiceError::unprocessable_state(member, "must have startover enabled")
}

/// The IAM store a harvest job's destination role is resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so MediaPackage reads the very records `CreateRole` wrote
/// rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store in the SAME shared engine, which is where the account's roles live.
///
/// This is the IAM service's OWN accessor, called rather than copied. `None` has the same
/// meaning as for [`service_state`]: the engine carries no such store, which a handler
/// reports as a server fault.
pub fn iam_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IamState>> {
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::service_state(engine)
}

/// The member a harvest job's destination role travels under, which its refusal is keyed
/// by (`S3Destination$RoleArn` is `@jsonName("roleArn")` inside `s3Destination`).
pub const HARVEST_ROLE_MEMBER: &str = "s3Destination.roleArn";

/// The requirement a member naming a role MediaPackage may not act as has failed, as the
/// service words it.
///
/// FIXED text for the error class, with the principal the trust policy has to admit
/// ([`SERVICE_PRINCIPAL`]) in it; the member the refusal is keyed by supplies the rest of
/// the sentence, so every member carrying such a role states the same requirement
/// (recorded 2026-10-29, `CreateHarvestJob`).
pub fn assumable_role_requirement() -> String {
    format!(
        "must be assumable by MediaPackage. Please ensure that its trust policy includes \
         {SERVICE_PRINCIPAL}"
    )
}

/// `None` when this emulator has no reason to think MediaPackage could not act as the role
/// `role_arn` that `member` names; otherwise the refusal that answers the request.
///
/// MediaPackage acts AS a caller-supplied role — it writes a harvest to the caller's bucket
/// as the destination role, reads the CDN secret as the authorization role, reaches the key
/// provider as the SPEKE role — so the service checks up front that the role's trust policy
/// lets [`SERVICE_PRINCIPAL`] assume it and refuses the request when it does not. The role
/// is resolved against the account's IAM records — the same records `CreateRole` wrote — by
/// its ARN. An ARN the IAM store holds no role for is NOT evidence of a missing trust (the
/// role may belong to another account, or to a client that never told this emulator about
/// it), so the request goes ahead; only a role this emulator holds and whose trust policy
/// withholds the grant is refused.
///
/// ONE function for every such member: the rule is about the role, not about the operation
/// that named it, so `member` is the only thing that differs between its callers. A role
/// the service has to act as to carry out the work the request asks it to do NOW — the
/// destination role a harvest's own output is written as — is resolved one step further
/// than this: see [`harvest_role_refusal`].
pub fn assume_role_refusal(member: &str, role_arn: &str, iam: &IamState) -> Option<ServiceError> {
    let role = iam.roles.values().find(|role| role.arn == role_arn)?;
    let document = role.assume_role_policy_document.as_deref()?;
    let admits =
        ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL);
    match admits {
        // A document this emulator cannot read as a policy is no evidence either way.
        None | Some(true) => None,
        Some(false) => Some(member_invalid(member, assumable_role_requirement())),
    }
}

/// Whether `role_arn` names a role of THIS account that the account's IAM records hold no
/// role for.
///
/// Which account a role ARN names is the shared reading every service that resolves a
/// caller-supplied role ARN uses ([`::service_common::iam_role::role_name`]): it answers a
/// role name only for an ARN of this account (an empty account field included), and nothing
/// for one of ANOTHER account, whose roles are not this emulator's IAM to be asked about. So
/// `false` covers both "this emulator holds the role" and "this emulator could not know".
fn role_absent_from_this_account(role_arn: &str, iam: &IamState) -> bool {
    ::service_common::iam_role::role_name(role_arn).is_some()
        && !iam.roles.values().any(|role| role.arn == role_arn)
}

/// [`assume_role_refusal`] for a harvest job's destination role, which is resolved one step
/// further than a role an endpoint write merely records.
///
/// MediaPackage carries a submitted harvest out ITSELF, writing the harvested manifest to
/// the caller's bucket as this role ([`harvest_export`]), so a submission naming a role the
/// service could not act as is refused up front. A role of THIS account that the account's
/// IAM records hold no role for ([`role_absent_from_this_account`]) is one of those: there
/// is no role to assume, so the harvest could never be written, and the refusal is the one
/// this service words for every member naming a role it cannot act as
/// ([`assumable_role_requirement`]) — "no such role" and "a trust that withholds the grant"
/// are the same fact about the harvest. An ARN of another account stays exempt, because this
/// emulator holds no record of another account's roles and so establishes nothing about it.
pub fn harvest_role_refusal(role_arn: &str, iam: &IamState) -> Option<ServiceError> {
    if role_absent_from_this_account(role_arn, iam) {
        return Some(member_invalid(
            HARVEST_ROLE_MEMBER,
            assumable_role_requirement(),
        ));
    }
    assume_role_refusal(HARVEST_ROLE_MEMBER, role_arn, iam)
}

/// The modelled `Tags` map (`map<string,string>`) for a stored tag set.
pub fn tags_to_wire(tags: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// A stored tag set from the modelled `Tags` map on an input.
pub fn tags_from_wire(
    tags: &Option<std::collections::HashMap<String, String>>,
) -> BTreeMap<String, String> {
    tags.iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// The region this request is addressed to: the SigV4 credential scope's region when the
/// request is signed, else [`DEFAULT_REGION`].
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Now, as the ISO-8601 instant MediaPackage reports in `CreatedAt`.
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    iso8601_of_secs(i64::try_from(secs).unwrap_or(0))
}

/// `secs` since the epoch, as the ISO-8601 instant this service writes an instant as.
///
/// The one rendering, so an instant a response reports ([`now_iso8601`]) and one a
/// harvested manifest states are written the same way.
pub fn iso8601_of_secs(secs: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The resource a tagging operation's `ResourceArn` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    Channel(String),
    OriginEndpoint(String),
}

impl TaggedResource {
    /// The resource's id, as the ARN spelled it.
    pub fn id(&self) -> &str {
        match self {
            Self::Channel(id) | Self::OriginEndpoint(id) => id,
        }
    }

    /// What to call this resource in a message to the caller.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Channel(_) => "Channel",
            Self::OriginEndpoint(_) => "OriginEndpoint",
        }
    }

    /// How an error body's `errors` map keys this resource — the resource type, as
    /// [`CHANNEL_KIND`] and [`ORIGIN_ENDPOINT_KIND`] name it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Channel(_) => CHANNEL_KIND,
            Self::OriginEndpoint(_) => ORIGIN_ENDPOINT_KIND,
        }
    }
}

/// The resource `arn` names, for a `ResourceArn` this service can tag.
///
/// A MediaPackage resource ARN is
/// `arn:<partition>:mediapackage:<region>:<account>:<type>/<id>` with `<type>` either
/// `channels` or `origin_endpoints` — the two forms [`channel_arn`] and
/// [`origin_endpoint_arn`] mint, and the only resources this service holds. `None` for
/// anything else: that is a malformed request, which is a different failure from an ARN
/// naming a resource the service does not hold.
///
/// This decides the resource TYPE and the id the ARN spells, and nothing else: whether
/// the ARN's partition, region and account name a resource this request can reach is
/// decided against the stored record by
/// [`ComAmazonawsMediapackageMediapackageState::tags`], because an ARN whose fields name
/// another Region or account is well-formed but resolves to nothing.
pub fn parse_tag_arn(arn: &str) -> Option<TaggedResource> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != SERVICE_NAME {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let (resource_type, id) = fields.next()?.split_once('/')?;
    if id.is_empty() {
        return None;
    }
    match resource_type {
        "channels" => Some(TaggedResource::Channel(id.to_string())),
        "origin_endpoints" => Some(TaggedResource::OriginEndpoint(id.to_string())),
        _ => None,
    }
}

/// `arn:aws:mediapackage:<region>:<account>:channels/<id>`.
pub fn channel_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("channels/{id}"))
}

/// `arn:aws:mediapackage:<region>:<account>:origin_endpoints/<id>`.
pub fn origin_endpoint_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("origin_endpoints/{id}"))
}

/// `arn:aws:mediapackage:<region>:<account>:harvest_jobs/<id>` — the ARN form
/// MediaPackage assigns a HarvestJob, whose resource segment is the operation's own URI
/// collection (`smithy_query(query="operation",operation="CreateHarvestJob",…)` binds
/// `POST /harvest_jobs`), as `channels` and `origin_endpoints` are above.
pub fn harvest_job_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(SERVICE_NAME, region, &format!("harvest_jobs/{id}"))
}

/// A stable, opaque 32-hex-character label derived from `parts`, used for the
/// account/resource labels in an ingest or playback hostname and path, and for the tag
/// that ties a page token to the enumeration that issued it ([`PageScope`]). Derived from
/// content (never from the clock or a random source) so the same resource always renders
/// the same URL on every later read.
pub fn stable_label(parts: &[&str]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

/// Which packaging configuration an OriginEndpoint was created with, and so which
/// manifest file its playback URL ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagingKind {
    Hls,
    Dash,
    Cmaf,
    Mss,
}

impl PackagingKind {
    /// The manifest path a packaging type's playback URL ends in, for `manifest_name`.
    ///
    /// The extensions are the manifest formats each packager emits: HLS and CMAF serve an
    /// HLS master playlist (`.m3u8`), DASH an MPD (`.mpd`), and Microsoft Smooth a
    /// server manifest (`.ism/Manifest`).
    pub fn manifest_path(self, manifest_name: &str) -> String {
        match self {
            Self::Hls => format!("{manifest_name}.m3u8"),
            Self::Dash => format!("{manifest_name}.mpd"),
            Self::Cmaf => format!("{manifest_name}/index.m3u8"),
            Self::Mss => format!("{manifest_name}.ism/Manifest"),
        }
    }
}

/// The single packaging configuration a create or update request carries.
///
/// An OriginEndpoint packages its channel in exactly ONE format — its playback URL ends
/// in that format's manifest — so a request naming several cannot be stored. `Ok(None)`
/// means the request named none: a create must refuse that, an update leaves the stored
/// packaging in place. `Err` carries the names supplied, for [`packaging_refusal`].
pub fn sole_packaging_kind(
    hls: bool,
    dash: bool,
    cmaf: bool,
    mss: bool,
) -> Result<Option<PackagingKind>, Vec<&'static str>> {
    let supplied: Vec<(PackagingKind, &'static str)> = [
        (PackagingKind::Hls, "hlsPackage", hls),
        (PackagingKind::Dash, "dashPackage", dash),
        (PackagingKind::Cmaf, "cmafPackage", cmaf),
        (PackagingKind::Mss, "mssPackage", mss),
    ]
    .into_iter()
    .filter_map(|(kind, name, present)| present.then_some((kind, name)))
    .collect();
    match supplied[..] {
        [] => Ok(None),
        [(kind, _)] => Ok(Some(kind)),
        _ => Err(supplied.iter().map(|(_, name)| *name).collect()),
    }
}

/// The refusal for a request that does not name exactly one packaging configuration,
/// naming the ones it did supply so the caller can fix the request from the error alone.
///
/// A rule about a COMBINATION of members, so it is keyed by the resource the request is
/// about rather than by one member ([`resource_invalid`]).
pub fn packaging_refusal(supplied: &[&str]) -> ServiceError {
    resource_invalid(
        ORIGIN_ENDPOINT_KIND,
        format!(
            "requires exactly one of hlsPackage, dashPackage, cmafPackage or mssPackage; got {}",
            if supplied.is_empty() {
                "none".to_string()
            } else {
                supplied.join(", ")
            }
        ),
    )
}

/// The host MediaPackage serves this account in `region` on: the account's own stable
/// label, then the regional service domain.
///
/// The single statement of that rule: every URL this service hands a caller — the ingest
/// URLs of a Channel and the playback URLs of an OriginEndpoint and its child manifests —
/// names this host, so they cannot come to name different ones.
pub fn media_hostname(region: &str) -> String {
    let account_label = stable_label(&[::emulator_server::emulator_account_id(), region]);
    format!("{account_label}.mediapackage.{region}.amazonaws.com")
}

/// The playback path every URL of ONE OriginEndpoint sits under: the egress host for this
/// account and Region, the egress path, then the endpoint's own path label.
///
/// The single statement of that rule. The endpoint's own `url` and the `url` of each child
/// manifest inside its CMAF package are this prefix plus their own tail, so they always
/// sit under one playback path.
pub fn playback_url_prefix(region: &str, channel_id: &str, endpoint_id: &str) -> String {
    let hostname = media_hostname(region);
    let endpoint_label = endpoint_path_label(channel_id, endpoint_id, region);
    format!(
        "https://{hostname}{}{endpoint_label}",
        data_plane::PLAYBACK_PATH_PREFIX
    )
}

/// The opaque label a CHANNEL is named by in the path of every URL this service publishes
/// for it — its ingest URLs ([`data_plane::ingest_path`]).
///
/// The single statement of that derivation: the channel's own id and the Region it was
/// created in, so the label names one channel of one Region and the data plane resolves
/// both from it ([`data_plane`]).
pub fn channel_path_label(channel_id: &str, region: &str) -> String {
    stable_label(&[channel_id, region])
}

/// The opaque label an ORIGIN ENDPOINT is named by in the path of its playback URLs.
///
/// The single statement of that derivation: the endpoint's own id, the channel it packages
/// and the Region it was created in.
pub fn endpoint_path_label(channel_id: &str, endpoint_id: &str, region: &str) -> String {
    stable_label(&[channel_id, endpoint_id, region])
}

/// The path ONE child HLS manifest of a CMAF package is served at, below its endpoint's
/// playback prefix: that child's own label, the name it carries, and the playlist.
///
/// The single statement of that path: [`cmaf_manifest_url`] publishes it and the playback
/// plane answers at it ([`data_plane::playback_content`]).
pub fn cmaf_manifest_path(
    channel_id: &str,
    endpoint_id: &str,
    region: &str,
    manifest_id: &str,
    manifest_name: &str,
) -> String {
    let manifest_label = stable_label(&[channel_id, endpoint_id, manifest_id, region]);
    format!("{manifest_label}/{manifest_name}/index.m3u8")
}

/// The playback URL for an OriginEndpoint: the endpoint's own playback path
/// ([`playback_url_prefix`]), then the manifest its packaging type serves under
/// `manifest_name`.
pub fn playback_url(
    region: &str,
    channel_id: &str,
    endpoint_id: &str,
    manifest_name: &str,
    kind: PackagingKind,
) -> String {
    let prefix = playback_url_prefix(region, channel_id, endpoint_id);
    let manifest = kind.manifest_path(manifest_name);
    format!("{prefix}/{manifest}")
}

/// The playback URL of one HLS manifest inside a CMAF package: the endpoint's own
/// playback path ([`playback_url_prefix`]), then that child manifest's own label and name.
pub fn cmaf_manifest_url(
    region: &str,
    channel_id: &str,
    endpoint_id: &str,
    manifest_id: &str,
    manifest_name: &str,
) -> String {
    let prefix = playback_url_prefix(region, channel_id, endpoint_id);
    let path = cmaf_manifest_path(channel_id, endpoint_id, region, manifest_id, manifest_name);
    format!("{prefix}/{path}")
}

/// How many hex characters a constant initialization vector is written as.
///
/// The model documents `CmafEncryption$ConstantInitializationVector` as "an optional
/// 128-bit, 16-byte hex value represented by a 32-character string"
/// (`smithy_query(query="doc",shape="com.amazonaws.mediapackage#CmafEncryption",member="ConstantInitializationVector",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`),
/// so the created value is 16 bytes rendered as this many hex characters.
pub const INITIALIZATION_VECTOR_CHARS: usize = 32;

/// The constant initialization vector the service creates for the CMAF encryption
/// configuration of ONE OriginEndpoint, for a request that supplied none.
///
/// The model states what the service does when the caller omits this member — "if you
/// don't specify a value, then MediaPackage creates the constant initialization vector
/// (IV)" — so the value is one the SERVICE supplies, not a default constant: it is
/// materialised at write time like the endpoint's ARN and playback URLs, and so every
/// projection of the stored endpoint reports it. The HLS sibling documents no created
/// value ("when not specified the initialization vector will be periodically rotated"), so
/// no block other than the CMAF one gains one.
///
/// Derived from the endpoint's own identity with [`stable_label`], which renders exactly
/// 16 bytes as [`INITIALIZATION_VECTOR_CHARS`] hex characters — the member's documented
/// form — so one endpoint's IV is its own and never moves between reads or between writes
/// that do not resend the encryption block.
pub fn created_initialization_vector(region: &str, channel_id: &str, endpoint_id: &str) -> String {
    stable_label(&["cmaf-encryption-iv", channel_id, endpoint_id, region])
}

/// The `CmafPackage` to store for the `CmafPackageCreateOrUpdateParameters` an input
/// carried.
///
/// The create/update parameter shape differs from the stored shape in exactly four ways,
/// all of them values the SERVICE supplies: each child HLS manifest gains the playback
/// `Url` derived for it, a child manifest sent without a `ManifestName` takes its
/// documented default — the endpoint's own effective `ManifestName`, which
/// `endpoint_manifest_name` carries already resolved — an absent `SegmentPrefix` takes
/// its documented default, the ChannelId, and an `Encryption` block sent without a
/// `ConstantInitializationVector` gains the one the service creates for this endpoint
/// ([`created_initialization_vector`]).
pub fn stored_cmaf_package(
    params: &CmafPackageCreateOrUpdateParameters,
    region: &str,
    channel_id: &str,
    endpoint_id: &str,
    endpoint_manifest_name: &str,
) -> CmafPackage {
    let hls_manifests = params
        .hls_manifests
        .iter()
        .flatten()
        .map(|manifest| {
            let manifest_name = manifest
                .manifest_name
                .clone()
                .unwrap_or_else(|| endpoint_manifest_name.to_string());
            HlsManifest {
                ad_markers: manifest.ad_markers.clone(),
                ad_triggers: manifest.ad_triggers.clone(),
                ads_on_delivery_restrictions: manifest.ads_on_delivery_restrictions.clone(),
                include_iframe_only_stream: manifest.include_iframe_only_stream,
                playlist_type: manifest.playlist_type.clone(),
                playlist_window_seconds: manifest.playlist_window_seconds,
                program_date_time_interval_seconds: manifest.program_date_time_interval_seconds,
                url: Some(cmaf_manifest_url(
                    region,
                    channel_id,
                    endpoint_id,
                    &manifest.id,
                    &manifest_name,
                )),
                manifest_name: Some(manifest_name),
                id: manifest.id.clone(),
            }
        })
        .collect();
    CmafPackage {
        encryption: params.encryption.clone().map(|mut encryption| {
            encryption.constant_initialization_vector = encryption
                .constant_initialization_vector
                .or_else(|| {
                    Some(created_initialization_vector(
                        region, channel_id, endpoint_id,
                    ))
                });
            encryption
        }),
        hls_manifests: Some(hls_manifests),
        segment_duration_seconds: params.segment_duration_seconds,
        segment_prefix: Some(
            params
                .segment_prefix
                .clone()
                .unwrap_or_else(|| channel_id.to_string()),
        ),
        stream_selection: params.stream_selection.clone(),
    }
}

/// Re-derive the endpoint's stored CMAF package from the parameters its last write
/// carried, for the endpoint's CURRENT effective `ManifestName`.
///
/// The members of a stored child HLS manifest that the SERVICE supplies — the name an
/// unnamed child inherits from the endpoint and the playback `Url` built from whichever
/// name the child ends up with — are derived from the endpoint's own effective
/// `ManifestName`. That name is decided afresh by every write of the endpoint, so a write
/// that does not resend the packaging configuration still moves what the children are
/// derived FROM; calling this after the record's `ManifestName` is settled is what keeps
/// the stored children, and therefore every later read of them, derived from the name the
/// endpoint actually reports. An endpoint that stores no CMAF package keeps none.
pub fn rederive_cmaf_package(record: &mut OriginEndpointRecord) {
    let manifest_name = record
        .manifest_name
        .clone()
        .unwrap_or_else(|| DEFAULT_MANIFEST_NAME.to_string());
    let package = record.cmaf_package_parameters.as_ref().map(|params| {
        stored_cmaf_package(
            params,
            &record.region,
            &record.channel_id,
            &record.id,
            &manifest_name,
        )
    });
    record.cmaf_package = package;
}

/// How many characters the tag half of a page token is: the width [`stable_label`]
/// renders.
const PAGE_TOKEN_TAG_CHARS: usize = 32;

/// The longest `nextToken` this service could ever have issued: the longest key a record
/// is stored under, hex-encoded at two characters a byte, plus the tag.
///
/// A longer string is refused before anything is decoded from it, so a caller cannot turn
/// an unbounded `nextToken` into work this service does.
const MAX_PAGE_TOKEN_CHARS: usize = MAX_STRING_CHARS * 2 + PAGE_TOKEN_TAG_CHARS;

/// Which enumeration a `nextToken` belongs to: the listing operation that would mint it,
/// the Region the request is served in, and the filters that decide which records the
/// listing walks at all.
///
/// A page cursor is not a free-standing position in the store but a position in ONE
/// enumeration — one collection's records, of one Region, selected by one set of filters,
/// in key order. All three decide WHICH records are walked, so all three are part of the
/// enumeration's identity: a cursor into a filtered walk points at the next MATCHING
/// record, which is not where the same key sits in any other walk. Carrying that identity
/// in the token is what lets [`Self::resume_key`] tell a cursor this listing minted from
/// any other string a caller could send, and what keeps a cursor of one enumeration from
/// slicing another.
#[derive(Debug, Clone)]
pub struct PageScope {
    /// The enumeration's identity, as the token's tag is derived from it. Rendered once
    /// at construction, with every part length-prefixed, so no two different
    /// operation/Region/filter sets can render the same identity whatever characters a
    /// caller puts in a filter value.
    identity: String,
}

impl PageScope {
    /// The enumeration `operation` serves in `region`, selecting records with `filters`:
    /// each filter's wire name, and the value this request gave it (`None` when the
    /// request left it out, which selects a different set of records from any value).
    pub fn new(operation: &str, region: &str, filters: &[(&str, Option<&str>)]) -> Self {
        let mut identity = String::new();
        let mut push = |part: &str| {
            identity.push_str(&format!("{}:{part}", part.len()));
        };
        push(operation);
        push(region);
        for (name, value) in filters {
            push(name);
            match value {
                Some(value) => push(&format!("={value}")),
                None => push("*"),
            }
        }
        Self { identity }
    }

    /// The tag that ties a token to THIS enumeration and the key it resumes at.
    ///
    /// Derived from the enumeration and the key alone (never from the clock or a random
    /// source), so the same cursor always renders the same token and a token this service
    /// issued still verifies on a later call.
    fn tag(&self, key: &str) -> String {
        stable_label(&["nextToken", &self.identity, key])
    }

    /// Mint the token for the page starting at `next_key`: the key, plus the tag that
    /// shows this enumeration issued it.
    ///
    /// Opaque to the caller, and resolved against the `BTreeMap` ordering, so a token
    /// taken from one call resumes exactly where that call stopped even if records were
    /// added or removed in between.
    pub fn issue(&self, next_key: &str) -> String {
        format!(
            "{}{}",
            hex::encode(next_key.as_bytes()),
            self.tag(next_key)
        )
    }

    /// The key a caller-supplied `nextToken` resumes this enumeration at, or
    /// [`invalid_page_token`] for a token this enumeration did not issue — which is told
    /// to the caller rather than silently served as an arbitrary slice of the collection.
    ///
    /// `Ok(None)` is a request carrying no token at all: page one.
    pub fn resume_key(&self, next_token: Option<&str>) -> Result<Option<String>, ServiceError> {
        next_token
            .map(|token| self.decode(token).ok_or_else(invalid_page_token))
            .transpose()
    }

    /// The key `token` names, when `token` is one this enumeration minted.
    fn decode(&self, token: &str) -> Option<String> {
        if token.len() > MAX_PAGE_TOKEN_CHARS
            || token.len() < PAGE_TOKEN_TAG_CHARS
            || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        // Every character is ASCII, so the split is on a character boundary.
        let (key_hex, tag) = token.split_at(token.len() - PAGE_TOKEN_TAG_CHARS);
        let key = String::from_utf8(hex::decode(key_hex).ok()?).ok()?;
        (self.tag(&key) == tag).then_some(key)
    }
}

/// How many items one `List*` call serves, for the `MaxResults` it was given.
///
/// A `MaxResults` outside the bound this service enforces is refused before the handler
/// runs ([`page_size_violation`]), so a value reaching here is already inside
/// [`MIN_PAGE_SIZE`]..=[`MAX_PAGE_SIZE`]; the clamp states that invariant locally instead
/// of leaving it implicit, and an absent `MaxResults` takes the service's page cap, which
/// is the most it would serve for any value the caller could send.
pub fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        None => MAX_PAGE_SIZE,
        Some(max) => usize::try_from(max)
            .unwrap_or(MIN_PAGE_SIZE)
            .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
    }
}

/// One page of `BTreeMap` keys: up to `page_size` keys at or after `start`, plus the
/// token the next page would be resumed with when (and only when) more remain.
///
/// The token is minted by `scope`, the enumeration these keys come from, so the only
/// cursor a later call can resume at is one this listing itself handed out.
pub fn paginate<'a, I>(
    keys: I,
    scope: &PageScope,
    start: Option<&str>,
    page_size: usize,
) -> (Vec<&'a str>, Option<String>)
where
    I: IntoIterator<Item = &'a str>,
{
    let mut remaining = keys
        .into_iter()
        .filter(|key| start.is_none_or(|from| *key >= from));
    let page: Vec<&'a str> = remaining.by_ref().take(page_size).collect();
    let next_token = remaining.next().map(|key| scope.issue(key));
    (page, next_token)
}
