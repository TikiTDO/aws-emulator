// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SNS topic + subscription + publish slice, driven over the
//! wire: every call below is an awsQuery HTTP request dispatched through the router the
//! emulator serves in production, so the handlers are exercised exactly as an AWS SDK
//! exercises them, and nothing but persisted state carries information between calls.

use std::collections::HashSet;
use std::sync::Arc;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_NEXT_TOKEN_REFUSAL, MAX_ATTRIBUTE_BYTES_PER_RECORD, MAX_DATA_PROTECTION_POLICY_LEN,
    MAX_RECORDS_PER_STORE, MAX_RETAINED_DELIVERY_BYTES, MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::{
    ComAmazonawsSnsAmazonsimplenotificationserviceState as SnsState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The emulator under test: one shared engine, one SNS service, one router.
///
/// `pub(crate)` so a sibling suite in this test target drives SNS over the same
/// wire harness rather than building a second one.
pub(crate) struct Sns {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

/// One answered SNS call.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
    /// The `x-amzn-requestid` response header, when the response carries one.
    pub(crate) request_id_header: Option<String>,
}

impl Answer {
    /// The text of the first `<tag>…</tag>` element of the response, if any.
    pub(crate) fn element(&self, tag: &str) -> Option<String> {
        self.elements(tag).into_iter().next()
    }

    /// The text of every `<tag>…</tag>` element of the response, in document order,
    /// with the XML entities the envelope escapes decoded back.
    pub(crate) fn elements(&self, tag: &str) -> Vec<String> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let mut found = Vec::new();
        let mut rest = self.body.as_str();
        while let Some(start) = rest.find(&open) {
            rest = &rest[start + open.len()..];
            let Some(end) = rest.find(&close) else { break };
            found.push(xml_unescape(&rest[..end]));
            rest = &rest[end + close.len()..];
        }
        found
    }

    /// The awsQuery `<Code>` of an error response, or `""` on a success envelope.
    pub(crate) fn code(&self) -> String {
        self.element("Code").unwrap_or_default()
    }
}

impl Sns {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Call `action` with form parameters, signed (in scope only — the emulator does
    /// not verify signatures by default) for `us-east-1`.
    pub(crate) async fn call(&self, action: &str, params: &[(&str, &str)]) -> Answer {
        self.call_in_region("us-east-1", action, params).await
    }

    /// Call `action` as a client configured for `region`: the region an ARN this
    /// request mints must carry.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> Answer {
        let mut body = format!("Action={action}&Version=2010-03-31");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&percent_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sns.amazonaws.com/")
            .header("host", format!("sns.{region}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/sns/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let request_id_header = response
            .headers()
            .get("x-amzn-requestid")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status,
            body: String::from_utf8_lossy(response.body()).to_string(),
            request_id_header,
        }
    }

    /// A topic created in `us-east-1`, with its ARN.
    ///
    /// A name ending in `.fifo` names a FIFO topic and nothing else — the suffix is the
    /// one place a topic name carries a period — so the create asks for the kind the
    /// name belongs to.
    pub(crate) async fn topic(&self, name: &str) -> String {
        let mut params: Vec<(&str, &str)> = vec![("Name", name)];
        if name.ends_with(".fifo") {
            params.push(("Attributes.entry.1.key", "FifoTopic"));
            params.push(("Attributes.entry.1.value", "true"));
        }
        let answer = self.call("CreateTopic", &params).await;
        assert_eq!(
            answer.status, 200,
            "CreateTopic {name}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        answer
            .element("TopicArn")
            .unwrap_or_else(|| panic!("CreateTopic {name}: no TopicArn in {}", answer.body))
    }

    /// A read of this service's persisted state, the way a handler reaches it.
    ///
    /// `pub(crate)` so a sibling suite in this test target asserts against the SAME
    /// persisted records rather than building a second harness.
    pub(crate) fn state(&self) -> ::engine::ServiceState<SnsState> {
        self.engine.state::<SnsState>()
    }
}

/// Decode the five XML entities the response envelope escapes (`xml_escape` in
/// `vendor/smithy-schema-server/src/xml.rs`), so a JSON attribute value parses.
///
/// `pub(crate)` so a sibling suite that reads a NESTED element out of the envelope
/// (`<Tags><member><Key>…`, which [`Answer::elements`] cannot pair up) decodes it the
/// same way rather than writing a second decoder.
pub(crate) fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The text of the first `<tag>…</tag>` element inside `block`, or `""` when the
/// element is absent or empty (`<tag/>`, `<tag></tag>`).
///
/// `pub(crate)` so every suite that reads an element NESTED inside a list member or a
/// map entry — which [`Answer::elements`] cannot pair up — scrapes it the same way
/// rather than writing another copy of this walk.
pub(crate) fn text_of(block: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let Some(start) = block.find(&open) else {
        return String::new();
    };
    let rest = &block[start + open.len()..];
    match rest.find(&format!("</{tag}>")) {
        Some(end) => xml_unescape(&rest[..end]),
        None => String::new(),
    }
}

/// Percent-encode a form value: everything but the unreserved set, so an ARN's `:`
/// and `/` survive the round trip.
///
/// `pub(crate)` so a sibling suite that drives SNS over the same awsQuery form encodes a
/// value the same way rather than re-deriving the encoding.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The topic-name rule SNS documents on `CreateTopicInput$Name`: 1 to 256 characters
/// of ASCII letters, numbers, underscores and hyphens, with `.fifo` the one allowed
/// suffix — and that suffix belongs to a FIFO topic and to no other, so the name and
/// the `FifoTopic` attribute have to agree. Rows cover both published bounds, one
/// character past the upper one, and both ways the name and the kind can disagree.
#[tokio::test]
async fn create_topic_enforces_the_documented_topic_name_rule() {
    let sns = Sns::new();
    let max_len = "a".repeat(256);
    let too_long = "a".repeat(257);
    let fifo_at_max = format!("{}.fifo", "b".repeat(251));
    // (case, name, FifoTopic asked for, accepted)
    let cases: &[(&str, &str, bool, bool)] = &[
        ("single character", "a", false, true),
        ("letters digits underscore hyphen", "Bootstrap_Topic-9", false, true),
        ("at the 256 character bound", &max_len, false, true),
        ("fifo suffix", "orders.fifo", true, true),
        ("fifo suffix at the 256 character bound", &fifo_at_max, true, true),
        ("one past the 256 character bound", &too_long, false, false),
        ("empty", "", false, false),
        ("embedded space", "two words", false, false),
        ("punctuation", "orders!", false, false),
        ("a dot that is not the fifo suffix", "orders.v2", false, false),
        ("non-ascii", "orders-ü", false, false),
        ("the fifo suffix without the fifo kind", "unordered.fifo", false, false),
        ("the fifo kind without the fifo suffix", "ordered", true, false),
    ];

    for (case, name, fifo, accepted) in cases {
        let mut params: Vec<(&str, &str)> = vec![("Name", name)];
        if *fifo {
            params.push(("Attributes.entry.1.key", "FifoTopic"));
            params.push(("Attributes.entry.1.value", "true"));
        }
        let answer = sns.call("CreateTopic", &params).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let arn = answer.element("TopicArn").unwrap_or_default();
            assert!(
                arn.ends_with(&format!(":{name}")),
                "case {case:?}: expected an ARN ending in the topic name, got {arn:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }
}

/// CreateTopic is idempotent in the name, and the ARN it mints is
/// `arn:aws:sns:<region>:<account>:<name>` for the region the caller addressed.
#[tokio::test]
async fn create_topic_is_idempotent_and_its_arn_carries_the_requests_region() {
    let sns = Sns::new();
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        let first = sns
            .call_in_region(region, "CreateTopic", &[("Name", "bootstrap-topic")])
            .await;
        let second = sns
            .call_in_region(region, "CreateTopic", &[("Name", "bootstrap-topic")])
            .await;
        let arn = first.element("TopicArn").unwrap_or_default();
        assert_eq!(
            arn,
            format!("arn:aws:sns:{region}:000000000000:bootstrap-topic"),
            "case {region:?}: unexpected ARN, got {}",
            first.body
        );
        assert_eq!(
            second.element("TopicArn").unwrap_or_default(),
            arn,
            "case {region:?}: CreateTopic must be idempotent, got {}",
            second.body
        );
    }

    // The three topics are three independent resources, one per region, and each
    // region's own listing reports its own and no other's — the regional scope
    // `regional_listings.rs` covers over every account-wide listing.
    for region in regions {
        let listed = sns.call_in_region(region, "ListTopics", &[]).await;
        let arns = listed.elements("TopicArn");
        assert_eq!(
            arns,
            vec![format!("arn:aws:sns:{region}:000000000000:bootstrap-topic")],
            "case {region:?}: ListTopics must list that region's topic alone, got {arns:?}"
        );
    }
}

/// A re-create asking for what the topic already is stays idempotent; one asking for a
/// DIFFERENT value of an attribute or a tag the topic holds — a tag it does not hold at
/// all included — is refused.
#[tokio::test]
async fn create_topic_refuses_a_re_create_that_conflicts_with_the_stored_topic() {
    let sns = Sns::new();
    let create = |attribute: Option<&'static str>, tag: Option<&'static str>| {
        let mut params: Vec<(&str, &str)> = vec![("Name", "conflicting-topic")];
        if let Some(value) = attribute {
            params.push(("Attributes.entry.1.key", "DisplayName"));
            params.push(("Attributes.entry.1.value", value));
        }
        if let Some(value) = tag {
            params.push(("Tags.member.1.Key", "team"));
            params.push(("Tags.member.1.Value", value));
        }
        params
    };

    let first = sns.call("CreateTopic", &create(Some("Alpha"), Some("a"))).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} / {}",
        first.status, first.body
    );
    let arn = first.element("TopicArn").unwrap_or_default();

    // (case, attribute asked for, tag asked for, status)
    let cases: &[(&str, Option<&'static str>, Option<&'static str>, u16)] = &[
        ("no attributes at all", None, None, 200),
        ("the same attribute value", Some("Alpha"), None, 200),
        ("the same attribute and the same tag", Some("Alpha"), Some("a"), 200),
        ("a different tag value", None, Some("b"), 400),
        ("a different attribute value", Some("Beta"), None, 400),
    ];
    for (case, attribute, tag, status) in cases {
        let answer = sns.call("CreateTopic", &create(*attribute, *tag)).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        if *status == 200 {
            assert_eq!(
                answer.element("TopicArn").unwrap_or_default(),
                arn,
                "case {case:?}: an idempotent re-create must answer the same ARN"
            );
        } else {
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }

    let read = sns.call("GetTopicAttributes", &[("TopicArn", &arn)]).await;
    assert_eq!(
        attribute_map(&read).get("DisplayName").map(String::as_str),
        Some("Alpha"),
        "a refused re-create must not change the stored attribute, got {}",
        read.body
    );

    // An attribute the topic does not hold yet is APPLIED by a re-create, not dropped.
    let added = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "conflicting-topic"),
                ("Attributes.entry.1.key", "DeliveryPolicy"),
                (
                    "Attributes.entry.1.value",
                    r#"{"http":{"disableSubscriptionOverrides":true}}"#,
                ),
            ],
        )
        .await;
    assert_eq!(
        added.status, 200,
        "a re-create adding an attribute the topic lacks must succeed, got {} / {}",
        added.status, added.body
    );
    let read = sns.call("GetTopicAttributes", &[("TopicArn", &arn)]).await;
    let attributes = attribute_map(&read);
    assert_eq!(
        attributes.get("DeliveryPolicy").map(String::as_str),
        Some(r#"{"http":{"disableSubscriptionOverrides":true}}"#),
        "the added attribute must be stored, got {attributes:?}"
    );
    assert_eq!(
        attributes.get("DisplayName").map(String::as_str),
        Some("Alpha"),
        "the attribute the topic already held must survive, got {attributes:?}"
    );
    let stored = sns.state();
    let stored = stored.lock();
    assert_eq!(
        stored
            .topics
            .get(&arn)
            .and_then(|topic| topic.tags.get("team"))
            .map(String::as_str),
        Some("a"),
        "a refused re-create must leave the tag the topic was created with"
    );
}

/// CreateTopic persists the attributes and tags it was given, and GetTopicAttributes
/// reads the stored ones back beside the ones SNS computes.
#[tokio::test]
async fn create_topic_persists_attributes_and_tags() {
    let sns = Sns::new();
    let answer = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "configured-topic"),
                ("Attributes.entry.1.key", "DisplayName"),
                ("Attributes.entry.1.value", "Bootstrap"),
                ("Attributes.entry.2.key", "DeliveryPolicy"),
                (
                    "Attributes.entry.2.value",
                    r#"{"http":{"disableSubscriptionOverrides":true}}"#,
                ),
                ("Tags.member.1.Key", "team"),
                ("Tags.member.1.Value", "platform"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "expected 200, got {} / {}",
        answer.status, answer.body
    );
    let arn = answer.element("TopicArn").unwrap_or_default();

    let read = sns.call("GetTopicAttributes", &[("TopicArn", &arn)]).await;
    assert_eq!(
        read.status, 200,
        "expected 200, got {} / {}",
        read.status, read.body
    );
    let attributes = attribute_map(&read);
    assert_eq!(
        attributes.get("DisplayName").map(String::as_str),
        Some("Bootstrap"),
        "a stored attribute must read back, got {attributes:?}"
    );
    assert_eq!(
        attributes.get("TopicArn").map(String::as_str),
        Some(arn.as_str()),
        "TopicArn must be computed into the attribute map, got {attributes:?}"
    );
    assert_eq!(
        attributes.get("Owner").map(String::as_str),
        Some("000000000000"),
        "Owner must be the emulator account, got {attributes:?}"
    );

    // The effective delivery policy is the system default with the topic's own
    // policy laid over it: the overridden member changes, the defaults survive.
    let effective: serde_json::Value = serde_json::from_str(
        attributes
            .get("EffectiveDeliveryPolicy")
            .map(String::as_str)
            .unwrap_or("null"),
    )
    .expect("EffectiveDeliveryPolicy must be JSON");
    assert_eq!(
        effective["http"]["disableSubscriptionOverrides"],
        serde_json::json!(true),
        "the topic's own DeliveryPolicy must win, got {effective}"
    );
    assert_eq!(
        effective["http"]["defaultHealthyRetryPolicy"]["numRetries"],
        serde_json::json!(3),
        "the system default must survive the merge, got {effective}"
    );
    let policy: serde_json::Value = serde_json::from_str(
        attributes
            .get("Policy")
            .map(String::as_str)
            .unwrap_or("null"),
    )
    .expect("Policy must be JSON");
    assert_eq!(
        policy["Statement"][0]["Resource"],
        serde_json::json!(arn),
        "the default policy must name the topic, got {policy}"
    );

    let stored = sns.state();
    let stored = stored.lock();
    let topic = stored.topics.get(&arn).expect("the topic must be stored");
    assert_eq!(
        topic.tags.get("team").map(String::as_str),
        Some("platform"),
        "CreateTopic must persist its tags, got {:?}",
        topic.tags
    );
}

/// The `<Attributes>` entries of a GetTopicAttributes response, IN DOCUMENT ORDER:
/// each `<entry>`'s key paired with its own value.
pub(crate) fn attribute_entries(answer: &Answer) -> Vec<(String, String)> {
    answer
        .elements("key")
        .into_iter()
        .zip(answer.elements("value"))
        .collect()
}

/// The `<Attributes>` map of a GetTopicAttributes response.
pub(crate) fn attribute_map(answer: &Answer) -> std::collections::HashMap<String, String> {
    attribute_entries(answer).into_iter().collect()
}

/// Subscribe over every protocol the model documents: it persists a subscription under
/// an ARN of the topic, and reports that ARN only once the subscription is confirmed —
/// unless `ReturnSubscriptionArn` asks for it in every case.
#[tokio::test]
async fn subscribe_reports_an_unconfirmed_subscription_as_pending() {
    let sns = Sns::new();
    // The protocol catalog from the `SubscribeInput$Protocol` doc; the ones addressed
    // by URL or email need the subscriber to confirm.
    let cases: &[(&str, &str, bool)] = &[
        ("http", "http://example.test/hook", true),
        ("https", "https://example.test/hook", true),
        ("email", "ops@example.test", true),
        ("email-json", "ops@example.test", true),
        ("sms", "+15550100", false),
        ("sqs", "arn:aws:sqs:us-east-1:000000000000:q", false),
        (
            "application",
            "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/00000000-0000-4000-8000-000000000000",
            false,
        ),
        ("lambda", "arn:aws:lambda:us-east-1:000000000000:function:f", false),
        ("firehose", "arn:aws:firehose:us-east-1:000000000000:deliverystream/s", false),
    ];

    for (protocol, endpoint, needs_confirmation) in cases {
        let topic = sns.topic(&format!("topic-{}", protocol.replace('-', ""))).await;
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", &topic),
            ("Protocol", protocol),
            ("Endpoint", endpoint),
        ];
        // "Specifying a valid ARN for this attribute is required for Firehose delivery
        // stream subscriptions" (`SubscribeInput$Attributes` doc).
        if *protocol == "firehose" {
            params.push(("Attributes.entry.1.key", "SubscriptionRoleArn"));
            params.push((
                "Attributes.entry.1.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let answer = sns.call("Subscribe", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {protocol:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let reported = answer.element("SubscriptionArn").unwrap_or_default();
        if *needs_confirmation {
            assert_eq!(
                reported, "pending confirmation",
                "case {protocol:?}: an unconfirmed subscription must not report its ARN"
            );
        } else {
            assert!(
                reported.starts_with(&format!("{topic}:")),
                "case {protocol:?}: expected an ARN under the topic, got {reported:?}"
            );
        }

        // With ReturnSubscriptionArn the ARN is reported either way — and it is the
        // SAME subscription, because Subscribe is idempotent in the endpoint.
        let mut repeat: Vec<(&str, &str)> = vec![
            ("TopicArn", &topic),
            ("Protocol", protocol),
            ("Endpoint", endpoint),
            ("ReturnSubscriptionArn", "true"),
        ];
        if *protocol == "firehose" {
            repeat.push(("Attributes.entry.1.key", "SubscriptionRoleArn"));
            repeat.push((
                "Attributes.entry.1.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let again = sns.call("Subscribe", &repeat).await;
        let arn = again.element("SubscriptionArn").unwrap_or_default();
        assert!(
            arn.starts_with(&format!("{topic}:")),
            "case {protocol:?}: expected the subscription ARN, got {arn:?} / {}",
            again.body
        );

        let listed = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
            .await;
        // The listing publishes the ARN only once the subscription is confirmed; while
        // it is pending it carries the sentinel in the ARN's place.
        let expected_in_listing = if *needs_confirmation {
            "PendingConfirmation".to_string()
        } else {
            arn.clone()
        };
        assert_eq!(
            listed.elements("SubscriptionArn"),
            vec![expected_in_listing],
            "case {protocol:?}: the topic must hold exactly the one subscription, got {}",
            listed.body
        );
        assert_eq!(
            listed.elements("Protocol"),
            vec![(*protocol).to_string()],
            "case {protocol:?}: the listed protocol must be the one subscribed, got {}",
            listed.body
        );
        assert_eq!(
            listed.elements("Endpoint"),
            vec![(*endpoint).to_string()],
            "case {protocol:?}: the listed endpoint must be the one subscribed, got {}",
            listed.body
        );
    }
}

/// One re-subscribe case: its name, the form parameters it carries, and whether the
/// request asks for what the subscription already holds.
type ReSubscribe<'a> = (&'a str, &'a [(&'static str, &'static str)], bool);

/// A re-subscribe of the same (protocol, endpoint) pair answers the subscription that
/// already exists only when it asks for the attributes that subscription already holds;
/// one asking for anything else is refused and changes nothing.
#[tokio::test]
async fn re_subscribing_is_idempotent_only_in_the_attributes_already_held() {
    let sns = Sns::new();
    let topic = sns.topic("attributed-subscription").await;
    let subscribe = |attributes: &[(&'static str, &'static str)]| {
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", topic.as_str()),
            ("Protocol", "sqs"),
            ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
        ];
        params.extend(attributes.iter().copied());
        params
    };

    let first = sns
        .call(
            "Subscribe",
            &subscribe(&[
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "true"),
            ]),
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the first subscribe must succeed, got {} / {}",
        first.status, first.body
    );
    let arn = first.element("SubscriptionArn").unwrap_or_default();

    // The sentence SNS refuses a conflicting re-subscribe with: entirely fixed text,
    // naming neither the attribute nor either value.
    let conflict = "Invalid parameter: Attributes Reason: Subscription already exists with \
                    different attributes";

    // (case, attributes asked for, whether the request asks for what is already held)
    let cases: &[ReSubscribe] = &[
        ("no attributes at all", &[], true),
        (
            "the attribute value the subscription holds",
            &[
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "true"),
            ],
            true,
        ),
        (
            "the same value written in another case",
            &[
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "TRUE"),
            ],
            true,
        ),
        (
            "another value for an attribute it holds",
            &[
                ("Attributes.entry.1.key", "RawMessageDelivery"),
                ("Attributes.entry.1.value", "false"),
            ],
            false,
        ),
        (
            "an attribute the subscription does not hold",
            &[
                ("Attributes.entry.1.key", "FilterPolicy"),
                ("Attributes.entry.1.value", r#"{"kind":["alert"]}"#),
            ],
            false,
        ),
        (
            "the removal of an attribute it does not hold",
            &[
                ("Attributes.entry.1.key", "FilterPolicy"),
                ("Attributes.entry.1.value", ""),
            ],
            true,
        ),
    ];

    for (case, attributes, idempotent) in cases {
        let answer = sns.call("Subscribe", &subscribe(attributes)).await;
        if *idempotent {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.element("SubscriptionArn").unwrap_or_default(),
                arn,
                "case {case:?}: an idempotent re-subscribe must answer the same subscription \
                 ARN"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
            assert_eq!(
                answer.element("Message").unwrap_or_default(),
                conflict,
                "case {case:?}: expected SNS's own sentence, got {}",
                answer.body
            );
        }
        // Whichever way it was answered, the subscription is the one the first call
        // created, holding exactly what that call gave it.
        let stored = sns.state();
        let stored = stored.lock();
        let held = stored
            .subscriptions
            .get(&arn)
            .expect("the subscription must still be stored");
        assert_eq!(
            held.attributes.get("RawMessageDelivery").map(String::as_str),
            Some("true"),
            "case {case:?}: the stored subscription must keep its attribute, got {:?}",
            held.attributes
        );
        assert_eq!(
            held.attributes.get("FilterPolicy"),
            None,
            "case {case:?}: a refused re-subscribe must store nothing, got {:?}",
            held.attributes
        );
        assert_eq!(
            stored.subscriptions.len(),
            1,
            "case {case:?}: a re-subscribe must not add a second subscription"
        );
    }
}

/// Subscribe refuses a topic that is not in the store (404) and a protocol the model
/// does not document (400).
#[tokio::test]
async fn subscribe_refuses_an_unknown_topic_and_an_unknown_protocol() {
    let sns = Sns::new();
    let topic = sns.topic("known-topic").await;

    let absent = sns
        .call(
            "Subscribe",
            &[
                (
                    "TopicArn",
                    "arn:aws:sns:us-east-1:000000000000:does-not-exist",
                ),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
        )
        .await;
    assert_eq!(
        absent.status, 404,
        "an unknown topic must be 404, got {} / {}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.code(),
        "NotFound",
        "an unknown topic must carry the NotFound code, got {}",
        absent.body
    );

    for protocol in ["carrier-pigeon", "HTTP3", "", "sq"] {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", protocol),
                    ("Endpoint", "somewhere"),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {protocol:?}: an unknown protocol must be 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {protocol:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }
}

/// Publish answers a fresh, unique MessageId per call and records the message against
/// every confirmed subscription of the topic — and against no unconfirmed one.
#[tokio::test]
async fn publish_delivers_every_message_to_each_confirmed_subscription() {
    let sns = Sns::new();
    let topic = sns.topic("fanout-topic").await;
    let queue = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("the sqs subscription ARN");
    let lambda = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "lambda"),
                ("Endpoint", "arn:aws:lambda:us-east-1:000000000000:function:f"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("the lambda subscription ARN");
    let pending = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "https"),
                ("Endpoint", "https://example.test/hook"),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("the https subscription ARN");

    let big = "x".repeat(200_000);
    // Rows differ along what a message can vary in: size, encoding, subject and
    // whether the body is a protocol-keyed JSON structure.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("plain text", vec![("Message", "hello")]),
        (
            "with a subject",
            vec![("Message", "hello"), ("Subject", "hi there")],
        ),
        ("non-ascii", vec![("Message", "héllo → ☃")]),
        ("one byte", vec![("Message", "x")]),
        ("200 KiB", vec![("Message", big.as_str())]),
        (
            "json message structure",
            vec![
                ("Message", r#"{"default":"d","https":"h"}"#),
                ("MessageStructure", "json"),
            ],
        ),
        // The member names the structure SNS reads the body under; a value naming no
        // structure it defines leaves the body one message for every protocol.
        (
            "a message structure SNS defines nothing for",
            vec![("Message", "hello"), ("MessageStructure", "xml")],
        ),
    ];

    let mut ids = HashSet::new();
    for (case, params) in &cases {
        let mut full = vec![("TopicArn", topic.as_str())];
        full.extend(params.iter().copied());
        let answer = sns.call("Publish", &full).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let id = answer.element("MessageId").unwrap_or_default();
        assert!(
            !id.is_empty(),
            "case {case:?}: expected a non-empty MessageId, got {}",
            answer.body
        );
        // SNS message ids are 8-4-4-4-12 lowercase hexadecimal.
        let groups: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected an 8-4-4-4-12 MessageId, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                && id
                    .chars()
                    .all(|c| !c.is_ascii_uppercase()),
            "case {case:?}: expected lowercase hexadecimal, got {id:?}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: MessageId {id} was already used by an earlier publish"
        );
        assert!(
            answer.element("SequenceNumber").is_none(),
            "case {case:?}: a standard topic has no SequenceNumber, got {}",
            answer.body
        );
    }

    let stored = sns.state();
    let stored = stored.lock();
    for subscription in [&queue, &lambda] {
        let delivered = stored
            .delivered
            .get(subscription)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            delivered.len(),
            cases.len(),
            "case {subscription:?}: expected every published message to be delivered, got {}",
            delivered.len()
        );
        assert_eq!(
            delivered
                .iter()
                .map(|m| m.message_id.clone())
                .collect::<HashSet<_>>(),
            ids,
            "case {subscription:?}: the delivered message ids must be the published ones"
        );
        assert_eq!(
            delivered[1].subject.as_deref(),
            Some("hi there"),
            "case {subscription:?}: the delivered message must keep its subject"
        );
        assert_eq!(
            delivered[4].message.len(),
            big.len(),
            "case {subscription:?}: the delivered message must keep its body"
        );
    }
    assert!(
        !stored.delivered.contains_key(&pending),
        "an unconfirmed subscription must receive nothing, got {:?}",
        stored.delivered.get(&pending)
    );
}

/// One refused call: the case name, its form parameters, and the status and awsQuery
/// error code it must answer with.
type Refusal<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// Publish refuses what it cannot deliver: no destination at all, a topic that is not
/// in the store, a `json` body that is not a JSON object, a `json` body with no
/// `default` member, and a subject outside the documented rule.
#[tokio::test]
async fn publish_refuses_a_request_it_cannot_deliver() {
    let sns = Sns::new();
    let topic = sns.topic("refusal-topic").await;
    let long_subject = "s".repeat(101);
    let cases: Vec<Refusal> = vec![
        ("no destination", vec![("Message", "m")], 400, "InvalidParameter"),
        (
            "both a topic and a target",
            vec![
                ("Message", "m"),
                ("TopicArn", topic.as_str()),
                ("TargetArn", topic.as_str()),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "absent topic",
            vec![
                ("Message", "m"),
                ("TopicArn", "arn:aws:sns:us-east-1:000000000000:absent"),
            ],
            404,
            "NotFound",
        ),
        (
            "a TopicArn that is not an SNS ARN",
            vec![("Message", "m"), ("TopicArn", "not-an-arn")],
            400,
            "InvalidParameter",
        ),
        (
            "json structure that is not JSON",
            vec![
                ("Message", "not json"),
                ("TopicArn", topic.as_str()),
                ("MessageStructure", "json"),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "json structure with no default member",
            vec![
                ("Message", r#"{"https":"h"}"#),
                ("TopicArn", topic.as_str()),
                ("MessageStructure", "json"),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "json structure whose default member names no body",
            vec![
                ("Message", r#"{"default":{"body":"d"}}"#),
                ("TopicArn", topic.as_str()),
                ("MessageStructure", "json"),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "json structure whose default member is a list",
            vec![
                ("Message", r#"{"default":["d"]}"#),
                ("TopicArn", topic.as_str()),
                ("MessageStructure", "json"),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "an empty message",
            vec![("Message", ""), ("TopicArn", topic.as_str())],
            400,
            "InvalidParameter",
        ),
        (
            "a subject one character past the bound",
            vec![
                ("Message", "m"),
                ("TopicArn", topic.as_str()),
                ("Subject", long_subject.as_str()),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "a subject with a control character",
            vec![
                ("Message", "m"),
                ("TopicArn", topic.as_str()),
                ("Subject", "two\nlines"),
            ],
            400,
            "InvalidParameter",
        ),
        (
            "a deduplication id on a standard topic",
            vec![
                ("Message", "m"),
                ("TopicArn", topic.as_str()),
                ("MessageDeduplicationId", "d1"),
            ],
            400,
            "InvalidParameter",
        ),
    ];

    for (case, params, status, code) in &cases {
        let answer = sns.call("Publish", params).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *code,
            "case {case:?}: expected the {code} code, got {}",
            answer.body
        );
    }

    let stored = sns.state();
    assert!(
        stored.lock().delivered.is_empty(),
        "a refused publish must deliver nothing"
    );
}

/// What Publish cannot emulate it answers with the scaffold's 501, never with a
/// success: an SMS destination and a platform-endpoint target, whose own operations
/// are all still scaffolded.
#[tokio::test]
async fn publish_answers_501_for_the_destinations_it_does_not_emulate() {
    let sns = Sns::new();
    let cases: &[(&str, Vec<(&str, &str)>)] = &[
        (
            "a phone number",
            vec![("Message", "m"), ("PhoneNumber", "+15550100")],
        ),
        (
            "a platform endpoint target",
            vec![
                ("Message", "m"),
                (
                    "TargetArn",
                    "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/00000000-0000-4000-8000-000000000000",
                ),
            ],
        ),
    ];

    for (case, params) in cases {
        let answer = sns.call("Publish", params).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?}: expected 501, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "NotImplemented",
            "case {case:?}: expected the NotImplemented code, got {}",
            answer.body
        );
    }
}

/// A FIFO topic needs the ordering parameters SNS documents for it, and every accepted
/// publish gets a strictly increasing SequenceNumber.
#[tokio::test]
async fn publish_to_a_fifo_topic_sequences_messages_and_requires_its_parameters() {
    let sns = Sns::new();
    let plain_fifo = sns.topic("ordered.fifo").await;
    let content_fifo = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "hashed.fifo"),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
                ("Attributes.entry.2.key", "ContentBasedDeduplication"),
                ("Attributes.entry.2.value", "true"),
            ],
        )
        .await
        .element("TopicArn")
        .expect("the FIFO topic ARN");

    // The documented refusals: a FIFO publish carries a MessageGroupId, and a
    // MessageDeduplicationId unless the topic derives one from the body.
    let refusals: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "no group id",
            vec![("TopicArn", plain_fifo.as_str()), ("Message", "m")],
        ),
        (
            "no deduplication id",
            vec![
                ("TopicArn", plain_fifo.as_str()),
                ("Message", "m"),
                ("MessageGroupId", "g1"),
            ],
        ),
        (
            "content based topic still needs a group id",
            vec![("TopicArn", content_fifo.as_str()), ("Message", "m")],
        ),
    ];
    for (case, params) in &refusals {
        let answer = sns.call("Publish", params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {case:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }

    let mut sequence_numbers = Vec::new();
    for (case, dedup) in [("explicit id", Some("d1")), ("another explicit id", Some("d2"))] {
        let mut params = vec![
            ("TopicArn", plain_fifo.as_str()),
            ("Message", "m"),
            ("MessageGroupId", "g1"),
        ];
        if let Some(dedup) = dedup {
            params.push(("MessageDeduplicationId", dedup));
        }
        let answer = sns.call("Publish", &params).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let sequence = answer.element("SequenceNumber").unwrap_or_default();
        assert!(
            !sequence.is_empty(),
            "case {case:?}: a FIFO publish must answer a SequenceNumber, got {}",
            answer.body
        );
        sequence_numbers.push(sequence);
    }
    // A content-based FIFO topic needs no deduplication id: SNS derives one.
    let derived = sns
        .call(
            "Publish",
            &[
                ("TopicArn", content_fifo.as_str()),
                ("Message", "m"),
                ("MessageGroupId", "g1"),
            ],
        )
        .await;
    assert_eq!(
        derived.status, 200,
        "a content-based FIFO publish needs no deduplication id, got {} / {}",
        derived.status, derived.body
    );
    sequence_numbers.push(derived.element("SequenceNumber").unwrap_or_default());

    let mut sorted = sequence_numbers.clone();
    sorted.sort();
    assert_eq!(
        sorted, sequence_numbers,
        "sequence numbers must increase with every publish, got {sequence_numbers:?}"
    );
    let unique: HashSet<&String> = sequence_numbers.iter().collect();
    assert_eq!(
        unique.len(),
        sequence_numbers.len(),
        "sequence numbers must be unique, got {sequence_numbers:?}"
    );
}

/// GetTopicAttributes derives its subscription counts from the subscription store, and
/// 404s once the topic is gone.
#[tokio::test]
async fn get_topic_attributes_derives_its_subscription_counts() {
    let sns = Sns::new();
    let topic = sns.topic("counted-topic").await;

    let counts = |answer: &Answer| -> (String, String, String) {
        let map = attribute_map(answer);
        (
            map.get("SubscriptionsConfirmed").cloned().unwrap_or_default(),
            map.get("SubscriptionsPending").cloned().unwrap_or_default(),
            map.get("SubscriptionsDeleted").cloned().unwrap_or_default(),
        )
    };

    let fresh = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        counts(&fresh),
        ("0".to_string(), "0".to_string(), "0".to_string()),
        "case \"fresh topic\": expected no subscriptions, got {}",
        fresh.body
    );

    // Two confirmed endpoints and one awaiting confirmation.
    let mut confirmed = Vec::new();
    for (protocol, endpoint) in [
        ("sqs", "arn:aws:sqs:us-east-1:000000000000:q1"),
        ("sqs", "arn:aws:sqs:us-east-1:000000000000:q2"),
    ] {
        confirmed.push(
            sns.call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", protocol),
                    ("Endpoint", endpoint),
                ],
            )
            .await
            .element("SubscriptionArn")
            .expect("a subscription ARN"),
        );
    }
    sns.call(
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "email"),
            ("Endpoint", "ops@example.test"),
        ],
    )
    .await;

    let subscribed = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        counts(&subscribed),
        ("2".to_string(), "1".to_string(), "0".to_string()),
        "case \"two confirmed, one pending\": got {}",
        subscribed.body
    );

    let removed = sns
        .call("Unsubscribe", &[("SubscriptionArn", &confirmed[0])])
        .await;
    assert_eq!(
        removed.status, 200,
        "Unsubscribe must succeed, got {} / {}",
        removed.status, removed.body
    );
    let after = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        counts(&after),
        ("1".to_string(), "1".to_string(), "1".to_string()),
        "case \"after one unsubscribe\": got {}",
        after.body
    );

    // A read must not change what the next read sees.
    let again = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        counts(&again),
        counts(&after),
        "GetTopicAttributes must not change the state it reports, got {}",
        again.body
    );

    let absent = sns
        .call(
            "GetTopicAttributes",
            &[("TopicArn", "arn:aws:sns:us-east-1:000000000000:absent")],
        )
        .await;
    assert_eq!(
        absent.status, 404,
        "an unknown topic must be 404, got {} / {}",
        absent.status, absent.body
    );
    assert_eq!(absent.code(), "NotFound", "got {}", absent.body);
}

/// Unsubscribe removes the subscription from the listing, is idempotent in a
/// well-formed ARN, and refuses a `SubscriptionArn` that is not one.
#[tokio::test]
async fn unsubscribe_removes_the_subscription_from_the_listing() {
    let sns = Sns::new();
    let topic = sns.topic("unsubscribe-topic").await;
    let mut arns = Vec::new();
    for endpoint in [
        "arn:aws:sqs:us-east-1:000000000000:q1",
        "arn:aws:sqs:us-east-1:000000000000:q2",
        "arn:aws:sqs:us-east-1:000000000000:q3",
    ] {
        arns.push(
            sns.call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", endpoint),
                ],
            )
            .await
            .element("SubscriptionArn")
            .expect("a subscription ARN"),
        );
    }

    for (removed, arn) in arns.iter().enumerate() {
        let answer = sns.call("Unsubscribe", &[("SubscriptionArn", arn)]).await;
        assert_eq!(
            answer.status, 200,
            "case {arn:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let listed = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
            .await;
        let remaining = listed.elements("SubscriptionArn");
        assert_eq!(
            remaining.len(),
            arns.len() - removed - 1,
            "case {arn:?}: the listing must drop the unsubscribed subscription, got {remaining:?}"
        );
        assert!(
            !remaining.contains(arn),
            "case {arn:?}: the unsubscribed subscription must be gone, got {remaining:?}"
        );

        // Deleting a subscription is idempotent in its ARN: the second call has
        // nothing left to do and is answered the same way.
        let repeat = sns.call("Unsubscribe", &[("SubscriptionArn", arn)]).await;
        assert_eq!(
            repeat.status, 200,
            "case {arn:?}: a second Unsubscribe must be 200, got {} / {}",
            repeat.status, repeat.body
        );
    }

    // A well-formed ARN no subscription carries is the idempotent case too; a value
    // that is not a subscription ARN at all is refused as bad input.
    let unknown = sns
        .call(
            "Unsubscribe",
            &[(
                "SubscriptionArn",
                "arn:aws:sns:us-east-1:000000000000:unsubscribe-topic:\
                 00000000-0000-4000-8000-000000000000",
            )],
        )
        .await;
    assert_eq!(
        unknown.status, 200,
        "an unknown but well-formed ARN must be 200, got {} / {}",
        unknown.status, unknown.body
    );
    for malformed in [
        "not-an-arn-at-all",
        "arn:aws:sns:us-east-1:000000000000:unsubscribe-topic",
    ] {
        let answer = sns
            .call("Unsubscribe", &[("SubscriptionArn", malformed)])
            .await;
        assert_eq!(
            answer.status, 400,
            "case {malformed:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {malformed:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }
}

/// DeleteTopic removes the topic and everything hanging off it, and succeeds whether
/// or not the topic is there.
#[tokio::test]
async fn delete_topic_is_idempotent_and_removes_its_subscriptions() {
    let sns = Sns::new();
    let topic = sns.topic("deleted-topic").await;
    let kept = sns.topic("kept-topic").await;
    let subscription = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("a subscription ARN");
    sns.call(
        "Publish",
        &[("TopicArn", &topic), ("Message", "before the delete")],
    )
    .await;

    for attempt in ["first", "second"] {
        let answer = sns.call("DeleteTopic", &[("TopicArn", &topic)]).await;
        assert_eq!(
            answer.status, 200,
            "case {attempt:?}: DeleteTopic is idempotent, got {} / {}",
            answer.status, answer.body
        );
    }

    let read = sns.call("GetTopicAttributes", &[("TopicArn", &topic)]).await;
    assert_eq!(
        read.status, 404,
        "a deleted topic must be 404, got {} / {}",
        read.status, read.body
    );
    let resubscribe = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
        )
        .await;
    assert_eq!(
        resubscribe.status, 404,
        "subscribing to a deleted topic must be 404, got {} / {}",
        resubscribe.status, resubscribe.body
    );
    let listed = sns.call("ListTopics", &[]).await;
    assert_eq!(
        listed.elements("TopicArn"),
        vec![kept.clone()],
        "only the surviving topic must be listed, got {}",
        listed.body
    );

    let stored = sns.state();
    let stored = stored.lock();
    assert!(
        !stored.subscriptions.contains_key(&subscription),
        "the deleted topic's subscriptions must be gone"
    );
    assert!(
        !stored.delivered.contains_key(&subscription),
        "the deleted subscription's deliveries must be gone"
    );
}

/// Both listings page at the documented page size: a full page carries a NextToken, the
/// last page carries none, and a token that is not one SNS issued is refused.
#[tokio::test]
async fn the_listings_page_at_the_documented_page_size() {
    let sns = Sns::new();
    const TOPICS: usize = 101;
    for index in 0..TOPICS {
        sns.topic(&format!("paged-topic-{index:03}")).await;
    }

    let first = sns.call("ListTopics", &[]).await;
    let first_page = first.elements("TopicArn");
    assert_eq!(
        first_page.len(),
        100,
        "the first page must hold the full page size, got {}",
        first_page.len()
    );
    let token = first
        .element("NextToken")
        .expect("a full first page must carry a NextToken");

    let second = sns.call("ListTopics", &[("NextToken", &token)]).await;
    let second_page = second.elements("TopicArn");
    assert_eq!(
        second_page.len(),
        TOPICS - 100,
        "the last page must hold the remainder, got {}",
        second_page.len()
    );
    assert!(
        second.element("NextToken").is_none(),
        "the last page must carry no NextToken, got {}",
        second.body
    );
    let mut all = first_page.clone();
    all.extend(second_page);
    assert_eq!(
        all.iter().collect::<HashSet<_>>().len(),
        TOPICS,
        "the pages together must hold every topic exactly once"
    );

    for bad in ["not-a-token", "-1", "99999999999999999999"] {
        let answer = sns.call("ListTopics", &[("NextToken", bad)]).await;
        assert_eq!(
            answer.status, 400,
            "case {bad:?}: an unissued token must be 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {bad:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        // The sentence is per error class, not per listing: every paginated SNS list
        // refuses a token it did not mint with the same one.
        assert_eq!(
            answer.element("Message").as_deref(),
            Some(INVALID_NEXT_TOKEN_REFUSAL),
            "case {bad:?}: expected the member's refusal sentence, got {}",
            answer.body
        );
    }

    // The same rule on the subscription listing.
    let topic = sns.topic("paged-subscriptions").await;
    for index in 0..TOPICS {
        sns.call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                (
                    "Endpoint",
                    &format!("arn:aws:sqs:us-east-1:000000000000:q{index:03}"),
                ),
            ],
        )
        .await;
    }
    let first = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    assert_eq!(
        first.elements("SubscriptionArn").len(),
        100,
        "the first subscription page must hold the full page size, got {}",
        first.body
    );
    let token = first
        .element("NextToken")
        .expect("a full first page must carry a NextToken");
    let second = sns
        .call(
            "ListSubscriptionsByTopic",
            &[("TopicArn", &topic), ("NextToken", &token)],
        )
        .await;
    assert_eq!(
        second.elements("SubscriptionArn").len(),
        TOPICS - 100,
        "the last subscription page must hold the remainder, got {}",
        second.body
    );
    // A token the subscription listing did not mint is refused with the same sentence
    // every paginated SNS list states for the member.
    let unissued = sns
        .call(
            "ListSubscriptionsByTopic",
            &[("TopicArn", &topic), ("NextToken", "not-a-token")],
        )
        .await;
    assert_eq!(
        unissued.status, 400,
        "an unissued subscription-listing token must be 400, got {} / {}",
        unissued.status, unissued.body
    );
    assert_eq!(
        unissued.element("Message").as_deref(),
        Some(INVALID_NEXT_TOKEN_REFUSAL),
        "an unissued subscription-listing token: expected the member's refusal \
         sentence, got {}",
        unissued.body
    );
    let absent = sns
        .call(
            "ListSubscriptionsByTopic",
            &[("TopicArn", "arn:aws:sns:us-east-1:000000000000:absent")],
        )
        .await;
    assert_eq!(
        absent.status, 404,
        "listing an unknown topic must be 404, got {} / {}",
        absent.status, absent.body
    );
}

/// Both caller-driven record stores are bounded: at the ceiling a further create is
/// refused with the modelled limit error, and the records already held are untouched.
#[tokio::test]
async fn the_record_stores_are_bounded_by_the_emulators_ceiling() {
    let sns = Sns::new();
    for index in 0..MAX_RECORDS_PER_STORE {
        let answer = sns
            .call("CreateTopic", &[("Name", &format!("bounded-{index:05}"))])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {index}: a topic below the ceiling must be created, got {} / {}",
            answer.status, answer.body
        );
    }
    let over = sns
        .call("CreateTopic", &[("Name", "one-topic-too-many")])
        .await;
    assert_eq!(
        over.status, 403,
        "case \"topic ceiling\": expected 403, got {} / {}",
        over.status, over.body
    );
    assert_eq!(
        over.code(),
        "TopicLimitExceeded",
        "case \"topic ceiling\": expected the TopicLimitExceeded code, got {}",
        over.body
    );
    // An idempotent re-create of a topic already held still succeeds at the ceiling.
    let existing = sns
        .call("CreateTopic", &[("Name", "bounded-00000")])
        .await;
    assert_eq!(
        existing.status, 200,
        "case \"re-create at the ceiling\": expected 200, got {} / {}",
        existing.status, existing.body
    );

    let topic = "arn:aws:sns:us-east-1:000000000000:bounded-00000".to_string();
    for index in 0..MAX_RECORDS_PER_STORE {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", "sqs"),
                    (
                        "Endpoint",
                        &format!("arn:aws:sqs:us-east-1:000000000000:q{index:05}"),
                    ),
                ],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {index}: a subscription below the ceiling must be created, got {} / {}",
            answer.status, answer.body
        );
    }
    let over = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:one-too-many"),
            ],
        )
        .await;
    assert_eq!(
        over.status, 403,
        "case \"subscription ceiling\": expected 403, got {} / {}",
        over.status, over.body
    );
    assert_eq!(
        over.code(),
        "SubscriptionLimitExceeded",
        "case \"subscription ceiling\": expected the SubscriptionLimitExceeded code, got {}",
        over.body
    );

    let stored = sns.state();
    let stored = stored.lock();
    assert_eq!(
        stored.topics.len(),
        MAX_RECORDS_PER_STORE,
        "the topic store must stay at its ceiling"
    );
    assert_eq!(
        stored.subscriptions.len(),
        MAX_RECORDS_PER_STORE,
        "the subscription store must stay at its ceiling"
    );
}

/// The delivery log is bounded in count per subscription and in payload bytes overall,
/// keeping the most recent deliveries and evicting the oldest — so an unbounded publish
/// loop cannot grow the process.
#[tokio::test]
async fn the_delivery_log_is_bounded_and_keeps_the_newest_messages() {
    let sns = Sns::new();
    let topic = sns.topic("retention-topic").await;
    let subscription = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("a subscription ARN");

    // Rows differ in payload size, which is what decides whether the count bound or
    // the byte bound binds first.
    let cases: &[(&str, usize, usize)] = &[
        ("small payloads, count bound", 16, MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION + 50),
        ("quarter-megabyte payloads, byte bound", 250_000, 40),
    ];

    for (case, payload, publishes) in cases {
        let message = "m".repeat(*payload);
        let mut ids = Vec::new();
        for index in 0..*publishes {
            let answer = sns
                .call(
                    "Publish",
                    &[("TopicArn", &topic), ("Message", message.as_str())],
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} publish {index}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            ids.push(answer.element("MessageId").unwrap_or_default());
        }

        let stored = sns.state();
        let stored = stored.lock();
        let held = stored
            .delivered
            .get(&subscription)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert!(
            held.len() <= MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION,
            "case {case:?}: at most {MAX_RETAINED_MESSAGES_PER_SUBSCRIPTION} messages may be \
             retained per subscription, got {}",
            held.len()
        );
        assert!(
            stored.retained_delivery_bytes() <= MAX_RETAINED_DELIVERY_BYTES,
            "case {case:?}: at most {MAX_RETAINED_DELIVERY_BYTES} payload bytes may be \
             retained, got {}",
            stored.retained_delivery_bytes()
        );
        // The accounted total must be the real one: every retained record's own cost,
        // summed. An unaccounted field would let a caller grow memory for free.
        let accounted: usize = stored
            .delivered
            .values()
            .flat_map(|held| held.iter())
            .map(|message| message.retained_bytes())
            .sum();
        assert_eq!(
            stored.retained_delivery_bytes(),
            accounted,
            "case {case:?}: the accounted bytes must equal what the log really holds"
        );
        assert!(
            !held.is_empty(),
            "case {case:?}: the log must still hold the most recent deliveries"
        );
        // What survives is the tail: the newest delivery is always retained, and the
        // retained run is contiguous and ends with it.
        let retained: Vec<String> = held.iter().map(|m| m.message_id.clone()).collect();
        let expected_tail: Vec<String> = ids[ids.len() - retained.len()..].to_vec();
        assert_eq!(
            retained, expected_tail,
            "case {case:?}: the retained deliveries must be the newest ones, oldest evicted"
        );
    }

    // Unsubscribing releases the whole log for that subscription.
    sns.call("Unsubscribe", &[("SubscriptionArn", &subscription)])
        .await;
    let stored = sns.state();
    let stored = stored.lock();
    assert!(
        !stored.delivered.contains_key(&subscription),
        "the removed subscription's deliveries must be released"
    );
    assert_eq!(
        stored.retained_delivery_bytes(),
        0,
        "every retained byte must be released with the last subscription"
    );
}

/// The ordering ids follow the rule the model documents for them: 1 to 128
/// alphanumeric or punctuation characters. That rule is also what bounds the size of
/// the fields a retained delivery carries beside its body.
#[tokio::test]
async fn publish_enforces_the_documented_ordering_id_rule() {
    let sns = Sns::new();
    let topic = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "ordering-ids.fifo"),
                ("Attributes.entry.1.key", "FifoTopic"),
                ("Attributes.entry.1.value", "true"),
                ("Attributes.entry.2.key", "ContentBasedDeduplication"),
                ("Attributes.entry.2.value", "true"),
            ],
        )
        .await
        .element("TopicArn")
        .expect("the FIFO topic ARN");
    let subscription = sns
        .call(
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", "arn:aws:sqs:us-east-1:000000000000:q.fifo"),
            ],
        )
        .await
        .element("SubscriptionArn")
        .expect("a subscription ARN");

    let at_bound = "g".repeat(128);
    let past_bound = "g".repeat(129);
    // (case, group id, deduplication id, accepted)
    let cases: &[(&str, &str, Option<&str>, bool)] = &[
        ("a short id", "g1", None, true),
        ("punctuation the doc lists", "g-1_2.3:4", None, true),
        ("at the 128 character bound", &at_bound, None, true),
        ("one past the 128 character bound", &past_bound, None, false),
        ("an embedded space", "g 1", None, false),
        ("non-ascii", "gé", None, false),
        ("an empty id", "", None, false),
        ("a deduplication id past the bound", "g1", Some(&past_bound), false),
        ("a deduplication id at the bound", "g1", Some(&at_bound), true),
    ];

    for (case, group, dedup, accepted) in cases {
        let mut params = vec![
            ("TopicArn", topic.as_str()),
            // One body per case: the topic deduplicates on the body, so repeating one
            // body would make every publish after the first a duplicate of it.
            ("Message", *case),
            ("MessageGroupId", *group),
        ];
        if let Some(dedup) = dedup {
            params.push(("MessageDeduplicationId", dedup));
        }
        let answer = sns.call("Publish", &params).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }

    let stored = sns.state();
    let stored = stored.lock();
    let held = stored
        .delivered
        .get(&subscription)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let long = held
        .iter()
        .find(|message| message.message_group_id.as_deref() == Some(at_bound.as_str()))
        .expect("the accepted 128 character group id must be retained with its delivery");
    assert!(
        long.retained_bytes() >= at_bound.len(),
        "a retained record must cost at least its group id, got {} for {} bytes of id",
        long.retained_bytes(),
        at_bound.len()
    );
    // Every retained field is accounted for, so the memory ceiling sees the ids too.
    let accounted: usize = held
        .iter()
        .map(|message| message.retained_bytes())
        .sum::<usize>();
    assert_eq!(
        stored.retained_delivery_bytes(),
        accounted,
        "the accounted bytes must equal what the log really holds"
    );
}

/// The attribute maps are a CLOSED set of names and bounded in bytes per record, on
/// both stores: SNS refuses an attribute name it does not define, and this emulator
/// also refuses a record whose attributes would outgrow its per-record ceiling, so no
/// repetition of a create or a subscribe can grow one record without limit.
#[tokio::test]
async fn the_attribute_maps_are_a_closed_set_and_bounded_in_bytes() {
    let sns = Sns::new();
    let topic = sns.topic("bounded-attributes").await;
    let under = "v".repeat(100);
    let over = "v".repeat(MAX_ATTRIBUTE_BYTES_PER_RECORD + 1);
    let long_policy = "p".repeat(MAX_DATA_PROTECTION_POLICY_LEN + 1);

    // (case, attribute name, value, accepted)
    let topic_cases: &[(&str, &str, &str, bool)] = &[
        ("a documented name", "DisplayName", under.as_str(), true),
        ("another documented name", "TracingConfig", "PassThrough", true),
        ("a name SNS does not define", "NotAnAttribute", "x", false),
        ("a name with the wrong case", "displayname", "x", false),
        ("a value past the per-record ceiling", "Policy", over.as_str(), false),
    ];
    for (case, name, value, accepted) in topic_cases {
        let answer = sns
            .call(
                "CreateTopic",
                &[
                    ("Name", "bounded-attributes"),
                    ("Attributes.entry.1.key", name),
                    ("Attributes.entry.1.value", value),
                ],
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }

    let over_policy = sns
        .call(
            "CreateTopic",
            &[
                ("Name", "long-policy-topic"),
                ("DataProtectionPolicy", long_policy.as_str()),
            ],
        )
        .await;
    assert_eq!(
        over_policy.status, 400,
        "a DataProtectionPolicy past its documented length must be 400, got {} / {}",
        over_policy.status, over_policy.body
    );

    // (case, attribute name, value, accepted) — the subscription store, same rule. Each
    // row subscribes its OWN endpoint: one topic holds at most one subscription per
    // (protocol, endpoint) pair, and a re-subscribe asking for a different
    // configuration of that pair is refused rather than applied.
    let subscription_cases: &[(&str, &str, &str, &str, bool)] = &[
        (
            "a documented name",
            "arn:aws:sqs:us-east-1:000000000000:q-raw",
            "RawMessageDelivery",
            "true",
            true,
        ),
        (
            "another documented name",
            "arn:aws:sqs:us-east-1:000000000000:q-scope",
            "FilterPolicyScope",
            "MessageBody",
            true,
        ),
        (
            "a name SNS does not define",
            "arn:aws:sqs:us-east-1:000000000000:q-unknown",
            "NotAnAttribute",
            "x",
            false,
        ),
        (
            "a value past the per-record ceiling",
            "arn:aws:sqs:us-east-1:000000000000:q-over",
            "FilterPolicy",
            over.as_str(),
            false,
        ),
    ];
    for (case, endpoint, name, value, accepted) in subscription_cases {
        let answer = sns
            .call(
                "Subscribe",
                &[
                    ("TopicArn", &topic),
                    ("Protocol", "sqs"),
                    ("Endpoint", endpoint),
                    ("Attributes.entry.1.key", name),
                    ("Attributes.entry.1.value", value),
                ],
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }

    // A refused request stored nothing, so neither record outgrew its ceiling.
    let stored = sns.state();
    let stored = stored.lock();
    for (arn, bytes) in stored
        .topics
        .values()
        .map(|topic| {
            (
                topic.arn.clone(),
                topic
                    .attributes
                    .iter()
                    .map(|(name, value)| name.len() + value.len())
                    .sum::<usize>(),
            )
        })
        .chain(stored.subscriptions.values().map(|sub| {
            (
                sub.arn.clone(),
                sub.attributes
                    .iter()
                    .map(|(name, value)| name.len() + value.len())
                    .sum::<usize>(),
            )
        }))
    {
        assert!(
            bytes <= MAX_ATTRIBUTE_BYTES_PER_RECORD,
            "case {arn:?}: a record holds at most {MAX_ATTRIBUTE_BYTES_PER_RECORD} attribute \
             bytes, got {bytes}"
        );
    }
}

/// A topic's `MaximumMessageSize` may only LOWER the ceiling: the documented default
/// is the emulator's hard limit, so a caller cannot buy a bigger per-message payload.
#[tokio::test]
async fn the_maximum_message_size_attribute_can_only_lower_the_ceiling() {
    let sns = Sns::new();
    // (case, configured MaximumMessageSize, message bytes, accepted)
    let cases: &[(&str, Option<&str>, usize, bool)] = &[
        ("default ceiling, at the bound", None, 262_144, true),
        ("default ceiling, one past the bound", None, 262_145, false),
        ("raised ceiling is not honoured", Some("1048576"), 300_000, false),
        ("raised ceiling still accepts the default", Some("1048576"), 262_144, true),
        // The documented floor of the attribute's range, which is also the smallest
        // ceiling a topic can be given.
        ("lowered ceiling, at the bound", Some("1024"), 1_024, true),
        ("lowered ceiling, one past the bound", Some("1024"), 1_025, false),
    ];

    for (index, (case, configured, size, accepted)) in cases.iter().enumerate() {
        let name = format!("sized-topic-{index}");
        let mut params = vec![("Name", name.as_str())];
        if let Some(configured) = configured {
            params.push(("Attributes.entry.1.key", "MaximumMessageSize"));
            params.push(("Attributes.entry.1.value", configured));
        }
        let topic = sns
            .call("CreateTopic", &params)
            .await
            .element("TopicArn")
            .unwrap_or_else(|| panic!("case {case:?}: no TopicArn"));

        let message = "m".repeat(*size);
        let answer = sns
            .call(
                "Publish",
                &[("TopicArn", &topic), ("Message", message.as_str())],
            )
            .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                "InvalidParameter",
                "case {case:?}: expected the InvalidParameter code, got {}",
                answer.body
            );
        }
    }
}
