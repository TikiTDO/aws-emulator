// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal: the modelled error bodies [`super`]'s handlers
//! answer with, and the one envelope every validation message is stated in.
//!
//! Split out of [`super`] so the module root holds the state and the rules while this
//! file holds the sentences they refuse with; every constructor keeps the rule it was
//! written with — a refusal depends only on WHAT was refused, never on the identifiers
//! the caller sent.

use super::*;

/// One kind of resource in mediapackagev2's containment hierarchy: channel group ->
/// channel -> origin endpoint -> harvest job.
///
/// The variants are the `ResourceTypeNotFound` enum values the model declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// The top-level resource, which owns channels.
    ChannelGroup,
    /// A channel inside a channel group, which owns origin endpoints.
    Channel,
    /// An origin endpoint on a channel.
    OriginEndpoint,
    /// A harvest job on an origin endpoint.
    HarvestJob,
}

impl ResourceKind {
    /// This kind's `ResourceTypeNotFound` value (model shape `ResourceTypeNotFound`,
    /// whose values are `CHANNEL_GROUP`, `CHANNEL`, `ORIGIN_ENDPOINT`, `HARVEST_JOB`).
    pub fn resource_type_not_found(self) -> &'static str {
        match self {
            Self::ChannelGroup => "CHANNEL_GROUP",
            Self::Channel => "CHANNEL",
            Self::OriginEndpoint => "ORIGIN_ENDPOINT",
            Self::HarvestJob => "HARVEST_JOB",
        }
    }

    /// How a mediapackagev2 message names this kind in prose.
    pub fn noun(self) -> &'static str {
        match self {
            Self::ChannelGroup => "channel group",
            Self::Channel => "channel",
            Self::OriginEndpoint => "origin endpoint",
            Self::HarvestJob => "harvest job",
        }
    }

    /// The plural of [`noun`](Self::noun), for a sentence about this kind's children.
    pub fn plural(self) -> String {
        format!("{}s", self.noun())
    }

    /// The indefinite article [`noun`](Self::noun) takes — `an` before a vowel.
    fn article(self) -> &'static str {
        match self.noun().starts_with(['a', 'e', 'i', 'o', 'u']) {
            true => "an",
            false => "a",
        }
    }

    /// How a not-found sentence asks the caller to check this kind, and what it tells them
    /// to do about it: the identifier the resource is ADDRESSED by, and the verb that
    /// brings one into being.
    ///
    /// A harvest job is addressed by an id and is created; every other kind of this
    /// hierarchy is addressed by a name and is added. The recorded `GetHarvestJob` and
    /// `CancelHarvestJob` 404s word it that way, and the other kinds' recorded 404s word it
    /// the other, so the sentence is a property of the KIND rather than one phrasing for
    /// all of them.
    fn not_found_remedy(self) -> (&'static str, &'static str) {
        match self {
            Self::HarvestJob => ("id", "create"),
            Self::ChannelGroup | Self::Channel | Self::OriginEndpoint => ("name", "add"),
        }
    }

    /// The 404 for a resource of this kind the service cannot find.
    ///
    /// The sentence names the KIND and nothing else: a caller's resource names are not
    /// echoed back, so two callers asking for two different absent channels read the
    /// same message.
    pub fn not_found(self) -> ResourceNotFoundException {
        let noun = self.noun();
        let article = self.article();
        let (identifier, remedy) = self.not_found_remedy();
        ResourceNotFoundException {
            message: Some(format!(
                "MediaPackage can't process your request because we can't find your {noun}. \
                 Verify your {noun} {identifier} or {remedy} {article} {noun} and then try \
                 again"
            )),
            resource_type_not_found: Some(self.resource_type_not_found().to_string()),
        }
    }
}

/// The 409 a delete answers while the resource it names still holds children.
///
/// `RESOURCE_IN_USE`: the deletion would orphan the children, so the service names the
/// child kind that has to go first — not how many of them there are, which is a fact
/// about the caller's state rather than about the refusal.
pub fn children_attached(parent: ResourceKind, child: ResourceKind) -> ConflictException {
    let parent_noun = parent.noun();
    let children = child.plural();
    ConflictException {
        message: Some(format!(
            "The {parent_noun} you tried to delete has {children} attached to it. \
             If you want to delete this {parent_noun}, you must first delete the \
             {children} attached to it"
        )),
        conflict_exception_type: Some("RESOURCE_IN_USE".to_string()),
    }
}

/// The 409 a delete answers while another resource still REFERENCES the one it names —
/// a channel that a multiview channel of the same group lists among its available
/// sources.
///
/// `RESOURCE_IN_USE`, the same discriminator [`children_attached`] uses: a reference is
/// a use of the resource just as a child is, and the deletion would leave the referrer
/// pointing at nothing. The sentence names the kind that has to be updated first and,
/// like every refusal here, neither the caller's names nor how many referrers there are.
pub fn multiview_source_in_use() -> ConflictException {
    ConflictException {
        message: Some(
            "The channel you tried to delete is listed as an available source by a \
             multiview channel in the same channel group. If you want to delete this \
             channel, you must first update the multiview channels that list it as an \
             available source"
                .to_string(),
        ),
        conflict_exception_type: Some("RESOURCE_IN_USE".to_string()),
    }
}

/// The 409 an update answers while a harvest job that has not finished still harvests a
/// manifest the update would stop the origin endpoint publishing.
///
/// `RESOURCE_IN_USE`, the same discriminator [`children_attached`] and
/// [`multiview_source_in_use`] use: a sub-resource that references the manifests is a use
/// of them, and the write would leave that reference pointing at nothing. The sentence
/// names what has to change first and, like every refusal here, neither the caller's names
/// nor how many references there are.
pub fn harvested_manifest_in_use() -> ConflictException {
    ConflictException {
        message: Some(
            "The origin endpoint you tried to update publishes manifests that a harvest \
             job which hasn't finished is harvesting. If you want to update this origin \
             endpoint, you must first cancel the harvest jobs that harvest the manifests \
             you're removing"
                .to_string(),
        ),
        conflict_exception_type: Some("RESOURCE_IN_USE".to_string()),
    }
}

/// The 409 a create answers when the name it was given is already taken.
///
/// `RESOURCE_ALREADY_EXISTS`, and resource-agnostic: mediapackagev2 states the collision
/// without naming either the resource kind or the name that collided, so a client's
/// idempotency logic matches one sentence for every create path.
pub fn resource_already_exists() -> ConflictException {
    ConflictException {
        message: Some(
            "MediaPackage can't create the resource you requested because a resource with \
             that name already exists. Verify the name of the resource and then try again"
                .to_string(),
        ),
        conflict_exception_type: Some("RESOURCE_ALREADY_EXISTS".to_string()),
    }
}

/// The 402 a create answers when the collection it would grow is already at its ceiling.
///
/// `ServiceQuotaExceededException` declares no discriminator member, so the sentence is
/// the whole refusal: it names the KIND whose quota is reached and, like every refusal
/// here, neither the caller's names nor the quota's value — which is a fact about the
/// account rather than about the refusal.
pub fn quota_exceeded(kind: ResourceKind) -> ServiceQuotaExceededException {
    let plural = kind.plural();
    ServiceQuotaExceededException {
        message: Some(format!(
            "MediaPackage can't create the resource you requested because you've reached \
             the quota for {plural}. Delete {plural} you no longer need, or request a \
             quota increase, and then try again"
        )),
    }
}

/// The 404 for a policy the service cannot find because none is attached to the resource
/// it would belong to — a channel or origin endpoint that exists with no policy on it.
///
/// The resource itself is present, and `ResourceTypeNotFound` enumerates only resource
/// types (`CHANNEL_GROUP`, `CHANNEL`, `ORIGIN_ENDPOINT`, `HARVEST_JOB`), so the
/// discriminator is left unset rather than naming a resource that is NOT missing. AWS
/// publishes no wording for this case: what the service requires is the 404 and the
/// error code, and the sentence follows the same rule as every other refusal here — one
/// fixed sentence, naming neither the resource the caller addressed nor its kind, since
/// what is missing is the policy either way.
pub fn policy_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(
            "MediaPackage can't process your request because we can't find your policy. \
             Attach a policy to your resource and then try again"
                .to_string(),
        ),
        resource_type_not_found: None,
    }
}

/// The modeled 400 for one constraint the request violated.
///
/// Both members are set, and both are derived from the violation rather than from the
/// request's values: `ValidationExceptionType` is the discriminator for the violated
/// constraint CLASS, and `Message` names the member path that failed and restates the
/// constraint it failed ([`constraint_detail`], which also words the framework's own
/// refusal fallback, so one violation cannot be worded two ways). A caller can therefore
/// branch on the type and repair the named member from the sentence alone.
pub fn member_validation_failed(violation: &Violation) -> ValidationException {
    validation_failed(
        &violation.path,
        &constraint_detail(&violation.kind),
        validation_exception_type(&violation.kind),
    )
}

/// The modeled 400 for a member this service refuses, in the envelope every
/// mediapackagev2 validation message uses ([`validation_message`]): the member path the
/// request named it at, then the requirement it failed.
///
/// A rule the service enforces itself (a policy document it cannot parse, a pagination
/// token it did not issue) is therefore stated exactly like a constraint the model
/// declares, so a caller reads one form for both.
pub fn validation_failed(
    member_path: &str,
    requirement: &str,
    exception_type: &str,
) -> ValidationException {
    cross_member_validation_failed(Some(member_path), requirement, exception_type)
}

/// The modeled 400 for a rule about the RELATIONSHIP between several members of the
/// request rather than about any one of them — and the ONE builder of this service's
/// `ValidationException` body, which [`validation_failed`] delegates to, so the envelope
/// is assembled in one place however the refusal was found.
///
/// Such a refusal is stated at the member that CONTAINS the members the rule relates —
/// the structure they are both in (`request.scheduleConfiguration` for a rule relating
/// `ScheduleConfiguration`'s start and end), or the request itself when they are the
/// request's own members (`request` for a rule relating `ContainerType` to a manifest
/// list). Naming one of the related members instead would point a caller that reads the
/// member path at a value that is acceptable on its own, so the path names the smallest
/// part of the request the caller has to reconsider as a whole.
///
/// `container` is `None` for members the request itself holds, which
/// [`validation_message`] states as the request.
pub fn cross_member_validation_failed(
    container: Option<&str>,
    requirement: &str,
    exception_type: &str,
) -> ValidationException {
    ValidationException {
        message: Some(validation_message(container, requirement)),
        validation_exception_type: Some(exception_type.to_string()),
    }
}

/// The `ValidationExceptionType` value for a violated constraint class.
///
/// The model's `ValidationExceptionType` enum carries one `MEMBER_*` value per Smithy
/// constraint class — `MEMBER_MISSING`, `MEMBER_MIN_LENGTH`, `MEMBER_MAX_LENGTH`,
/// `MEMBER_MIN_VALUE`, `MEMBER_MAX_VALUE`, `MEMBER_DOES_NOT_MATCH_PATTERN`,
/// `MEMBER_INVALID_ENUM_VALUE` — plus `MEMBER_INVALID` for a member that is wrong in a
/// way none of the others names. This is that mapping and nothing more.
fn validation_exception_type(kind: &ViolationKind) -> &'static str {
    match kind {
        ViolationKind::MissingRequiredMember => "MEMBER_MISSING",
        ViolationKind::MinLength(_) => "MEMBER_MIN_LENGTH",
        ViolationKind::MaxLength(_) => "MEMBER_MAX_LENGTH",
        ViolationKind::MinValue(_) => "MEMBER_MIN_VALUE",
        ViolationKind::MaxValue(_) => "MEMBER_MAX_VALUE",
        ViolationKind::Pattern(_) => "MEMBER_DOES_NOT_MATCH_PATTERN",
        ViolationKind::EnumValue(_) => "MEMBER_INVALID_ENUM_VALUE",
        // `@uniqueItems`, and any constraint class added to the framework later: the
        // member is invalid, which is all the enum can say about it.
        _ => "MEMBER_INVALID",
    }
}

// =============================================================================
// Validation: how this service words a refused request
// =============================================================================

/// The sentence mediapackagev2 appends when the caller has to look at a value it sent.
pub(super) const CHECK_THE_VALUE: &str =
    "Check the value you specified in your request and then try again";

/// How mediapackagev2 states an input failure: the fixed prefix, the member path the
/// failure is at (in lowerCamel, as the wire names it), then what is wrong with it.
///
/// Every `ValidationException` this service raises goes through here, so the whole
/// surface reports a refusal in one form — `Validation error encountered at
/// request.<member>: <rule>` — rather than one form at the request boundary and another
/// from the handlers. `member_path` is `None` for a rule that is about the request as a
/// whole rather than about one member.
pub fn validation_message(member_path: Option<&str>, detail: &str) -> String {
    match member_path {
        Some(path) => format!("Validation error encountered at request.{path}: {detail}"),
        None => format!("Validation error encountered at request: {detail}"),
    }
}

/// The requirement a declared-constraint violation states, worded as this service words
/// it: the violated bound, then the sentence telling the caller to check what it sent.
///
/// Reached from both refusal paths — the modelled `ValidationException` this service
/// serializes for a constraint violation ([`member_validation_failed`])
/// and the framework's own refusal wording ([`refusal_message`]) — so one violation
/// cannot be worded two ways.
pub fn constraint_detail(kind: &ViolationKind) -> String {
    let requirement = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        // A `@pattern` that is one character class is stated as the characters it
        // ADMITS, which is what the caller has to act on; a pattern with no listable
        // character set is stated as the pattern itself.
        ViolationKind::Pattern(pattern) => match pattern_character_set(pattern) {
            Some(supported) => format!(
                "Member must contain only supported characters. Supported characters are \
                 {supported}"
            ),
            None => format!("Member must satisfy regular expression pattern: {pattern}"),
        },
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class added later still
        // gets this service's form rather than falling back to the framework's.
        _ => "Member must satisfy the constraints specified by the AWS service".to_string(),
    };
    format!("{requirement}. {CHECK_THE_VALUE}")
}

/// The characters a `@pattern` of the form `^[<class>]+$` admits, listed the way a
/// MediaPackage validation message lists them — `^[a-zA-Z0-9_-]+$` becomes
/// `A-Z, a-z, 0-9, _ (underscore), and - (hyphen)`.
///
/// `None` when the pattern is not a single, non-negated character class (the model's
/// `^[\S]+$`, say): there is no set of characters to list, so the caller states the
/// pattern itself instead.
///
/// Deliberately a RENDERER for message wording and nothing else: it reads the model's
/// own `@pattern` string, never caller data, and must never be used to decide whether a
/// value is acceptable — that is the framework's constraint walk, which compiles the
/// pattern with the `regex` crate. Anything it cannot describe it declines.
pub(super) fn pattern_character_set(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('^')?.strip_suffix('$')?;
    let class = body
        .strip_suffix('+')
        .or_else(|| body.strip_suffix('*'))
        .unwrap_or(body);
    let class = class.strip_prefix('[')?.strip_suffix(']')?;
    // A negated class names what is FORBIDDEN, and an escape (`\S`, `\w`) names a set
    // with no listable members, so neither can be rendered as "supported characters".
    if class.starts_with('^') || class.contains('\\') || class.is_empty() {
        return None;
    }
    let chars: Vec<char> = class.chars().collect();
    let mut ranges: Vec<String> = Vec::new();
    let mut literals: Vec<char> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        // `X-Y` is a range; a `-` with nothing after it is the literal hyphen.
        if index + 2 < chars.len() && chars[index + 1] == '-' {
            // A range whose ends are not one run of letters or digits (`[_-x]`, which
            // regex reads as the code points between them) has no listing a message
            // could state, so the whole pattern is left to its own rendering rather
            // than described as something narrower than it is.
            if !is_listable_range(chars[index], chars[index + 2]) {
                return None;
            }
            ranges.push(format!("{}-{}", chars[index], chars[index + 2]));
            index += 3;
        } else {
            literals.push(chars[index]);
            index += 1;
        }
    }
    // The message lists the ranges in a canonical order — upper case, then lower case,
    // then digits — rather than in the order the pattern happens to write them.
    ranges.sort_by_key(|range| match range.chars().next() {
        Some(first) if first.is_ascii_uppercase() => 0,
        Some(first) if first.is_ascii_lowercase() => 1,
        Some(first) if first.is_ascii_digit() => 2,
        _ => 3,
    });
    let items: Vec<String> = ranges
        .into_iter()
        .chain(literals.into_iter().map(named_character))
        .collect();
    Some(join_with_and(&items))
}

/// Whether `start`-`end` is a range a message can state as itself: one run of upper-case
/// letters, lower-case letters or digits, in ascending order. Anything else is a range
/// over code points that happens to be writable in a class, and listing it would claim
/// more or less than the pattern admits.
fn is_listable_range(start: char, end: char) -> bool {
    let same_run = (start.is_ascii_uppercase() && end.is_ascii_uppercase())
        || (start.is_ascii_lowercase() && end.is_ascii_lowercase())
        || (start.is_ascii_digit() && end.is_ascii_digit());
    same_run && start <= end
}

/// One literal character as a message names it: the character, plus the word for it
/// where it has one, because `_` and `-` are not legible on their own.
fn named_character(character: char) -> String {
    match character {
        '_' => "_ (underscore)".to_string(),
        '-' => "- (hyphen)".to_string(),
        '.' => ". (period)".to_string(),
        ' ' => "a space".to_string(),
        other => other.to_string(),
    }
}

/// Join `items` as prose: `a`, `a and b`, `a, b, and c`.
fn join_with_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [leading @ .., last] => format!("{}, and {last}", leading.join(", ")),
    }
}

/// This service's wording for a refusal the request boundary made from the model's own
/// constraints — a `@range`, `@length`, `@pattern` or enum violation, or an absent
/// required member — before any handler ran.
///
/// mediapackagev2 names ONE member path per message, so the first violation is the one
/// stated; the framework's default lists them all in its own `N validation errors
/// detected: ...` form, which no mediapackagev2 response carries.
pub fn refusal_message(violations: &[Violation]) -> String {
    match violations.first() {
        Some(violation) => validation_message(
            Some(&violation.path),
            &constraint_detail(&violation.kind),
        ),
        // The boundary refused the request without naming a violation; there is no
        // member path to state, so the message is about the request as a whole.
        None => validation_message(
            None,
            "The input failed to meet the constraints specified by the AWS service",
        ),
    }
}
