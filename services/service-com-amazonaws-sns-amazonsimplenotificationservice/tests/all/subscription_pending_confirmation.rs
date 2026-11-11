// SPDX-License-Identifier: CC0-1.0

//! The pending-confirmation half of the SNS subscription model, driven over the wire:
//! every call below is an awsQuery HTTP request dispatched through the router the
//! emulator serves in production, so nothing but persisted state carries information
//! between calls.
//!
//! Three rules are covered, each over a SET of inputs rather than one recorded case:
//!
//! 1. a subscription SNS has not confirmed yet has no ARN SNS publishes, so a listing
//!    carries the `PendingConfirmation` sentinel in `Subscription$SubscriptionArn`
//!    until the subscriber confirms;
//! 2. `Subscribe` refuses, with `InvalidParameterException` (HTTP 400,
//!    `InvalidParameter`), an `Endpoint` that is not in the form the protocol
//!    addresses — the per-protocol catalog of `SubscribeInput$Endpoint` — and writes no
//!    subscription when it does;
//! 3. `Unsubscribe` refuses, the same way, a `SubscriptionArn` that names a
//!    subscription still awaiting confirmation (including the sentinel a listing or a
//!    `Subscribe` response carries in its place) and leaves that subscription in place,
//!    while a confirmed subscription unsubscribes with HTTP 200;
//! 4. `Unsubscribe` is idempotent in a well-formed subscription ARN — an ARN whose
//!    subscription is already gone is answered 200, with nothing left to do — and
//!    refuses a `SubscriptionArn` that is not a subscription ARN at all with HTTP 400.

use std::sync::Arc;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    PENDING_CONFIRMATION_LISTED, PENDING_CONFIRMATION_REPORTED,
};
use service_com_amazonaws_sns_amazonsimplenotificationservice::{
    ComAmazonawsSnsAmazonsimplenotificationserviceState as SnsState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The emulator under test: one shared engine, one SNS service, one router.
struct Sns {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

/// One answered SNS call.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of the first `<tag>…</tag>` element of the response, if any.
    fn element(&self, tag: &str) -> Option<String> {
        self.elements(tag).into_iter().next()
    }

    /// The text of every `<tag>…</tag>` element of the response, in document order.
    fn elements(&self, tag: &str) -> Vec<String> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let mut found = Vec::new();
        let mut rest = self.body.as_str();
        while let Some(start) = rest.find(&open) {
            rest = &rest[start + open.len()..];
            let Some(end) = rest.find(&close) else { break };
            found.push(rest[..end].to_string());
            rest = &rest[end + close.len()..];
        }
        found
    }

    /// The awsQuery `<Code>` of an error response, or `""` on a success envelope.
    fn code(&self) -> String {
        self.element("Code").unwrap_or_default()
    }
}

impl Sns {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Call `action` with form parameters, signed (in scope only — the emulator does
    /// not verify signatures by default) for `us-east-1`.
    async fn call(&self, action: &str, params: &[(&str, &str)]) -> Answer {
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
            .header("host", "sns.us-east-1.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/us-east-1/sns/\
                 aws4_request, SignedHeaders=host, Signature=0000",
            )
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        Answer {
            status,
            body: String::from_utf8_lossy(response.body()).to_string(),
        }
    }

    /// A topic created in `us-east-1`, with its ARN.
    async fn topic(&self, name: &str) -> String {
        let answer = self.call("CreateTopic", &[("Name", name)]).await;
        assert_eq!(
            answer.status, 200,
            "CreateTopic {name}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        answer
            .element("TopicArn")
            .unwrap_or_else(|| panic!("CreateTopic {name}: no TopicArn in {}", answer.body))
    }

    /// Subscribe `endpoint` over `protocol`, asking for the ARN in every case, and
    /// answer the subscription ARN the call reports.
    ///
    /// A Firehose subscription carries the role SNS delivers as — "specifying a valid
    /// ARN for this attribute is required for Firehose delivery stream subscriptions"
    /// (`SubscribeInput$Attributes` doc, of `SubscriptionRoleArn`).
    async fn subscribe(&self, topic: &str, protocol: &str, endpoint: &str) -> String {
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", topic),
            ("Protocol", protocol),
            ("Endpoint", endpoint),
            ("ReturnSubscriptionArn", "true"),
        ];
        if protocol == "firehose" {
            params.push(("Attributes.entry.1.key", "SubscriptionRoleArn"));
            params.push((
                "Attributes.entry.1.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let answer = self.call("Subscribe", &params).await;
        assert_eq!(
            answer.status, 200,
            "Subscribe {protocol} {endpoint}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        answer
            .element("SubscriptionArn")
            .unwrap_or_else(|| panic!("Subscribe {protocol}: no SubscriptionArn in {}", answer.body))
    }

    /// A read of this service's persisted state, the way a handler reaches it.
    fn state(&self) -> ::engine::ServiceState<SnsState> {
        self.engine.state::<SnsState>()
    }
}

/// Percent-encode a form value: everything but the unreserved set, so an ARN's `:`
/// and `/` survive the round trip.
fn percent_encode(value: &str) -> String {
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

/// The protocols whose subscriber has to confirm, each with an endpoint in its own
/// documented form (`SubscribeInput$Protocol` / `SubscribeInput$Endpoint` docs). Two
/// rows per protocol, so no assertion can pass by matching one recorded string.
const PENDING_PROTOCOLS: &[(&str, &str)] = &[
    ("http", "http://example.test/hook"),
    ("http", "http://10.0.0.1:8080/notify?x=1"),
    ("https", "https://example.test/hook"),
    ("https", "https://deep.example.test/a/b/c"),
    ("email", "ops@example.test"),
    ("email", "first.last+tag@mail.example.co.uk"),
    ("email-json", "ops@example.test"),
    ("email-json", "alerts@sub.example.test"),
];

/// The protocols whose endpoint is an AWS resource the caller already owns, so SNS
/// confirms the subscription itself.
const CONFIRMED_PROTOCOLS: &[(&str, &str)] = &[
    ("sqs", "arn:aws:sqs:us-east-1:000000000000:q"),
    ("sqs", "arn:aws:sqs:eu-west-1:000000000000:other-queue"),
    ("sms", "+15550100"),
    ("sms", "+44 20 7946 0958"),
    (
        "application",
        "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/app/00000000-0000-4000-8000-000000000000",
    ),
    ("lambda", "arn:aws:lambda:us-east-1:000000000000:function:f"),
    (
        "lambda",
        "arn:aws:lambda:ap-southeast-2:000000000000:function:notifier:PROD",
    ),
    (
        "firehose",
        "arn:aws:firehose:us-east-1:000000000000:deliverystream/s",
    ),
];

/// A subscription SNS has not confirmed yet is LISTED with the `PendingConfirmation`
/// sentinel instead of an ARN, whatever protocol put it in that state — and a
/// subscription SNS confirmed itself is listed with its real ARN. The listing is also
/// stable: reading it twice reports the same thing.
#[tokio::test]
async fn a_pending_subscription_is_listed_with_the_pending_confirmation_sentinel() {
    let sns = Sns::new();

    let rows: Vec<(&str, &str, bool)> = PENDING_PROTOCOLS
        .iter()
        .map(|(protocol, endpoint)| (*protocol, *endpoint, true))
        .chain(
            CONFIRMED_PROTOCOLS
                .iter()
                .map(|(protocol, endpoint)| (*protocol, *endpoint, false)),
        )
        .collect();

    for (index, (protocol, endpoint, pending)) in rows.into_iter().enumerate() {
        let case = format!("{protocol} -> {endpoint}");
        let topic = sns.topic(&format!("pending-listing-{index:02}")).await;
        let arn = sns.subscribe(&topic, protocol, endpoint).await;

        let listed = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            listed.status, listed.body
        );
        let expected = if pending {
            PENDING_CONFIRMATION_LISTED.to_string()
        } else {
            arn.clone()
        };
        assert_eq!(
            listed.elements("SubscriptionArn"),
            vec![expected.clone()],
            "case {case:?}: expected the listed ARN {expected:?}, got {}",
            listed.body
        );
        // Whatever the listing reports, the OTHER members still describe the
        // subscription, and the store still holds its real ARN.
        assert_eq!(
            listed.elements("Protocol"),
            vec![protocol.to_string()],
            "case {case:?}: the listed protocol must be the one subscribed, got {}",
            listed.body
        );
        // Whatever the listing reports, the store still holds the subscription under
        // its real ARN, with the pending flag the protocol implies.
        let stored_pending = {
            let stored = sns.state();
            let held = stored.lock();
            held.subscriptions
                .get(&arn)
                .map(|record| record.pending_confirmation)
        };
        assert_eq!(
            stored_pending,
            Some(pending),
            "case {case:?}: expected the stored subscription to be pending={pending}, \
             got {stored_pending:?}"
        );

        // A read must not change what the next read sees.
        let again = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
            .await;
        assert_eq!(
            again.elements("SubscriptionArn"),
            listed.elements("SubscriptionArn"),
            "case {case:?}: a listing must not change what the next one reports, got {}",
            again.body
        );
    }
}

/// One endpoint-validation row: the protocol, the endpoint, and whether SNS accepts
/// that endpoint for that protocol.
type EndpointCase = (&'static str, &'static str, bool);

/// `Subscribe` accepts only an endpoint in the form its protocol addresses, and
/// refuses every other with HTTP 400 / `InvalidParameter` — writing no subscription.
///
/// The accepted forms are the per-protocol catalog of the `SubscribeInput$Endpoint`
/// doc; the refused rows are the same endpoints offered to a protocol that does not
/// address them, plus the shapeless values a client typo produces.
#[tokio::test]
async fn subscribe_refuses_an_endpoint_that_is_not_in_the_protocols_form() {
    let sns = Sns::new();
    let topic = sns.topic("endpoint-validation").await;

    let mut cases: Vec<EndpointCase> = Vec::new();
    for (protocol, endpoint) in PENDING_PROTOCOLS.iter().chain(CONFIRMED_PROTOCOLS) {
        cases.push((*protocol, *endpoint, true));
    }

    cases.extend_from_slice(&[
        // A URL protocol wants its OWN scheme and a host.
        ("http", "https://example.test/hook", false),
        ("https", "http://example.test/hook", false),
        ("http", "example.test/hook", false),
        ("https", "https://", false),
        ("http", "", false),
        // An email address, not a URL and not a bare word.
        ("email", "ops", false),
        ("email", "ops@", false),
        ("email", "@example.test", false),
        ("email", "ops@example", false),
        ("email", "ops example@test.com", false),
        ("email-json", "https://example.test/hook", false),
        // A phone number: digits, at most the 15 an E.164 number holds.
        ("sms", "not-a-number", false),
        ("sms", "+1555", false),
        ("sms", "+1234567890123456", false),
        ("sms", "ops@example.test", false),
        // The ARN of the resource the protocol names — of that service, and of that
        // kind of resource within it.
        ("sqs", "my-queue", false),
        ("sqs", "arn:aws:sns:us-east-1:000000000000:q", false),
        ("sqs", "arn:aws:sqs:us-east-1::q", false),
        ("lambda", "my-function", false),
        (
            "lambda",
            "arn:aws:sqs:us-east-1:000000000000:function:f",
            false,
        ),
        ("lambda", "arn:aws:lambda:us-east-1:000000000000:f", false),
        (
            "lambda",
            "arn:aws:lambda:us-east-1:000000000000:function:",
            false,
        ),
        (
            "firehose",
            "arn:aws:firehose:us-east-1:000000000000:stream/s",
            false,
        ),
        ("firehose", "arn:aws:firehose:us-east-1:000000000000:", false),
        (
            "application",
            "arn:aws:sns:us-east-1:000000000000:app/GCM/x",
            false,
        ),
        ("application", "my-app", false),
    ]);

    for (protocol, endpoint, accepted) in &cases {
        let case = format!("{protocol} -> {endpoint:?}");
        let before = sns.state().lock().subscriptions.len();
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", &topic),
            ("Protocol", protocol),
            ("Endpoint", endpoint),
            ("ReturnSubscriptionArn", "true"),
        ];
        // The role a Firehose subscription is required to carry, so these rows fail (or
        // pass) on the ENDPOINT alone.
        if *protocol == "firehose" {
            params.push(("Attributes.entry.1.key", "SubscriptionRoleArn"));
            params.push((
                "Attributes.entry.1.value",
                "arn:aws:iam::000000000000:role/sns-firehose",
            ));
        }
        let answer = sns.call("Subscribe", &params).await;
        let after = sns.state().lock().subscriptions.len();
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            let arn = answer.element("SubscriptionArn").unwrap_or_default();
            assert!(
                arn.starts_with(&format!("{topic}:")),
                "case {case:?}: expected an ARN under the topic, got {arn:?}"
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
                after, before,
                "case {case:?}: a refused Subscribe must write no subscription, \
                 held {before} then {after}"
            );
        }
    }

    // An absent Endpoint is refused for every protocol: SNS delivers to none without
    // one.
    for (protocol, _) in PENDING_PROTOCOLS.iter().chain(CONFIRMED_PROTOCOLS) {
        let answer = sns
            .call(
                "Subscribe",
                &[("TopicArn", &topic), ("Protocol", protocol)],
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {protocol:?} with no Endpoint: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {protocol:?} with no Endpoint: expected the InvalidParameter code, got {}",
            answer.body
        );
    }
}

/// `Unsubscribe` refuses a subscription that is still awaiting confirmation — by its
/// ARN or by the sentinel a listing reports in that ARN's place — and leaves it in
/// place, while the confirmed subscriptions of the same topic unsubscribe with 200.
#[tokio::test]
async fn unsubscribe_refuses_a_subscription_that_is_pending_confirmation() {
    let sns = Sns::new();
    let topic = sns.topic("pending-unsubscribe").await;

    // Every protocol that leaves a subscription pending, refused by its own ARN.
    for (protocol, endpoint) in PENDING_PROTOCOLS {
        let case = format!("{protocol} -> {endpoint}");
        let arn = sns.subscribe(&topic, protocol, endpoint).await;
        let answer = sns.call("Unsubscribe", &[("SubscriptionArn", &arn)]).await;
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
        assert!(
            sns.state().lock().subscriptions.contains_key(&arn),
            "case {case:?}: a refused Unsubscribe must leave the subscription in place"
        );
    }

    // And by either sentinel a caller can have read out of a response.
    for sentinel in [PENDING_CONFIRMATION_LISTED, PENDING_CONFIRMATION_REPORTED] {
        let answer = sns
            .call("Unsubscribe", &[("SubscriptionArn", sentinel)])
            .await;
        assert_eq!(
            answer.status, 400,
            "case {sentinel:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {sentinel:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }

    let pending_count = PENDING_PROTOCOLS.len();
    assert_eq!(
        sns.state().lock().subscriptions.len(),
        pending_count,
        "every pending subscription must have survived its refused Unsubscribe"
    );

    // A confirmed subscription of the same topic still unsubscribes, and only it goes.
    for (protocol, endpoint) in CONFIRMED_PROTOCOLS {
        let case = format!("{protocol} -> {endpoint}");
        let arn = sns.subscribe(&topic, protocol, endpoint).await;
        let answer = sns.call("Unsubscribe", &[("SubscriptionArn", &arn)]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            answer.body.contains("UnsubscribeResponse"),
            "case {case:?}: expected an UnsubscribeResponse envelope, got {}",
            answer.body
        );
        assert!(
            !sns.state().lock().subscriptions.contains_key(&arn),
            "case {case:?}: the unsubscribed subscription must be gone"
        );
        // Deleting it again has nothing left to do: the delete is idempotent in the ARN.
        let repeat = sns.call("Unsubscribe", &[("SubscriptionArn", &arn)]).await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: a second Unsubscribe must be 200, got {} / {}",
            repeat.status, repeat.body
        );
    }

    assert_eq!(
        sns.state().lock().subscriptions.len(),
        pending_count,
        "the pending subscriptions must be the ones left standing"
    );
    let listed = sns
        .call("ListSubscriptionsByTopic", &[("TopicArn", &topic)])
        .await;
    let reported = listed.elements("SubscriptionArn");
    assert_eq!(
        reported.len(),
        pending_count,
        "the listing must still hold every pending subscription, got {reported:?}"
    );
    assert!(
        reported
            .iter()
            .all(|arn| arn == PENDING_CONFIRMATION_LISTED),
        "every surviving subscription is pending, so every listed ARN must be the \
         sentinel, got {reported:?}"
    );
}

/// `Unsubscribe` is IDEMPOTENT in a well-formed subscription ARN: an ARN whose
/// subscription is already gone — deleted by an earlier call, or deleted with its
/// topic — leaves nothing to do and is answered the same way as the call that did the
/// work. A `SubscriptionArn` that is not a subscription ARN at all is argument
/// validation's business: 400 / `InvalidParameter`, before any lookup.
#[tokio::test]
async fn unsubscribe_is_idempotent_in_a_well_formed_arn_and_refuses_one_that_is_not() {
    let sns = Sns::new();
    let topic = sns.topic("idempotent-unsubscribe").await;
    let live = sns
        .subscribe(&topic, "sqs", "arn:aws:sqs:us-east-1:000000000000:q")
        .await;

    // A subscription of a topic that is then deleted: the ARN outlives the record.
    let doomed_topic = sns.topic("deleted-with-subscriptions").await;
    let doomed = sns
        .subscribe(
            &doomed_topic,
            "sqs",
            "arn:aws:sqs:us-east-1:000000000000:doomed",
        )
        .await;
    let deleted = sns
        .call("DeleteTopic", &[("TopicArn", &doomed_topic)])
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteTopic must succeed, got {} / {}",
        deleted.status, deleted.body
    );

    // Well-formed subscription ARNs OF THIS REGION, each answered 200 however many times
    // it is sent: the live one, the one whose topic is gone, and ones that never existed
    // (an id nothing carries, another topic). An ARN naming another region is not one this
    // endpoint's idempotence covers: it is refused below.
    let well_formed = [
        ("the live subscription", live.clone()),
        ("a subscription deleted with its topic", doomed.clone()),
        (
            "an id no subscription carries",
            format!("{topic}:00000000-0000-4000-8000-000000000000"),
        ),
        (
            "a topic that does not exist",
            "arn:aws:sns:us-east-1:000000000000:absent-topic:\
             11111111-1111-4111-8111-111111111111"
                .to_string(),
        ),
    ];
    for (case, arn) in &well_formed {
        for attempt in ["first", "second"] {
            let answer = sns.call("Unsubscribe", &[("SubscriptionArn", arn)]).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} ({attempt} attempt): expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert!(
                answer.body.contains("UnsubscribeResponse"),
                "case {case:?} ({attempt} attempt): expected an UnsubscribeResponse \
                 envelope, got {}",
                answer.body
            );
        }
    }
    assert!(
        sns.state().lock().subscriptions.is_empty(),
        "every subscription must be gone, and no call may have invented one"
    );

    // And the values that are not subscription ARNs THIS endpoint serves: too few
    // elements, a topic ARN (one element short of a subscription's), an empty element,
    // another service, and a well-formed subscription ARN of another region — which this
    // endpoint does not reach, so the delete is refused rather than answered as the
    // idempotent one.
    let malformed = [
        "not-an-arn-at-all",
        "arn:aws:sns",
        "arn:aws:sns:us-east-1:000000000000:idempotent-unsubscribe",
        "arn:aws:sns:us-east-1:000000000000:idempotent-unsubscribe:",
        "arn:aws:sns::000000000000:idempotent-unsubscribe:11111111",
        "arn:aws:sqs:us-east-1:000000000000:q:11111111",
        "arn:aws:sns:us-east-1:000000000000:two words:11111111",
        "arn:aws:sns:us-east-1:000000000000:topic:id:extra",
        "arn:aws:sns:eu-west-1:123456789012:other-topic:22222222-2222-4222-8222-222222222222",
        "arn:aws-us-gov:sns:us-gov-west-1:000000000000:gov-topic:\
         33333333-3333-4333-8333-333333333333",
    ];
    for arn in malformed {
        let answer = sns.call("Unsubscribe", &[("SubscriptionArn", arn)]).await;
        assert_eq!(
            answer.status, 400,
            "case {arn:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {arn:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
    }
}
