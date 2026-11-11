// SPDX-License-Identifier: CC0-1.0

//! `ListSubscriptions`, the account-wide subscription listing, driven over the wire:
//! every call below is an awsQuery HTTP request dispatched through the router the
//! emulator serves in production, so nothing but persisted state carries information
//! between calls.
//!
//! The rules covered, each over a SET of inputs rather than one recorded case:
//!
//! 1. the listing reports EVERY subscription record the account holds, whatever topic
//!    it belongs to and whatever its endpoint names — nothing resolves an endpoint to
//!    decide whether a subscription is listed, so a subscription whose endpoint ARN
//!    names a resource that does not exist is listed like any other;
//! 2. each listed subscription carries the stored record's `Owner`, `Protocol`,
//!    `Endpoint` and `TopicArn`, and its own ARN unless it is still awaiting
//!    confirmation, where the `PendingConfirmation` sentinel stands in its place;
//! 3. the listing is a page of up to 100 with an opaque `NextToken`, the pages
//!    partition the account's subscriptions, and a token the service did not mint is
//!    refused with `InvalidParameterException` (HTTP 400, `InvalidParameter`);
//! 4. reading the listing changes nothing a later caller sees, and what it reports
//!    follows the store: an unsubscribed subscription, and one deleted with its topic,
//!    leave it.

use std::collections::HashSet;
use std::sync::Arc;

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    INVALID_NEXT_TOKEN_REFUSAL, PAGE_SIZE, PENDING_CONFIRMATION_LISTED,
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

    /// The awsQuery `<Message>` of an error response, or `""` when there is none.
    fn message(&self) -> String {
        self.element("Message").unwrap_or_default()
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
        answer.element("SubscriptionArn").unwrap_or_else(|| {
            panic!("Subscribe {protocol}: no SubscriptionArn in {}", answer.body)
        })
    }

    /// Every subscription ARN `ListSubscriptions` reports, walking every page, with the
    /// number of pages it took. Fails the test on any page that is not answered 200.
    async fn all_listed_subscription_arns(&self, case: &str) -> (Vec<String>, usize) {
        let mut arns: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let params: Vec<(&str, &str)> = match token.as_deref() {
                Some(token) => vec![("NextToken", token)],
                None => Vec::new(),
            };
            let answer = self.call("ListSubscriptions", &params).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} (page {pages}): expected 200, got {} / {}",
                answer.status, answer.body
            );
            pages += 1;
            let page = answer.elements("SubscriptionArn");
            assert!(
                page.len() <= PAGE_SIZE,
                "case {case:?} (page {pages}): a page holds at most {PAGE_SIZE}, got {}",
                page.len()
            );
            arns.extend(page);
            token = answer.element("NextToken");
            if token.is_none() {
                return (arns, pages);
            }
            assert!(
                pages < 64,
                "case {case:?}: the pages must run out, got {pages} of them"
            );
        }
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

/// One subscription to make: the topic index it goes on, the protocol, the endpoint,
/// and whether that protocol leaves the subscription awaiting confirmation.
///
/// The endpoint forms are the per-protocol catalog of the `SubscribeInput$Endpoint`
/// doc. EVERY resource-ARN endpoint here names a resource that does NOT exist in this
/// emulator — no queue, function, stream or device endpoint was created — which is the
/// point: SNS does not resolve the endpoint, so the subscription is minted, confirmed
/// where the protocol says so, and listed either way.
const ROWS: &[(usize, &str, &str, bool)] = &[
    (
        0,
        "sqs",
        "arn:aws:sqs:us-east-1:000000000000:queue-that-was-never-created",
        false,
    ),
    (
        0,
        "sqs",
        "arn:aws:sqs:eu-west-1:000000000000:absent-queue",
        false,
    ),
    (0, "http", "http://example.test/hook", true),
    (1, "https", "https://deep.example.test/a/b/c", true),
    (1, "email", "first.last+tag@mail.example.co.uk", true),
    (1, "email-json", "alerts@sub.example.test", true),
    (
        2,
        "lambda",
        "arn:aws:lambda:ap-southeast-2:000000000000:function:absent-notifier:PROD",
        false,
    ),
    (
        2,
        "firehose",
        "arn:aws:firehose:us-east-1:000000000000:deliverystream/absent-stream",
        false,
    ),
    (
        2,
        "application",
        "arn:aws:sns:us-east-1:000000000000:endpoint/GCM/absent-app/00000000-0000-4000-8000-000000000000",
        false,
    ),
    (3, "sms", "+15550100", false),
];

/// The topics `ROWS` spreads its subscriptions over.
const TOPIC_NAMES: &[&str] = &[
    "account-listing-sqs",
    "account-listing-web",
    "account-listing-aws",
    "account-listing-sms",
];

/// `ListSubscriptions` reports every subscription the account holds — across topics,
/// across protocols, and whether or not the endpoint names anything that exists —
/// with the members of the stored record, and reading it changes nothing.
#[tokio::test]
async fn list_subscriptions_reports_every_subscription_of_the_account() {
    let sns = Sns::new();

    // An account with no subscriptions lists none, and says so with a 200 rather than
    // an error.
    let empty = sns.call("ListSubscriptions", &[]).await;
    assert_eq!(
        empty.status, 200,
        "case \"no subscriptions\": expected 200, got {} / {}",
        empty.status, empty.body
    );
    assert!(
        empty.elements("SubscriptionArn").is_empty(),
        "case \"no subscriptions\": expected an empty list, got {}",
        empty.body
    );
    assert!(
        empty.element("NextToken").is_none(),
        "case \"no subscriptions\": a last page carries no NextToken, got {}",
        empty.body
    );

    let mut topics: Vec<String> = Vec::new();
    for name in TOPIC_NAMES {
        topics.push(sns.topic(name).await);
    }

    // Subscribe every row, remembering what each one should look like in a listing.
    let mut expected: Vec<(String, String, String, String, String)> = Vec::new();
    for (topic_index, protocol, endpoint, pending) in ROWS {
        let topic = topics[*topic_index].clone();
        let arn = sns.subscribe(&topic, protocol, endpoint).await;
        let listed_arn = if *pending {
            PENDING_CONFIRMATION_LISTED.to_string()
        } else {
            arn.clone()
        };
        expected.push((
            arn,
            listed_arn,
            (*protocol).to_string(),
            (*endpoint).to_string(),
            topic,
        ));
    }

    let answer = sns.call("ListSubscriptions", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case \"the whole account\": expected 200, got {} / {}",
        answer.status, answer.body
    );
    let listed_arns = answer.elements("SubscriptionArn");
    assert_eq!(
        listed_arns.len(),
        ROWS.len(),
        "case \"the whole account\": expected {} subscriptions, got {listed_arns:?}",
        ROWS.len()
    );

    let listed_endpoints = answer.elements("Endpoint");
    let listed_protocols = answer.elements("Protocol");
    let listed_topics = answer.elements("TopicArn");
    let owners = answer.elements("Owner");
    let account = {
        let held = sns.state();
        let store = held.lock();
        store
            .subscriptions
            .values()
            .next()
            .map(|record| record.owner.clone())
            .expect("the store holds the subscriptions just made")
    };
    assert!(
        owners.iter().all(|owner| *owner == account),
        "case \"the whole account\": every listed Owner must be the account that owns \
         the subscription ({account}), got {owners:?}"
    );

    for (arn, listed_arn, protocol, endpoint, topic) in &expected {
        let case = format!("{protocol} -> {endpoint}");
        // A pending subscription's sentinel repeats, so locate this row by its
        // endpoint, which is unique across the table, and check the rest line up.
        let row = listed_endpoints
            .iter()
            .position(|found| found == endpoint)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected {endpoint:?} among the listed endpoints, got \
                     {listed_endpoints:?}"
                )
            });
        assert_eq!(
            listed_protocols.get(row),
            Some(protocol),
            "case {case:?}: expected the subscribed protocol, got {:?}",
            listed_protocols.get(row)
        );
        assert_eq!(
            listed_topics.get(row),
            Some(topic),
            "case {case:?}: expected the topic subscribed to, got {:?}",
            listed_topics.get(row)
        );
        assert_eq!(
            listed_arns.get(row),
            Some(listed_arn),
            "case {case:?}: expected the listed ARN {listed_arn:?}, got {:?}",
            listed_arns.get(row)
        );
        // Whatever the listing reports, the store still holds the subscription under
        // its real ARN.
        let held = sns.state();
        let store = held.lock();
        assert!(
            store.subscriptions.contains_key(arn),
            "case {case:?}: the store must still hold {arn:?}"
        );
    }

    // The account-wide listing is the union of the per-topic listings: it selects on
    // nothing but the record's existence.
    let mut by_topic: Vec<String> = Vec::new();
    for topic in &topics {
        let answer = sns
            .call("ListSubscriptionsByTopic", &[("TopicArn", topic)])
            .await;
        assert_eq!(
            answer.status, 200,
            "case {topic:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        by_topic.extend(answer.elements("Endpoint"));
    }
    by_topic.sort();
    let mut account_wide = listed_endpoints.clone();
    account_wide.sort();
    assert_eq!(
        account_wide, by_topic,
        "case \"union over topics\": the account-wide listing must report exactly the \
         per-topic listings, got {account_wide:?} and {by_topic:?}"
    );

    // A read must not change what the next read sees.
    let again = sns.call("ListSubscriptions", &[]).await;
    assert_eq!(
        again.elements("SubscriptionArn"),
        listed_arns,
        "case \"read twice\": a listing must not change what the next one reports, got {}",
        again.body
    );
    assert_eq!(
        again.elements("Endpoint"),
        listed_endpoints,
        "case \"read twice\": the listing order must be stable, got {}",
        again.body
    );
}

/// What the listing reports FOLLOWS the store: a subscription that is unsubscribed, and
/// one deleted with its topic, leave it; the rest stay.
#[tokio::test]
async fn list_subscriptions_follows_the_store_as_subscriptions_go() {
    let sns = Sns::new();
    let kept = sns.topic("listing-kept").await;
    let doomed = sns.topic("listing-doomed").await;

    let survivor = sns
        .subscribe(&kept, "sqs", "arn:aws:sqs:us-east-1:000000000000:absent-a")
        .await;
    let unsubscribed = sns
        .subscribe(&kept, "sqs", "arn:aws:sqs:us-east-1:000000000000:absent-b")
        .await;
    let with_topic = sns
        .subscribe(
            &doomed,
            "lambda",
            "arn:aws:lambda:us-east-1:000000000000:function:absent-f",
        )
        .await;

    let (listed, _) = sns.all_listed_subscription_arns("three subscriptions").await;
    for arn in [&survivor, &unsubscribed, &with_topic] {
        assert!(
            listed.contains(arn),
            "case \"three subscriptions\": expected {arn:?} in {listed:?}"
        );
    }

    let gone = sns
        .call("Unsubscribe", &[("SubscriptionArn", &unsubscribed)])
        .await;
    assert_eq!(
        gone.status, 200,
        "Unsubscribe: expected 200, got {} / {}",
        gone.status, gone.body
    );
    let deleted = sns.call("DeleteTopic", &[("TopicArn", &doomed)]).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteTopic: expected 200, got {} / {}",
        deleted.status, deleted.body
    );

    let (listed, _) = sns.all_listed_subscription_arns("one subscription left").await;
    assert_eq!(
        listed,
        vec![survivor.clone()],
        "case \"one subscription left\": expected only {survivor:?}, got {listed:?}"
    );
}

/// The listing is paginated at [`PAGE_SIZE`]: the pages partition the account's
/// subscriptions with no repeat and no gap, only the last page has no `NextToken`, and
/// a token the service did not mint is refused with HTTP 400 / `InvalidParameter`.
#[tokio::test]
async fn list_subscriptions_pages_at_the_documented_size_and_refuses_a_token_it_did_not_mint() {
    let sns = Sns::new();
    let first = sns.topic("paged-listing-one").await;
    let second = sns.topic("paged-listing-two").await;

    // Just over two pages, split across two topics, so the paging is the listing's and
    // not a single topic's.
    let total = PAGE_SIZE * 2 + 3;
    let mut minted: HashSet<String> = HashSet::new();
    for index in 0..total {
        let topic = if index % 2 == 0 { &first } else { &second };
        let endpoint = format!("arn:aws:sqs:us-east-1:000000000000:absent-queue-{index:04}");
        let arn = sns.subscribe(topic, "sqs", &endpoint).await;
        assert!(
            minted.insert(arn.clone()),
            "Subscribe {endpoint}: every subscription must have its own ARN, {arn:?} repeats"
        );
    }

    let (listed, pages) = sns.all_listed_subscription_arns("just over two pages").await;
    assert_eq!(
        pages, 3,
        "case \"just over two pages\": {total} subscriptions at {PAGE_SIZE} a page is 3 \
         pages, got {pages}"
    );
    assert_eq!(
        listed.len(),
        total,
        "case \"just over two pages\": expected {total} subscriptions over the pages, got {}",
        listed.len()
    );
    let unique: HashSet<&String> = listed.iter().collect();
    assert_eq!(
        unique.len(),
        total,
        "case \"just over two pages\": the pages must not repeat a subscription, got {} \
         distinct of {}",
        unique.len(),
        listed.len()
    );
    assert_eq!(
        unique,
        minted.iter().collect::<HashSet<&String>>(),
        "case \"just over two pages\": the pages must report exactly the subscriptions \
         Subscribe minted"
    );

    // The first page is full and carries a token; the token walks forward.
    let page_one = sns.call("ListSubscriptions", &[]).await;
    assert_eq!(
        page_one.elements("SubscriptionArn").len(),
        PAGE_SIZE,
        "case \"first page\": expected a full page of {PAGE_SIZE}, got {}",
        page_one.elements("SubscriptionArn").len()
    );
    let token = page_one.element("NextToken").unwrap_or_else(|| {
        panic!(
            "case \"first page\": a page with more to come must carry a NextToken, got {}",
            page_one.body
        )
    });
    let page_two = sns.call("ListSubscriptions", &[("NextToken", &token)]).await;
    assert_eq!(
        page_two.status, 200,
        "case \"second page\": expected 200, got {} / {}",
        page_two.status, page_two.body
    );
    let overlap: HashSet<String> = page_one
        .elements("SubscriptionArn")
        .into_iter()
        .collect::<HashSet<String>>()
        .intersection(
            &page_two
                .elements("SubscriptionArn")
                .into_iter()
                .collect::<HashSet<String>>(),
        )
        .cloned()
        .collect();
    assert!(
        overlap.is_empty(),
        "case \"second page\": consecutive pages must not overlap, shared {overlap:?}"
    );
    // The same token read twice reports the same page: the token is a position, not a
    // cursor the service consumes.
    let page_two_again = sns.call("ListSubscriptions", &[("NextToken", &token)]).await;
    assert_eq!(
        page_two_again.elements("SubscriptionArn"),
        page_two.elements("SubscriptionArn"),
        "case \"token read twice\": the same token must report the same page, got {}",
        page_two_again.body
    );

    // Tokens the service never minted: not a position at all, a negative one, a
    // fractional one, and one past the end of the list.
    let refused = [
        "",
        " ",
        "not-a-token",
        "-1",
        "1.5",
        "0x10",
        "+2",
        "999999999",
        "99999999999999999999999999",
        "MA==",
    ];
    for token in refused {
        let answer = sns.call("ListSubscriptions", &[("NextToken", token)]).await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: expected 400, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidParameter",
            "case {token:?}: expected the InvalidParameter code, got {}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            INVALID_NEXT_TOKEN_REFUSAL,
            "case {token:?}: expected the NextToken refusal sentence, got {}",
            answer.body
        );
    }

    // A refused token leaves the listing exactly where it was.
    let (after, pages_after) = sns.all_listed_subscription_arns("after the refusals").await;
    assert_eq!(
        (after.len(), pages_after),
        (total, 3),
        "case \"after the refusals\": the listing must be untouched, got {} over {} pages",
        after.len(),
        pages_after
    );
}
