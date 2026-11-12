// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this crate's test binary shares: a router holding
//! this service over a fresh engine, one awsQuery call, and just enough XML
//! reading to assert on a response.
//!
//! Declared in `tests/all.rs` beside the suites, so each suite reaches it as
//! `crate::harness`.

use service_com_amazonaws_ses_simpleemailservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The API version SES's query API carries on every request.
pub const VERSION: &str = "2010-12-01";

/// The Region every request this harness builds is scoped to, which is the
/// Region of its SigV4 credential scope ([`AUTHORIZATION`]) and therefore the
/// Region an ARN such a request names has to be in.
pub const REGION: &str = "us-east-1";

/// The account this emulator serves, which is the account an ARN a request
/// names has to belong to.
pub fn account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// The ARN of the Amazon SNS topic `name` of this account in [`REGION`], which
/// is the published form of a topic ARN
/// (`arn:<partition>:sns:<region>:<account-id>:<topic-name>`).
pub fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{}:{name}", account())
}

/// How the router resolves which service a request is for: the 4th field of the
/// SigV4 credential scope is the signing name, and this service's is `ses`
/// (`resolve_route_key`). Request authentication is off unless the process turns
/// it on, so the signature itself is never checked here.
pub const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ses/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One form field of a query request.
pub type Field = (String, String);

/// A form field, from anything stringy.
pub fn field(name: impl Into<String>, value: impl Into<String>) -> Field {
    (name.into(), value.into())
}

/// A router holding only this service, over a fresh engine — the same
/// construction `router/src/lib.rs` performs for every service.
pub fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// A router holding only this service, over an engine the CALLER holds, so a
/// suite can read or seed this service's store ([`state::service_state`]) beside
/// the calls it makes over the wire.
///
/// [`state::service_state`]: service_com_amazonaws_ses_simpleemailservice::state::service_state
pub fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A router and the engine behind it, for a suite that needs both.
pub fn emulator_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
}

/// A router holding this service beside Amazon SNS — the service that OWNS every topic a
/// destination of this service names — over ONE shared engine, plus that engine, for a suite
/// that mounts a further peer service on it.
///
/// Amazon SNS is mounted exactly as the assembled router mounts it, so what a destination
/// pre-flight resolves is the topic Amazon SNS itself holds. Here rather than in one suite
/// because every suite about a destination this service resolves against Amazon SNS needs the
/// same two services on one engine.
pub fn emulator_with_sns_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; the awsQuery calls
    // these suites make sign nothing, so the check is off and the scope header is read for
    // routing only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// The same router, for a suite that needs no further service on that engine.
pub fn emulator_with_sns() -> MultiServiceRouter {
    emulator_with_sns_and_engine().0
}

/// An Amazon SNS client over `router`'s own in-process callback, so every topic a suite makes
/// is made through Amazon SNS's own operations and nothing leaves this process.
pub fn sns(router: &MultiServiceRouter) -> aws_sdk_sns::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_sns::Config::builder()
        .behavior_version(aws_sdk_sns::config::BehaviorVersion::latest())
        .region(aws_sdk_sns::config::Region::new(REGION))
        .endpoint_url(format!("https://sns.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_sns::Client::from_conf(config)
}

/// The store as it stands once the clock has carried every requested
/// verification through to `Success`.
///
/// Asking for an identity's verification only STARTS it: the move the clock owes
/// the record falls due `IDENTITY_VERIFICATION_SPAN` later, so a suite that needs
/// a SENDING identity settles the store against an instant past that span instead
/// of sleeping for it. Nothing else about the store is touched, and a suite about
/// the span itself (or about the pending state) drives the real clock instead.
///
/// [`IDENTITY_VERIFICATION_SPAN`]: service_com_amazonaws_ses_simpleemailservice::state::IDENTITY_VERIFICATION_SPAN
pub fn settle_verifications(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_com_amazonaws_ses_simpleemailservice::state::service_state(&**engine)
        .expect("this service has a store on the engine it is mounted on");
    let past_the_span = std::time::SystemTime::now()
        + service_com_amazonaws_ses_simpleemailservice::state::IDENTITY_VERIFICATION_SPAN;
    state.lock().settle_identities(past_the_span);
}

/// One awsQuery call: `(status, body)`.
pub async fn call(router: &MultiServiceRouter, action: &str, params: &[Field]) -> (u16, String) {
    let answer = call_keeping_headers(router, action, params).await;
    (answer.status, answer.body)
}

/// How many times `needle` occurs in `text`.
///
/// Here rather than in one suite because every suite that asserts an envelope is ONE
/// document counts the same way.
pub fn occurrences(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// The header AWS reports a response's request id in, beside the `<RequestId>`
/// element an awsQuery body carries.
pub const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// One answer from the emulator: what a client reads before it parses anything.
///
/// Here rather than in one suite because more than one suite asserts on the
/// response HEADERS, which [`call`]'s `(status, body)` pair does not carry.
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The [`REQUEST_ID_HEADER`] header, empty when the response carries none.
    pub request_id_header: String,
    /// The response body.
    pub body: String,
}

/// One awsQuery call, exactly as [`call`] makes it, with the response HEADERS
/// kept.
pub async fn call_keeping_headers(
    router: &MultiServiceRouter,
    action: &str,
    params: &[Field],
) -> Answer {
    let mut body = format!("Action={action}&Version={VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        request_id_header: response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form of an AWS request id,
/// which is the form the success path's `ResponseMetadata/RequestId` carries
/// too.
///
/// Here rather than in one suite for the same reason [`Answer`] is: the form is
/// the service's, not one suite's.
pub fn is_request_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        })
}

/// The destination an event destination publishes to, as the form fields that
/// name it on the wire.
///
/// Here rather than in one suite because `CreateConfigurationSetEventDestination`
/// and `UpdateConfigurationSetEventDestination` carry the same
/// `EventDestination` shape, so the suites about them write one destination on
/// the wire between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// `SNSDestination.TopicARN`.
    Sns(String),
    /// `KinesisFirehoseDestination`: `(IAMRoleARN, DeliveryStreamARN)`.
    Firehose(String, String),
    /// `CloudWatchDestination.DimensionConfigurations`: one entry per
    /// `(DimensionName, DimensionValueSource, DefaultDimensionValue)`.
    CloudWatch(Vec<(String, String, String)>),
    /// No destination member at all.
    None,
    /// Both an SNS and a CloudWatch destination, which is more than one.
    SnsAndCloudWatch(String),
}

impl Destination {
    /// The form fields this destination contributes to a request.
    pub fn fields(&self) -> Vec<Field> {
        match self {
            Self::Sns(topic_arn) => {
                vec![field("EventDestination.SNSDestination.TopicARN", topic_arn)]
            }
            Self::Firehose(role_arn, stream_arn) => vec![
                field(
                    "EventDestination.KinesisFirehoseDestination.IAMRoleARN",
                    role_arn,
                ),
                field(
                    "EventDestination.KinesisFirehoseDestination.DeliveryStreamARN",
                    stream_arn,
                ),
            ],
            Self::CloudWatch(dimensions) => dimensions
                .iter()
                .enumerate()
                .flat_map(|(index, (name, source, default))| {
                    let at = format!(
                        "EventDestination.CloudWatchDestination.DimensionConfigurations.member.{}",
                        index + 1
                    );
                    [
                        field(format!("{at}.DimensionName"), name),
                        field(format!("{at}.DimensionValueSource"), source),
                        field(format!("{at}.DefaultDimensionValue"), default),
                    ]
                })
                .collect(),
            Self::None => Vec::new(),
            Self::SnsAndCloudWatch(topic_arn) => {
                let mut fields = Self::Sns(topic_arn.clone()).fields();
                fields.extend(
                    Self::CloudWatch(vec![(
                        "campaign".to_string(),
                        "messageTag".to_string(),
                        "none".to_string(),
                    )])
                    .fields(),
                );
                fields
            }
        }
    }
}

/// One call of `action` carrying a whole `EventDestination` — the request shape
/// `CreateConfigurationSetEventDestination` and
/// `UpdateConfigurationSetEventDestination` share.
pub async fn write_event_destination(
    router: &MultiServiceRouter,
    action: &str,
    configuration_set: &str,
    name: &str,
    enabled: bool,
    event_types: &[&str],
    destination: &Destination,
) -> (u16, String) {
    let mut params = vec![
        field("ConfigurationSetName", configuration_set),
        field("EventDestination.Name", name),
        field(
            "EventDestination.Enabled",
            if enabled { "true" } else { "false" },
        ),
    ];
    for (index, event_type) in event_types.iter().enumerate() {
        params.push(field(
            format!("EventDestination.MatchingEventTypes.member.{}", index + 1),
            *event_type,
        ));
    }
    params.extend(destination.fields());
    call(router, action, &params).await
}

/// `CreateConfigurationSetEventDestination`.
pub async fn create_event_destination(
    router: &MultiServiceRouter,
    configuration_set: &str,
    name: &str,
    enabled: bool,
    event_types: &[&str],
    destination: &Destination,
) -> (u16, String) {
    write_event_destination(
        router,
        "CreateConfigurationSetEventDestination",
        configuration_set,
        name,
        enabled,
        event_types,
        destination,
    )
    .await
}

/// `UpdateConfigurationSetEventDestination`.
pub async fn update_event_destination(
    router: &MultiServiceRouter,
    configuration_set: &str,
    name: &str,
    enabled: bool,
    event_types: &[&str],
    destination: &Destination,
) -> (u16, String) {
    write_event_destination(
        router,
        "UpdateConfigurationSetEventDestination",
        configuration_set,
        name,
        enabled,
        event_types,
        destination,
    )
    .await
}

/// The `<EventDestinations>` element `DescribeConfigurationSet` reports for
/// `name`, asked for by the attribute group the model's own
/// `ConfigurationSetAttribute` enum names.
pub async fn described_event_destinations(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeConfigurationSet",
        &[
            field("ConfigurationSetName", name),
            field(
                "ConfigurationSetAttributeNames.member.1",
                "eventDestinations",
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {name}: expected 200, got {status} with {body}"
    );
    element(&body, "EventDestinations").unwrap_or_default()
}

/// Percent-encode a form field, leaving only the RFC 3986 unreserved set.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Index just past the opening `<tag>` — or `<tag xmlns="…">`, which is how the
/// codec writes the first element of a nested struct.
fn past_open_tag(xml: &str, tag: &str) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&needle) {
        let at = from + at;
        let after = at + needle.len();
        match xml[after..].chars().next() {
            Some('>') => return Some(after + 1),
            Some(c) if c.is_whitespace() => return Some(after + xml[after..].find('>')? + 1),
            // `<Identities…` is not `<Identity…`: keep looking.
            _ => from = after,
        }
    }
    None
}

/// The inner XML of every `<tag>…</tag>` in `xml`, in document order.
pub fn elements(xml: &str, tag: &str) -> Vec<String> {
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = past_open_tag(rest, tag) {
        let after = &rest[start..];
        let Some(end) = after.find(&close) else { break };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

/// The inner XML of the first `<tag>…</tag>` in `xml`.
pub fn element(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).into_iter().next()
}

/// The `<entry><key>…</key><value>…</value></entry>` pairs of an awsQuery map,
/// as `(key, the value element's inner XML)`.
pub fn map_entries(xml: &str) -> Vec<(String, String)> {
    elements(xml, "entry")
        .into_iter()
        .map(|entry| {
            (
                element(&entry, "key").unwrap_or_default(),
                element(&entry, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// XML character data as the value it stands for.
///
/// A value an assertion is about — a policy document, a refusal message — may
/// carry `"`, `'`, `<`, `>` or `&`, which an XML body escapes; the escaping is
/// undone before comparing. Those five entities are the only ones a body can
/// carry, and data that was not escaped passes through unchanged. `&amp;` is
/// replaced last, so an escaped ampersand is not re-read as the start of
/// another entity.
pub fn xml_text(data: &str) -> String {
    data.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope, with
/// the message's escaped entities read back ([`xml_text`]).
///
/// Here rather than in one suite because every suite that asserts a refusal
/// reads the same two elements out of the same envelope.
pub fn error_envelope(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code"),
        element(body, "Message").map(|text| xml_text(&text)),
    )
}
