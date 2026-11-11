// SPDX-License-Identifier: CC0-1.0

//! The account's TOPIC RULE DESTINATIONS: the record one holds, the state machine the
//! four single-destination operations share, and the CONFIRMATION CHALLENGE this service
//! sends to the URL a destination names.
//!
//! A topic rule destination is the endpoint a rule's `http` action is allowed to send
//! traffic to, and `CreateTopicRuleDestination` documents "The destination must be
//! confirmed prior to use". `TopicRuleDestination$status` states the whole machine:
//!
//! * `IN_PROGRESS` — "A topic rule destination was created but has not been confirmed.
//!   You can set `status` to `IN_PROGRESS` by calling `UpdateTopicRuleDestination`.
//!   Calling `UpdateTopicRuleDestination` causes a new confirmation challenge to be sent
//!   to your confirmation endpoint." So a create answers in this status, and a request
//!   for it again is a request for a FRESH challenge.
//! * `DISABLED` — "Confirmation was completed, and traffic to this destination is not
//!   allowed. You can set `status` to `ENABLED` by calling
//!   `UpdateTopicRuleDestination`." This is where a confirmed destination RESTS: nothing
//!   but an explicit update enables it.
//! * `ENABLED` — "Confirmation was completed, and traffic to this destination is
//!   allowed. You can set `status` to `DISABLED` by calling
//!   `UpdateTopicRuleDestination`."
//! * `ERROR` — "Confirmation could not be completed … You can set `status` to
//!   `IN_PROGRESS`."
//! * `DELETING` — the status a destination being removed holds; no caller asks for it.
//!
//! The confirmation itself is the one thing a client cannot drive: the service sends a
//! `DestinationConfirmation` request to the `confirmationUrl` the configuration named,
//! carrying a [`CONFIRMATION_TOKEN_QUERY_PARAMETER`] the owner of that endpoint hands
//! back through `ConfirmTopicRuleDestination`. The URL a create may name is held to this
//! module's own policy ([`unchallengeable_confirmation_url`]) and the challenge is sent over
//! the emulator's own IN-PROCESS transport and nothing else ([`InProcessTransport`]), so a
//! URL a caller configured reaches only what this emulator serves itself: no socket is
//! opened, no host name is resolved and no redirect is followed.
//!
//! Leaving `IN_PROGRESS` is a transition the CLOCK owes, not the confirming call: AWS
//! documents the destination as leaving that status asynchronously once the token comes
//! back, so the confirming call records WHEN the move is due ([`CONFIRMATION_SPAN`]) and
//! every later reader materialises it ([`RegisteredTopicRuleDestination::settle`]) — the
//! destination is `DISABLED` by then whether or not anybody called.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::aws_smithy_runtime_api::client::http::{HttpConnector, SharedHttpConnector};

use crate::ops::types::{HttpAction, HttpUrlDestinationProperties, TopicRuleDestination};

/// `TopicRuleDestinationStatus$IN_PROGRESS`: created, and not confirmed.
pub const DESTINATION_IN_PROGRESS: &str = "IN_PROGRESS";

/// `TopicRuleDestinationStatus$DISABLED`: confirmed, and traffic not allowed.
pub const DESTINATION_DISABLED: &str = "DISABLED";

/// `TopicRuleDestinationStatus$ENABLED`: confirmed, and traffic allowed.
pub const DESTINATION_ENABLED: &str = "ENABLED";

/// `TopicRuleDestinationStatus$ERROR`: the confirmation could not be completed.
pub const DESTINATION_ERROR: &str = "ERROR";

/// `TopicRuleDestinationStatus$DELETING`: the destination is being removed. It is the
/// service's own status for work it is doing, so a caller asking for it is refused.
pub const DESTINATION_DELETING: &str = "DELETING";

/// How long after `ConfirmTopicRuleDestination` the destination leaves `IN_PROGRESS` and
/// rests at [`DESTINATION_DISABLED`].
///
/// The destination is answered in a NON-TERMINAL status, so the clock owes the move: the
/// span is the emulator's own — AWS publishes no figure — chosen so a caller reading
/// straight after the confirming call still observes `IN_PROGRESS`, and the due instant is
/// recorded rather than slept for.
pub const CONFIRMATION_SPAN: Duration = Duration::from_secs(3);

/// How many topic rule destinations this emulator keeps for the account.
///
/// The store is one a CALLER grows — one record per `CreateTopicRuleDestination` — so it
/// is bounded for the same reason [`super::MAX_TOPIC_RULES`] is: past the ceiling nothing
/// is recorded and the create is refused with a modelled error rather than growing
/// without bound.
pub const MAX_TOPIC_RULE_DESTINATIONS: usize = 1_000;

/// The resource type an HTTP topic rule destination's ARN carries, under which the
/// destination's own identifier is minted.
const HTTP_DESTINATION_RESOURCE: &str = "ruledestination/http";

/// The ARN of an HTTP topic rule destination:
/// `arn:aws:iot:<region>:<account>:ruledestination/http/<id>`.
///
/// The destination is addressed by ARN alone — the model binds it as the `@httpLabel` of
/// `/destinations/{arn+}` and as the `@required` member of the update — so the id is
/// minted here and never taken from the request; the namespace and account segments are
/// the ones [`super::thing_arn`] documents.
pub fn topic_rule_destination_arn(region: &str, destination_id: &str) -> String {
    ::emulator_server::emulator_arn(
        "iot",
        region,
        &format!("{HTTP_DESTINATION_RESOURCE}/{destination_id}"),
    )
}

/// How every operation that addresses ONE topic rule destination refuses an `arn` this
/// account holds no destination under: the family's fixed text, with the ARN taken from
/// the request.
pub fn missing_topic_rule_destination_message(arn: &str) -> String {
    format!("Topic rule destination {arn} not found")
}

/// How `ConfirmTopicRuleDestination` refuses a token that is no destination's.
///
/// The token is the whole request, so there is no resource name to qualify the sentence
/// with — and the token itself is not echoed: a caller that sent the wrong one learns
/// that it was not accepted, not what this service holds.
pub const INVALID_CONFIRMATION_TOKEN_MESSAGE: &str = "Invalid confirmation token";

/// How an update refuses a `status` the destination cannot be moved to from the one it
/// holds: the two statuses, in the order the sentence states them.
///
/// The statuses are the destination's current one and the requested one, both read from
/// this account's state and the request — never from any one call.
pub fn invalid_status_transition_message(from: &str, to: &str) -> String {
    format!("Topic rule destination status cannot be changed from {from} to {to}")
}

/// One topic rule destination in the destinations store, as this service persists it.
///
/// One record carries everything a reader hands back, so `GetTopicRuleDestination` after
/// `CreateTopicRuleDestination` reports the created destination member for member: the
/// reader clones what the writer stored and the two cannot drift.
#[derive(Debug, Clone)]
pub struct RegisteredTopicRuleDestination {
    /// The destination's ARN ([`topic_rule_destination_arn`]) — its identity, and its key
    /// in the store.
    pub arn: String,
    /// `HttpUrlDestinationConfiguration$confirmationUrl`, "The URL IoT uses to confirm
    /// ownership of or access to the topic rule destination URL", reported back as
    /// `HttpUrlDestinationProperties$confirmationUrl`.
    pub confirmation_url: String,
    /// The token the outstanding confirmation challenge carries, which
    /// `ConfirmTopicRuleDestination` accepts. A fresh challenge mints a fresh one, so a
    /// token a resend superseded no longer confirms anything.
    pub confirmation_token: String,
    /// `TopicRuleDestination$status`.
    pub status: String,
    /// `TopicRuleDestination$statusReason`, "Additional details or reason why the topic
    /// rule destination is in the current status" — set only where there is something to
    /// say that the status does not already say.
    pub status_reason: Option<String>,
    /// `TopicRuleDestination$createdAt`.
    pub created_at: SystemTime,
    /// `TopicRuleDestination$lastUpdatedAt`, moved by every write to the record.
    pub last_updated_at: SystemTime,
    /// When the confirmed destination's move out of `IN_PROGRESS` comes due, for one that
    /// has been confirmed and has not got there yet; `None` for a destination with no
    /// owed transition. See [`CONFIRMATION_SPAN`].
    pub confirmed_due_at: Option<SystemTime>,
}

impl RegisteredTopicRuleDestination {
    /// Record an HTTP destination created at `now` for `confirmation_url`, with
    /// `confirmation_token` as its outstanding challenge.
    ///
    /// It enters the lifecycle at [`DESTINATION_IN_PROGRESS`] — "a topic rule destination
    /// was created but has not been confirmed" — which is the status the create answers
    /// with.
    pub fn new(
        arn: String,
        confirmation_url: String,
        confirmation_token: String,
        now: SystemTime,
    ) -> Self {
        Self {
            arn,
            confirmation_url,
            confirmation_token,
            status: DESTINATION_IN_PROGRESS.to_string(),
            status_reason: None,
            created_at: now,
            last_updated_at: now,
            confirmed_due_at: None,
        }
    }

    /// Whether the destination's confirmation has been completed, so the only thing left
    /// to decide is whether traffic is allowed.
    ///
    /// A destination whose move out of `IN_PROGRESS` is still owed counts as confirmed:
    /// the token came back, and the status it rests at is a matter of the clock.
    pub fn is_confirmed(&self) -> bool {
        self.confirmed_due_at.is_some()
            || self.status == DESTINATION_DISABLED
            || self.status == DESTINATION_ENABLED
    }

    /// Whether traffic to this destination is allowed: the one question the `http` action's
    /// delivery path asks.
    pub fn allows_traffic(&self) -> bool {
        self.status == DESTINATION_ENABLED
    }

    /// Record that the outstanding confirmation token came back at `now`.
    ///
    /// The destination does not leave `IN_PROGRESS` on this call: the move to
    /// [`DESTINATION_DISABLED`] is what the clock then owes ([`CONFIRMATION_SPAN`]), so
    /// what is written here is WHEN it is due. A destination that was already confirmed is
    /// left exactly as it stands, which is what makes a repeated confirmation of one token
    /// change nothing.
    pub fn record_confirmation(&mut self, now: SystemTime) {
        if self.is_confirmed() {
            return;
        }
        self.status_reason = None;
        self.confirmed_due_at = Some(now + CONFIRMATION_SPAN);
        self.last_updated_at = now;
    }

    /// Put the destination back in `IN_PROGRESS` at `now` with a FRESH challenge token,
    /// which the caller then sends to the confirmation URL, and answer that token.
    ///
    /// "Calling `UpdateTopicRuleDestination` causes a new confirmation challenge to be
    /// sent to your confirmation endpoint" — so the token the superseded challenge carried
    /// stops confirming anything, and a destination that had already been confirmed is
    /// unconfirmed again.
    pub fn challenge_again(&mut self, token: String, now: SystemTime) -> String {
        self.confirmation_token = token;
        self.status = DESTINATION_IN_PROGRESS.to_string();
        self.status_reason = None;
        self.confirmed_due_at = None;
        self.last_updated_at = now;
        self.confirmation_token.clone()
    }

    /// Move a confirmed destination to `status` at `now`, which is what an update of a
    /// confirmed destination does.
    pub fn set_status(&mut self, status: &str, now: SystemTime) {
        self.status = status.to_string();
        self.status_reason = None;
        self.confirmed_due_at = None;
        self.last_updated_at = now;
    }

    /// Materialise the move out of `IN_PROGRESS` the clock already owes at `now`, and
    /// answer whether anything changed.
    ///
    /// The destination was answered `IN_PROGRESS` with its confirmation completed and the
    /// transition due at a recorded instant, so by `now` it rests at
    /// [`DESTINATION_DISABLED`] whether or not anybody called: a reader states that, and
    /// stamps the transition's own instant as the destination's `lastUpdatedAt` rather
    /// than the moment of the read.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let Some(due) = self.confirmed_due_at.filter(|due| now >= *due) else {
            return false;
        };
        self.status = DESTINATION_DISABLED.to_string();
        self.confirmed_due_at = None;
        self.last_updated_at = due;
        true
    }

    /// This destination as the modelled `TopicRuleDestination` a reader hands back.
    ///
    /// `vpcProperties` and `influxDBProperties` are the other destination kinds' and are
    /// left out rather than reported empty; the HTTP kind reports the URL its confirmation
    /// challenge was sent to.
    pub fn view(&self) -> TopicRuleDestination {
        TopicRuleDestination {
            arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_updated_at: Some(::aws_smithy_types::DateTime::from(self.last_updated_at)),
            status_reason: self.status_reason.clone(),
            http_url_properties: Some(HttpUrlDestinationProperties {
                confirmation_url: Some(self.confirmation_url.clone()),
            }),
            vpc_properties: None,
            influx_db_properties: None,
        }
    }
}

/// Materialise every move out of `IN_PROGRESS` the clock owes at `now` over the whole
/// store, and answer whether anything changed.
///
/// Every reader of a destination calls this first, so the status it reports is the current
/// one; it writes only what was already due at `now`.
pub fn settle_destinations(
    destinations: &mut BTreeMap<String, RegisteredTopicRuleDestination>,
    now: SystemTime,
) -> bool {
    let mut changed = false;
    for destination in destinations.values_mut() {
        // Every destination is settled, so this is not a short-circuiting `any`: the clock
        // owes each of them its own transition.
        changed |= destination.settle(now);
    }
    changed
}

/// The destination the outstanding confirmation token `token` belongs to, or `None` when
/// it belongs to none.
///
/// A token is minted per challenge and replaced by the next one, so at most one
/// destination answers to it — which is what makes the token the whole of
/// `ConfirmTopicRuleDestination`'s request.
pub fn destination_of_token<'a>(
    destinations: &'a mut BTreeMap<String, RegisteredTopicRuleDestination>,
    token: &str,
) -> Option<&'a mut RegisteredTopicRuleDestination> {
    destinations
        .values_mut()
        .find(|destination| destination.confirmation_token == token)
}

/// The opening of a substitution template, which AWS IoT renders from the message. A value
/// carrying one is computed from each message, which this engine does not do, so an action
/// carrying one is not performed rather than performed on the literal text.
const SUBSTITUTION_TEMPLATE_OPENING: &str = "${";

/// Why this engine does not perform the `http` action `action` at all, or `None` when it
/// does.
///
/// What it cannot compute faithfully it does not perform:
///
/// * a `url`, `confirmationUrl` or header carrying a substitution template is a value
///   rendered from each message — and, for the URLs, a destination per rendered value
///   ("If you use substitution templates in the confirmationUrl, you must create and enable
///   topic rule destinations that match each possible value of the substitution template" —
///   `HttpAction$confirmationUrl`), so neither the request nor the gate can be decided here;
/// * an `auth` configuration asks for the request to be SIGNED as the role it names
///   (`HttpAuthorization$sigv4`), and a request this engine sent unsigned would not be the
///   request the action asks for.
pub fn http_action_unperformable(action: &HttpAction) -> Option<&'static str> {
    if action.url.contains(SUBSTITUTION_TEMPLATE_OPENING)
        || action
            .confirmation_url
            .as_deref()
            .is_some_and(|url| url.contains(SUBSTITUTION_TEMPLATE_OPENING))
    {
        return Some("its url or confirmationUrl carries a substitution template");
    }
    let templated_header = action.headers.iter().flatten().any(|header| {
        header.key.contains(SUBSTITUTION_TEMPLATE_OPENING)
            || header.value.contains(SUBSTITUTION_TEMPLATE_OPENING)
    });
    if templated_header {
        return Some("one of its headers carries a substitution template");
    }
    if action
        .auth
        .as_ref()
        .is_some_and(|auth| auth.sigv4.is_some())
    {
        return Some("it asks for the request to be signed under a role");
    }
    None
}

/// Whether the `http` action `action` is ALLOWED to deliver to its endpoint URL, given the
/// destinations this account holds.
///
/// `HttpAction$confirmationUrl` documents the relation between an action and a
/// destination: "The value of the confirmation URL must be a prefix of the endpoint URL.
/// If you do not specify a confirmation URL IoT uses the endpoint URL as the confirmation
/// URL" — so the action is admitted by a destination whose `confirmationUrl` is a prefix
/// of the action's `url`, and traffic reaches that URL only while such a destination is
/// `ENABLED` ("Confirmation was completed, and traffic to this destination is allowed").
///
/// An action whose own `confirmationUrl` is not a prefix of its `url` names no destination
/// it could be admitted by, so nothing is delivered for it either.
pub fn http_action_is_allowed(
    destinations: &BTreeMap<String, RegisteredTopicRuleDestination>,
    action: &HttpAction,
) -> bool {
    let confirmation_url = action.confirmation_url.as_deref().unwrap_or(&action.url);
    if !action.url.starts_with(confirmation_url) {
        return false;
    }
    destinations
        .values()
        .filter(|destination| destination.allows_traffic())
        .any(|destination| action.url.starts_with(&destination.confirmation_url))
}

/// Deliver one selected message to the endpoint an `http` action names, and answer whether
/// that endpoint took it.
///
/// The request is the one `HttpAction` describes: a POST of the message the rule's
/// statement selected to the action's `url`, carrying every header the action names
/// (`HttpAction$headers`, "The HTTP headers to send with the message data"). It is made
/// over the IN-PROCESS transport and nothing else, so a URL a caller configured reaches
/// only what this emulator serves itself.
///
/// The GATE is the caller's ([`http_action_is_allowed`]): this sends what it is given.
pub async fn deliver_to_endpoint(
    transport: &InProcessTransport,
    action: &HttpAction,
    message: &[u8],
) -> Result<(), String> {
    let mut request = ::http::Request::builder()
        .method(::http::Method::POST)
        .uri(&action.url);
    // The authority of the URL the gate admitted, which the destination's own
    // `confirmationUrl` is a prefix of — so the host named here is the confirmed
    // destination's.
    if let Some(host) = authority_of(&action.url) {
        request = request.header(::http::header::HOST, host);
    }
    for header in action.headers.iter().flatten() {
        // "The HTTP headers to send with the message data" go to the endpoint, but the
        // endpoint is reached over this emulator's OWN dispatch ([`InProcessTransport`]), so
        // a header that dispatch reads is not a header a rule gets to choose: it would
        // decide which service answers, or who the request claims to be, rather than
        // travelling to a destination.
        if is_read_by_this_emulators_dispatch(&header.key) {
            continue;
        }
        request = request.header(&header.key, &header.value);
    }
    // Every value above comes from the rule, so a `url` that is not a URI or a header the
    // action named that is not a header value fails HERE rather than panicking.
    let request = request
        .body(::aws_smithy_types::body::SdkBody::from(
            ::bytes::Bytes::copy_from_slice(message),
        ))
        .map_err(|error| error.to_string())
        .and_then(|request| {
            ::aws_smithy_runtime_api::http::Request::try_from(request).map_err(|e| e.to_string())
        })
        .map_err(|error| format!("the action does not render a request: {error}"))?;
    let response = transport
        .call(request)
        .await
        .map_err(|error| format!("the endpoint could not be reached: {error}"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("the endpoint answered {status}"));
    }
    Ok(())
}

/// The query parameter the confirmation challenge carries its token in, which the owner of
/// the confirmation endpoint reads it out of and hands to `ConfirmTopicRuleDestination`.
pub const CONFIRMATION_TOKEN_QUERY_PARAMETER: &str = "confirmationToken";

/// The `messageType` the confirmation challenge's document names itself by, so an endpoint
/// that also takes the rule traffic can tell the two apart.
pub const DESTINATION_CONFIRMATION_MESSAGE_TYPE: &str = "DestinationConfirmation";

/// The path `ConfirmTopicRuleDestination` is served at, which the challenge's `enableUrl`
/// leads back to: `GET /confirmdestination/{confirmationToken+}`.
const CONFIRM_DESTINATION_PATH: &str = "/confirmdestination";

/// The confirmation challenge one destination's endpoint is sent: which destination it is
/// about, the token that confirms it, and the address the token is handed back at.
#[derive(Debug, Clone)]
pub struct ConfirmationChallenge {
    /// The ARN of the destination being confirmed.
    pub arn: String,
    /// The URL the challenge is sent to.
    pub confirmation_url: String,
    /// The outstanding token this challenge carries.
    pub confirmation_token: String,
    /// Where the token is handed back: this emulator's own
    /// `ConfirmTopicRuleDestination`, which is what an endpoint that confirms itself
    /// calls.
    pub enable_url: String,
}

impl ConfirmationChallenge {
    /// The challenge for `destination`, whose token is handed back at `endpoint`'s
    /// `ConfirmTopicRuleDestination`.
    pub fn new(destination: &RegisteredTopicRuleDestination, endpoint: &str) -> Self {
        Self {
            arn: destination.arn.clone(),
            confirmation_url: destination.confirmation_url.clone(),
            confirmation_token: destination.confirmation_token.clone(),
            enable_url: format!(
                "{endpoint}{CONFIRM_DESTINATION_PATH}/{}",
                destination.confirmation_token
            ),
        }
    }

    /// The URL this challenge is sent to: the configured confirmation URL carrying the
    /// token as [`CONFIRMATION_TOKEN_QUERY_PARAMETER`], added to whatever query string
    /// that URL already has.
    ///
    /// The token is this service's own ([`super::fresh_uuid`]), so it needs no escaping to
    /// travel in a query string; nothing of the caller's URL is rewritten.
    pub fn request_url(&self) -> String {
        let separator = if self.confirmation_url.contains('?') {
            '&'
        } else {
            '?'
        };
        format!(
            "{}{separator}{CONFIRMATION_TOKEN_QUERY_PARAMETER}={}",
            self.confirmation_url, self.confirmation_token
        )
    }

    /// The JSON document the challenge POSTs: which destination it is about, the token
    /// that confirms it, where to hand that token back, and the message type that tells it
    /// apart from the rule traffic the same endpoint may take.
    pub fn document(&self) -> ::serde_json::Value {
        ::serde_json::json!({
            "arn": self.arn,
            "confirmationToken": self.confirmation_token,
            "enableUrl": self.enable_url,
            "messageType": DESTINATION_CONFIRMATION_MESSAGE_TYPE,
        })
    }
}

/// Send one confirmation challenge to the endpoint it is about, and answer whether that
/// endpoint took it.
///
/// The request is a POST of [`ConfirmationChallenge::document`] to
/// [`ConfirmationChallenge::request_url`] — the confirmation URL carrying the token — made
/// over the IN-PROCESS transport and nothing else, so it reaches only what this emulator
/// serves itself.
///
/// An endpoint that is not there, or that answers outside `2xx`, leaves the destination
/// exactly as it is: `IN_PROGRESS` with its token outstanding, which the owner can have
/// challenged again by setting the status back to `IN_PROGRESS`. Nothing about the
/// challenge is reported to the caller of the create, exactly as AWS reports nothing about
/// it.
pub async fn send_confirmation_challenge(
    transport: &InProcessTransport,
    challenge: &ConfirmationChallenge,
) -> Result<(), String> {
    let document = challenge.document().to_string();
    let mut request = ::http::Request::builder()
        .method(::http::Method::POST)
        .uri(challenge.request_url())
        .header(::http::header::CONTENT_TYPE, "application/json");
    if let Some(host) = authority_of(&challenge.confirmation_url) {
        request = request.header(::http::header::HOST, host);
    }
    // Every value above comes from the configuration, so a `confirmationUrl` that is not a
    // URI fails HERE rather than panicking.
    let request = request
        .body(::aws_smithy_types::body::SdkBody::from(
            ::bytes::Bytes::from(document),
        ))
        .map_err(|error| error.to_string())
        .and_then(|request| {
            ::aws_smithy_runtime_api::http::Request::try_from(request).map_err(|e| e.to_string())
        })
        .map_err(|error| format!("the confirmation URL does not render a request: {error}"))?;
    let response = transport
        .call(request)
        .await
        .map_err(|error| format!("the confirmation URL could not be reached: {error}"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!(
            "the confirmation URL answered the challenge with {status}"
        ));
    }
    Ok(())
}

/// The authority of `url`, which a request to it names as its `Host`, or `None` when the
/// value carries none.
pub fn authority_of(url: &str) -> Option<String> {
    let uri: ::http::Uri = url.parse().ok()?;
    uri.authority().map(::std::string::ToString::to_string)
}

/// The headers this emulator's own request dispatch reads: the host that selects which
/// service answers, and the SigV4 values that say who the request is from
/// (`emulator_server::resolve_route_key` and the authenticator behind it).
///
/// A rule's `headers` are values for its DESTINATION, so none of these travels: a delivery
/// made over this emulator's own dispatch ([`InProcessTransport`]) must not let a rule pick
/// the service that answers it or the identity it is answered as.
const HEADERS_READ_BY_DISPATCH: &[&str] = &[
    "host",
    "authorization",
    "x-amz-target",
    "x-amz-security-token",
    "x-amz-date",
    "x-amz-content-sha256",
];

/// Whether `name` is one of [`HEADERS_READ_BY_DISPATCH`]. HTTP header names are
/// case-insensitive, so the comparison is too.
fn is_read_by_this_emulators_dispatch(name: &str) -> bool {
    HEADERS_READ_BY_DISPATCH
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

/// The ONE scheme a topic rule destination's confirmation URL may name.
///
/// AWS IoT's HTTP topic rule destinations are HTTPS endpoints — the confirmation is how the
/// service establishes "ownership of or access to the topic rule destination URL"
/// (`HttpUrlDestinationConfiguration$confirmationUrl`), which a cleartext endpoint cannot
/// carry — so a create naming any other scheme is refused rather than recorded.
pub const CONFIRMATION_URL_SCHEME: &str = "https";

/// Why `url` is not a confirmation URL this service will challenge, or `None` when it is.
///
/// This is the DESTINATION POLICY the create applies to the one value it takes from the
/// request, before anything is recorded or any challenge is built:
///
/// * it must be an absolute URL — `Url` is a plain string in the model (`length 0..2000`),
///   so the empty string and a bare word satisfy the model, and neither is an address a
///   challenge could be sent to;
/// * its scheme must be [`CONFIRMATION_URL_SCHEME`] — the one scheme AWS IoT confirms an
///   HTTP destination over;
/// * it must name a host, which is what decides which of the services this emulator serves
///   the challenge is matched against ([`InProcessTransport`]);
/// * it must carry no fragment: a fragment is never sent to a server, so a token added
///   after one would reach nobody.
pub fn unchallengeable_confirmation_url(url: &str) -> Option<&'static str> {
    let Ok(uri) = url.parse::<::http::Uri>() else {
        return Some("it is not a URL");
    };
    if uri.scheme_str() != Some(CONFIRMATION_URL_SCHEME) {
        return Some("its scheme is not https");
    }
    if uri.authority().is_none() {
        return Some("it names no host");
    }
    if url.contains('#') {
        return Some("it carries a URL fragment, which is not sent to an endpoint");
    }
    None
}

/// The ONE transport every request this module makes is made over: the emulator the
/// handler asking for it is running within.
///
/// [`::smithy_server_runtime::EmbeddedEmulator::http_connector`] hands out the ROUTER that
/// dispatched the request being served, so a request made over it is matched against the
/// services this emulator serves ITSELF and answered by one of them, or by that router's
/// own refusal. Whatever URL a caller configured:
///
/// * no socket is opened and no host name is resolved, so no address — a loopback service,
///   a private range, a link-local metadata endpoint — is reachable, and there is no
///   resolution to re-check or to be rebound;
/// * no redirect is followed: the router's answer is the answer, and this module makes
///   exactly one request per challenge and per delivered message;
/// * nothing of the host reaches it: the request carries only the challenge document or the
///   selected message, and the emulator's own credentials are never put on it;
/// * at most [`MAX_REQUESTS_IN_FLIGHT`] of these requests exist at any instant
///   ([`RequestInFlight`]), which is what bounds the work one caller can start.
///
/// There is NO constructor from an HTTP client of any kind, which is what keeps that true
/// as this module changes: a caller can only hand over the emulator it is running within
/// ([`InProcessTransport::of`]) or, from a test, another in-process endpoint standing in for
/// the router ([`InProcessTransport::over_in_process_endpoint`]).
#[derive(Debug, Clone)]
pub struct InProcessTransport(SharedHttpConnector);

impl InProcessTransport {
    /// The transport of the emulator a handler is running within: `ctx.callback()`.
    #[must_use]
    pub fn of(emulator: &::smithy_server_runtime::EmbeddedEmulator) -> Self {
        Self(emulator.http_connector())
    }

    /// Another IN-PROCESS endpoint standing in for the router, for a test that asserts the
    /// REQUEST this emulator renders rather than what an endpoint did with it. The
    /// connector a caller hands over must itself answer in process — this is the test seam,
    /// not a way to give the emulator a network client.
    #[must_use]
    pub fn over_in_process_endpoint(endpoint: SharedHttpConnector) -> Self {
        Self(endpoint)
    }

    /// Make one request over this transport, within this module's own budget.
    ///
    /// The budget is taken BEFORE the request is made and released when it has been
    /// answered ([`RequestInFlight`]), so it bounds the whole in-process chain and not one
    /// level of it: a request held open while the endpoint it reached is served keeps its
    /// own slot, so an endpoint that is itself this emulator — an endpoint URL naming an
    /// operation whose own work reaches this module again — can nest at most
    /// [`MAX_REQUESTS_IN_FLIGHT`] deep, whatever the per-message fan-out is. Past the
    /// budget nothing is sent and the caller reports it the way it reports an endpoint that
    /// did not take the request.
    async fn call(
        &self,
        request: ::aws_smithy_runtime_api::http::Request,
    ) -> Result<::aws_smithy_runtime_api::client::orchestrator::HttpResponse, String> {
        let _in_flight = RequestInFlight::take().ok_or_else(|| {
            format!(
                "{MAX_REQUESTS_IN_FLIGHT} topic rule destination requests are already in \
                 flight, which is the most this emulator makes at once"
            )
        })?;
        HttpConnector::call(&self.0, request)
            .await
            .map_err(|error| error.to_string())
    }
}

/// How many requests this module makes at once, across every destination and every rule.
///
/// An EMULATOR ceiling, not an AWS limit: the requests are driven by callers — one per
/// create or requested challenge, one per selected message of an `http` action — and each
/// one is answered by this same process, which may itself reach this module again. Far
/// above any rule set or destination set a client under test drives at once, and low enough
/// that no caller can make the process spend itself on nested in-process requests.
pub const MAX_REQUESTS_IN_FLIGHT: usize = 16;

/// How many of this module's requests are in flight right now.
static REQUESTS_IN_FLIGHT: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One slot of the [`MAX_REQUESTS_IN_FLIGHT`] budget, held for as long as the request it
/// was taken for is being answered and released on EVERY path out — including an error and
/// a cancelled task — because the release is this value's `Drop`.
struct RequestInFlight;

impl RequestInFlight {
    /// Take a slot, or `None` when the budget is already spent.
    ///
    /// The compare-and-exchange is what keeps the budget a budget under concurrent
    /// requests: it retries only when another request took a slot in between, and never
    /// raises the count past [`MAX_REQUESTS_IN_FLIGHT`].
    fn take() -> Option<Self> {
        let mut in_flight = REQUESTS_IN_FLIGHT.load(::std::sync::atomic::Ordering::SeqCst);
        loop {
            if in_flight >= MAX_REQUESTS_IN_FLIGHT {
                return None;
            }
            match REQUESTS_IN_FLIGHT.compare_exchange_weak(
                in_flight,
                in_flight + 1,
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
            ) {
                Ok(_) => return Some(Self),
                Err(taken) => in_flight = taken,
            }
        }
    }
}

impl Drop for RequestInFlight {
    fn drop(&mut self) {
        REQUESTS_IN_FLIGHT.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// Send the outstanding confirmation challenge of the destination `arn` holds, off the
/// response path.
///
/// Returns at once: the challenge is sent in a task of its own, because the create and the
/// update that ask for one answer while the challenge is still owed — AWS answers them the
/// same way whatever the endpoint does with it. The caller hands over its own clone of the
/// in-process transport, so the challenge is sent exactly where the asking call was made.
pub fn start_confirmation_challenge<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    transport: InProcessTransport,
    endpoint: String,
    arn: String,
) {
    ::tokio::spawn(async move {
        challenge_destination(&engine, &transport, &endpoint, &arn).await;
    });
}

/// Send the challenge of ONE destination: read what it needs under the guard, release the
/// guard, and POST.
///
/// A destination the store no longer holds has nothing owed to it, and a store that is not
/// there holds none.
pub async fn challenge_destination<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    transport: &InProcessTransport,
    endpoint: &str,
    arn: &str,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    // The challenge is built under the guard and sent with it released: no lock is held
    // across an await.
    let challenge = {
        let store = state.lock();
        let Some(destination) = store.topic_rule_destinations.get(arn) else {
            return;
        };
        ConfirmationChallenge::new(destination, endpoint)
    };
    if let Err(reason) = send_confirmation_challenge(transport, &challenge).await {
        ::tracing::warn!(
            destination = %arn,
            reason = %reason,
            "a topic rule destination's confirmation challenge was not taken, so the \
             destination is still unconfirmed"
        );
    }
}
