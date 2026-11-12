// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `BatchDisableStandards`: a subscription named by its
//! `StandardsSubscriptionArn` enters `DELETING` and is then removed, so the standard
//! stops being reported as enabled; an ARN that names no subscription of the caller's
//! hub is a no-op that disables nothing, so a batch disables exactly what it resolves.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// The Region most cases sign for.
const REGION: &str = "us-east-1";

/// A second Region, so "a subscription belongs to one account AND Region" is
/// observable.
const OTHER_REGION: &str = "eu-west-1";

/// How long a case waits for a transition the clock owes (the `PENDING` -> `READY`
/// one, or the removal a disable owes) to be materialised. Far longer than the spans
/// themselves, so a loaded or slow host cannot fail the wait: every wait below looks
/// at the CONDITION in a loop and only gives up when this deadline passes.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a wait pauses between two looks at the condition.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// One standard of the catalog, as a case drives it: the `<slug>` and `<version>` its
/// ARNs carry, the `StandardsProvider` whose resources it evaluates, and whether the
/// standard keeps the partition-wide `ruleset/` ARN form.
struct CatalogEntry {
    slug: &'static str,
    version: &'static str,
    provider: &'static str,
    partition_wide_ruleset: bool,
}

/// Standards from `crate::state::STANDARDS_CATALOG` (the AWS Security Hub user
/// guide's "Security Hub standards reference"), which is also what `DescribeStandards`
/// reports. The rows span what the disable rule generalises over: the evaluated cloud
/// provider, the regional `standards/` and partition-wide `ruleset/` ARN forms, and
/// two slug/version pairs the issue never cited.
const CATALOG: [CatalogEntry; 5] = [
    CatalogEntry {
        slug: "aws-foundational-security-best-practices",
        version: "1.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "cis-aws-foundations-benchmark",
        version: "1.2.0",
        provider: "AWS",
        partition_wide_ruleset: true,
    },
    CatalogEntry {
        slug: "azure-foundational-security-best-practices",
        version: "1.0.0",
        provider: "Azure",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "nist-800-53",
        version: "5.0.0",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
    CatalogEntry {
        slug: "pci-dss",
        version: "4.0.1",
        provider: "AWS",
        partition_wide_ruleset: false,
    },
];

impl CatalogEntry {
    /// The ARN of the standard itself: a standard's ARN carries the Region but no
    /// account, except the one partition-wide ruleset, which carries neither.
    fn standards_arn(&self, region: &str) -> String {
        if self.partition_wide_ruleset {
            format!(
                "arn:aws:securityhub:::ruleset/{}/v/{}",
                self.slug, self.version
            )
        } else {
            format!(
                "arn:aws:securityhub:{region}::standards/{}/v/{}",
                self.slug, self.version
            )
        }
    }

    /// The ARN of this account's subscription to the standard in `region`.
    fn subscription_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:securityhub:{region}:{ACCOUNT_ID}:subscription/{}/v/{}",
            self.slug, self.version
        )
    }

    /// How a case names this standard in its assertion text.
    fn case(&self) -> String {
        format!("{}/v/{}", self.slug, self.version)
    }
}

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request signed for `region` and return `(status, parsed body)`.
    async fn call(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn enable_hub_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/accounts", Some(body)).await
    }

    async fn batch_enable_standards_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/standards/register", Some(body))
            .await
    }

    async fn batch_enable_standards(&self, body: Value) -> (u16, Value) {
        self.batch_enable_standards_in(REGION, body).await
    }

    async fn batch_disable_standards_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/standards/deregister", Some(body))
            .await
    }

    async fn batch_disable_standards(&self, body: Value) -> (u16, Value) {
        self.batch_disable_standards_in(REGION, body).await
    }

    async fn get_enabled_standards_in(&self, region: &str, body: Value) -> (u16, Value) {
        self.call(region, "POST", "/standards/get", Some(body)).await
    }

    async fn get_enabled_standards(&self, body: Value) -> (u16, Value) {
        self.get_enabled_standards_in(REGION, body).await
    }

    /// A harness whose `REGION` hub is subscribed with NO standard enabled, so every
    /// subscription a case observes is one `BatchEnableStandards` created.
    async fn subscribed_without_standards() -> Self {
        let harness = Self::new();
        let (status, body) = harness
            .enable_hub_in(REGION, json!({ "EnableDefaultStandards": false }))
            .await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Enable `standards_arns` and wait until every one of them reports `READY`,
    /// answering the subscription ARNs the enabling call minted.
    async fn enable_and_settle(&self, case: &str, standards_arns: &[String]) -> Vec<String> {
        let (status, body) = self
            .batch_enable_standards(register_request(standards_arns))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchEnableStandards must succeed, got {status} with {body}"
        );
        let minted = returned_subscription_arns(&body);
        let awaited: Vec<String> = minted.clone();
        self.wait_until(case, "every enabled standard reports READY", move |read| {
            let statuses = subscription_statuses(read);
            awaited
                .iter()
                .all(|arn| statuses.get(arn).map(String::as_str) == Some("READY"))
        })
        .await;
        minted
    }

    /// Look at `GetEnabledStandards` until `condition` holds over the body it
    /// answers, or [`SETTLE_DEADLINE`] passes — the wait is on the CONDITION, never on
    /// a fixed pause. Answers the body that satisfied it.
    async fn wait_until<F>(&self, case: &str, expectation: &str, condition: F) -> Value
    where
        F: Fn(&Value) -> bool,
    {
        let started = std::time::Instant::now();
        loop {
            let (status, body) = self.get_enabled_standards(json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetEnabledStandards must succeed, got {status} with {body}"
            );
            if condition(&body) {
                return body;
            }
            assert!(
                started.elapsed() < SETTLE_DEADLINE,
                "case {case:?}: expected {expectation}, last read {body}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A `BatchEnableStandards` request enabling each ARN in `standards_arns`.
fn register_request(standards_arns: &[String]) -> Value {
    json!({
        "StandardsSubscriptionRequests": standards_arns
            .iter()
            .map(|arn| json!({ "StandardsArn": arn }))
            .collect::<Vec<Value>>(),
    })
}

/// A `BatchDisableStandards` request disabling each subscription in
/// `subscription_arns`.
fn deregister_request(subscription_arns: &[String]) -> Value {
    json!({ "StandardsSubscriptionArns": subscription_arns })
}

/// The `StandardsSubscriptionArn`s of a response, in the order it listed them.
fn returned_subscription_arns(body: &Value) -> Vec<String> {
    body["StandardsSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
        .iter()
        .map(|subscription| {
            subscription["StandardsSubscriptionArn"]
                .as_str()
                .unwrap_or_else(|| panic!("every subscription carries an ARN: {body}"))
                .to_string()
        })
        .collect()
}

/// The `StandardsStatus` each subscription of a response reports, by subscription ARN.
fn subscription_statuses(body: &Value) -> BTreeMap<String, String> {
    body["StandardsSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("StandardsSubscriptions must be a list: {body}"))
        .iter()
        .map(|subscription| {
            (
                subscription["StandardsSubscriptionArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                subscription["StandardsStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The error code of a refusal, whichever casing the wire shape carries.
fn error_code(body: &Value) -> Option<&str> {
    body["Code"].as_str().or_else(|| body["code"].as_str())
}

/// Disabling a standard the hub subscribes to puts its subscription in `DELETING` and
/// then takes it away: the standard stops being reported as enabled, and while it is
/// still reported it reports `DELETING`.
#[tokio::test]
async fn disabling_a_standard_removes_its_subscription() {
    // One row per standard: the rows differ in slug, version, evaluated cloud
    // provider and ARN form.
    for entry in &CATALOG {
        let case = entry.case();
        let harness = Harness::subscribed_without_standards().await;
        let standards_arn = entry.standards_arn(REGION);
        let subscription_arn = entry.subscription_arn(REGION);
        harness
            .enable_and_settle(&case, std::slice::from_ref(&standards_arn))
            .await;

        let (status, body) = harness
            .batch_disable_standards(deregister_request(std::slice::from_ref(&subscription_arn)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchDisableStandards must succeed, got {status} with {body}"
        );
        let subscriptions = body["StandardsSubscriptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"));
        assert_eq!(
            subscriptions.len(),
            1,
            "case {case:?}: one disabled subscription yields one entry, got {body}"
        );
        let subscription = &subscriptions[0];
        assert_eq!(
            subscription["StandardsSubscriptionArn"].as_str(),
            Some(subscription_arn.as_str()),
            "case {case:?}: expected {subscription_arn}, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsArn"].as_str(),
            Some(standards_arn.as_str()),
            "case {case:?}: expected StandardsArn {standards_arn}, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsStatus"].as_str(),
            Some("DELETING"),
            "case {case:?}: a just-disabled standard is DELETING, got {subscription}"
        );
        assert_eq!(
            subscription["StandardsControlsUpdatable"].as_str(),
            Some("NOT_READY_FOR_UPDATES"),
            "case {case:?}: a DELETING standard's controls are not updatable, got {subscription}"
        );
        assert_eq!(
            subscription["Provider"].as_str(),
            Some(entry.provider),
            "case {case:?}: expected Provider {}, got {subscription}",
            entry.provider
        );

        // Read the disabled standard back through the implemented reader: it is gone,
        // and every look that still reports it reports it as DELETING.
        let read_back = harness
            .wait_until(&case, "the disabled subscription to be removed", |read| {
                match subscription_statuses(read).get(&subscription_arn) {
                    Some(status) => {
                        assert_eq!(
                            status, "DELETING",
                            "case {case:?}: a subscription still reported after its disable \
                             must be DELETING, got {read}"
                        );
                        false
                    }
                    None => true,
                }
            })
            .await;
        let still_enabled = returned_subscription_arns(&read_back);
        assert!(
            still_enabled.is_empty(),
            "case {case:?}: the hub's only standard was disabled, got {still_enabled:?}"
        );
    }
}

/// One call disables every subscription it names, whatever the batch's size, and
/// leaves the standards it does not name enabled.
#[tokio::test]
async fn one_call_disables_every_subscription_it_names() {
    let every_standard: Vec<String> = CATALOG
        .iter()
        .map(|entry| entry.standards_arn(REGION))
        .collect();

    // Rows differ in how many subscriptions the batch names, and whether it repeats
    // one: `(case, indices to disable)`.
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("a single subscription", vec![3]),
        ("two subscriptions", vec![0, 2]),
        ("every enabled subscription", (0..CATALOG.len()).collect()),
        (
            "the modelled maximum of 25 entries, with repeats",
            (0..25).map(|index| index % CATALOG.len()).collect(),
        ),
        ("one subscription named twice", vec![4, 4]),
    ];

    for (case, indices) in cases {
        let harness = Harness::subscribed_without_standards().await;
        harness.enable_and_settle(case, &every_standard).await;

        let requested: Vec<String> = indices
            .iter()
            .map(|index| CATALOG[*index].subscription_arn(REGION))
            .collect();
        let expected_disabled: BTreeSet<String> = requested.iter().cloned().collect();
        let (status, body) = harness
            .batch_disable_standards(deregister_request(&requested))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchDisableStandards must succeed, got {status} with {body}"
        );
        let returned: BTreeSet<String> = returned_subscription_arns(&body).into_iter().collect();
        assert_eq!(
            returned, expected_disabled,
            "case {case:?}: expected {expected_disabled:?}, got {returned:?}"
        );
        for status in subscription_statuses(&body).values() {
            assert_eq!(
                status, "DELETING",
                "case {case:?}: every disabled subscription is DELETING, got {body}"
            );
        }

        let expected_remaining: BTreeSet<String> = (0..CATALOG.len())
            .filter(|index| !indices.contains(index))
            .map(|index| CATALOG[index].subscription_arn(REGION))
            .collect();
        let read_back = harness
            .wait_until(case, "the disabled subscriptions to be removed", |read| {
                let listed: BTreeSet<String> =
                    returned_subscription_arns(read).into_iter().collect();
                listed.is_disjoint(&expected_disabled)
            })
            .await;
        let remaining: BTreeSet<String> = returned_subscription_arns(&read_back)
            .into_iter()
            .collect();
        assert_eq!(
            remaining, expected_remaining,
            "case {case:?}: the standards the batch did not name stay enabled: expected \
             {expected_remaining:?}, got {remaining:?}"
        );
        for (arn, status) in subscription_statuses(&read_back) {
            assert_eq!(
                status, "READY",
                "case {case:?}: a standard that was not disabled keeps its status, got \
                 {status} for {arn}"
            );
        }
    }
}

/// A `StandardsSubscriptionArn` that names no subscription of the caller's hub asks
/// for a state that already holds, so the call succeeds, reports nothing for it and
/// leaves the hub's standards alone.
#[tokio::test]
async fn an_arn_naming_no_subscription_is_a_no_op() {
    let harness = Harness::subscribed_without_standards().await;
    let entry = &CATALOG[0];
    let standards_arn = entry.standards_arn(REGION);
    harness
        .enable_and_settle("one enabled standard", std::slice::from_ref(&standards_arn))
        .await;

    let cases: Vec<(&str, String)> = vec![
        ("not an ARN at all", "not-a-subscription".to_string()),
        (
            "the standard's ARN instead of the subscription's",
            standards_arn.clone(),
        ),
        (
            "a subscription to a standard this hub has not enabled",
            CATALOG[3].subscription_arn(REGION),
        ),
        (
            "the subscription ARN of another Region",
            entry.subscription_arn(OTHER_REGION),
        ),
        (
            "a subscription ARN carrying another account id",
            format!(
                "arn:aws:securityhub:{REGION}:999999999999:subscription/{}/v/{}",
                entry.slug, entry.version
            ),
        ),
        (
            "a slug no published standard has",
            format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:subscription/not-a-standard/v/1.0.0"),
        ),
    ];

    for (case, subscription_arn) in cases {
        let (status, body) = harness
            .batch_disable_standards(deregister_request(std::slice::from_ref(&subscription_arn)))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchDisableStandards must succeed, got {status} with {body}"
        );
        let returned = returned_subscription_arns(&body);
        assert!(
            returned.is_empty(),
            "case {case:?}: an ARN naming no subscription disables nothing, got {returned:?}"
        );
    }

    // The enabled standard survived every no-op.
    let (status, read_back) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {read_back}"
    );
    assert_eq!(
        returned_subscription_arns(&read_back),
        vec![entry.subscription_arn(REGION)],
        "a disable that resolved nothing must leave the hub's standards enabled, got {read_back}"
    );
    for (arn, status) in subscription_statuses(&read_back) {
        assert_eq!(
            status, "READY",
            "a disable that resolved nothing must not move a status, got {status} for {arn}"
        );
    }
}

/// A batch mixing ARNs that resolve with ones that do not disables exactly the
/// subscriptions it resolved, wherever the unresolvable entry sits.
#[tokio::test]
async fn a_batch_disables_the_subscriptions_it_resolves() {
    let unknown = format!(
        "arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:subscription/not-a-standard/v/1.0.0"
    );
    let enabled: Vec<String> = CATALOG
        .iter()
        .map(|entry| entry.standards_arn(REGION))
        .collect();

    // Rows differ in where the unresolvable ARN sits in the batch, and in how many
    // resolvable ARNs sit beside it.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        (
            "unresolvable ARN first",
            vec![unknown.clone(), CATALOG[0].subscription_arn(REGION)],
            vec![0],
        ),
        (
            "unresolvable ARN last",
            vec![CATALOG[1].subscription_arn(REGION), unknown.clone()],
            vec![1],
        ),
        (
            "unresolvable ARN between two resolvable ones",
            vec![
                CATALOG[2].subscription_arn(REGION),
                unknown.clone(),
                CATALOG[4].subscription_arn(REGION),
            ],
            vec![2, 4],
        ),
        (
            "nothing but unresolvable ARNs",
            vec![unknown.clone(), "not-a-subscription".to_string()],
            vec![],
        ),
    ];

    for (case, requested, disabled_indices) in cases {
        let harness = Harness::subscribed_without_standards().await;
        harness.enable_and_settle(case, &enabled).await;
        let expected_disabled: BTreeSet<String> = disabled_indices
            .iter()
            .map(|index| CATALOG[*index].subscription_arn(REGION))
            .collect();
        let expected_remaining: BTreeSet<String> = (0..CATALOG.len())
            .filter(|index| !disabled_indices.contains(index))
            .map(|index| CATALOG[index].subscription_arn(REGION))
            .collect();

        let (status, body) = harness
            .batch_disable_standards(deregister_request(&requested))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchDisableStandards must succeed, got {status} with {body}"
        );
        let returned: BTreeSet<String> = returned_subscription_arns(&body).into_iter().collect();
        assert_eq!(
            returned, expected_disabled,
            "case {case:?}: expected {expected_disabled:?} to be disabled, got {returned:?}"
        );

        let read_back = harness
            .wait_until(case, "the disabled subscriptions to be removed", |read| {
                let listed: BTreeSet<String> =
                    returned_subscription_arns(read).into_iter().collect();
                listed.is_disjoint(&expected_disabled)
            })
            .await;
        let persisted: BTreeSet<String> = returned_subscription_arns(&read_back)
            .into_iter()
            .collect();
        assert_eq!(
            persisted, expected_remaining,
            "case {case:?}: the standards the batch did not resolve stay enabled: expected \
             {expected_remaining:?}, got {persisted:?}"
        );
        for (arn, status) in subscription_statuses(&read_back) {
            assert_eq!(
                status, "READY",
                "case {case:?}: a standard that was not disabled keeps its status, got \
                 {status} for {arn}"
            );
        }
    }
}

/// Disabling a standard needs a hub: an account that has not subscribed to Security
/// Hub CSPM in the request's Region is refused, whatever ARNs the batch names.
#[tokio::test]
async fn disabling_needs_a_hub_in_the_requests_region() {
    let harness = Harness::new();
    let subscription_arn = CATALOG[0].subscription_arn(REGION);

    for region in [REGION, OTHER_REGION] {
        let (status, body) = harness
            .batch_disable_standards_in(
                region,
                deregister_request(std::slice::from_ref(&subscription_arn)),
            )
            .await;
        assert_eq!(
            status, 401,
            "case {region:?}: an unsubscribed account must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            Some("InvalidAccessException"),
            "case {region:?}: expected InvalidAccessException, got {body}"
        );
    }

    let harness = Harness::subscribed_without_standards().await;
    harness
        .enable_and_settle(
            "a standard enabled in one Region",
            std::slice::from_ref(&CATALOG[0].standards_arn(REGION)),
        )
        .await;
    let (status, body) = harness
        .batch_disable_standards_in(
            OTHER_REGION,
            deregister_request(std::slice::from_ref(&subscription_arn)),
        )
        .await;
    assert_eq!(
        status, 401,
        "the other Region is still unsubscribed, got {status} with {body}"
    );
    let (status, read_back) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "GetEnabledStandards must succeed, got {status} with {read_back}"
    );
    assert_eq!(
        returned_subscription_arns(&read_back),
        vec![subscription_arn],
        "a standard stays enabled in the Region it was enabled in, got {read_back}"
    );
}

/// Disabling is idempotent while the removal is pending, and a standard enabled again
/// after its disable subscribes afresh rather than being taken away by the removal the
/// disable owed.
#[tokio::test]
async fn a_repeat_disable_reports_the_same_subscription() {
    let harness = Harness::subscribed_without_standards().await;
    let entry = &CATALOG[4];
    let standards_arn = entry.standards_arn(REGION);
    let subscription_arn = entry.subscription_arn(REGION);
    harness
        .enable_and_settle("the standard to disable", std::slice::from_ref(&standards_arn))
        .await;

    let (status, body) = harness
        .batch_disable_standards(deregister_request(std::slice::from_ref(&subscription_arn)))
        .await;
    assert_eq!(
        status, 200,
        "the first disable must succeed, got {status} with {body}"
    );
    let returned = returned_subscription_arns(&body);
    assert_eq!(
        returned,
        vec![subscription_arn.clone()],
        "the first disable reports one subscription, got {returned:?}"
    );
    assert_eq!(
        body["StandardsSubscriptions"][0]["StandardsStatus"].as_str(),
        Some("DELETING"),
        "a disabled subscription reports DELETING, got {body}"
    );

    // A repeat asks for a state the first call already put in motion, so it succeeds
    // either way: it reports the subscription as it stands while the removal is
    // pending, and reports nothing once that removal has fallen due and taken the
    // subscription. Which one a repeat sees depends only on the clock.
    let (status, body) = harness
        .batch_disable_standards(deregister_request(std::slice::from_ref(&subscription_arn)))
        .await;
    assert_eq!(
        status, 200,
        "a repeated disable must succeed, got {status} with {body}"
    );
    let returned = returned_subscription_arns(&body);
    if returned.is_empty() {
        assert!(
            subscription_statuses(&body).is_empty(),
            "a repeat after the removal fell due reports nothing, got {body}"
        );
    } else {
        assert_eq!(
            returned,
            vec![subscription_arn.clone()],
            "a repeated disable reports the same subscription, got {returned:?}"
        );
        assert_eq!(
            body["StandardsSubscriptions"][0]["StandardsStatus"].as_str(),
            Some("DELETING"),
            "a repeated disable does not change the status, got {body}"
        );
    }

    // Enabling the standard again subscribes afresh: the new subscription reaches
    // READY and stays, so the removal the disable owed took only the subscription it
    // was asked for.
    let (status, body) = harness
        .batch_enable_standards(register_request(std::slice::from_ref(&standards_arn)))
        .await;
    assert_eq!(
        status, 200,
        "re-enabling the standard must succeed, got {status} with {body}"
    );
    assert_eq!(
        body["StandardsSubscriptions"][0]["StandardsStatus"].as_str(),
        Some("PENDING"),
        "a standard enabled again is a new subscription, got {body}"
    );
    let case = "the re-enabled standard";
    harness
        .wait_until(case, "the re-enabled standard to report READY", |read| {
            subscription_statuses(read)
                .get(&subscription_arn)
                .map(String::as_str)
                == Some("READY")
        })
        .await;
    let (status, read_back) = harness.get_enabled_standards(json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEnabledStandards must succeed, got {status} with {read_back}"
    );
    assert_eq!(
        returned_subscription_arns(&read_back),
        vec![subscription_arn],
        "case {case:?}: the re-enabled standard stays enabled, got {read_back}"
    );
}

/// A request outside the modelled input is refused with HTTP 400, at each published
/// bound of `StandardsSubscriptionArns` and one past it.
#[tokio::test]
async fn requests_outside_the_modelled_input_are_refused() {
    let harness = Harness::subscribed_without_standards().await;
    let enabled: Vec<String> = CATALOG
        .iter()
        .map(|entry| entry.standards_arn(REGION))
        .collect();
    harness.enable_and_settle("every standard", &enabled).await;
    let subscription_arn = CATALOG[0].subscription_arn(REGION);

    // `StandardsSubscriptionArns` is `@length(min: 1, max: 25)` and its member is
    // `NonEmptyString` with `@pattern("\\S")`.
    let cases: Vec<(&str, Value)> = vec![
        ("no StandardsSubscriptionArns member", json!({})),
        (
            "an empty batch, one below the published minimum",
            json!({ "StandardsSubscriptionArns": [] }),
        ),
        (
            "26 entries, one above the published maximum",
            deregister_request(
                &(0..26)
                    .map(|index| CATALOG[index % CATALOG.len()].subscription_arn(REGION))
                    .collect::<Vec<String>>(),
            ),
        ),
        (
            "a blank StandardsSubscriptionArn",
            json!({ "StandardsSubscriptionArns": [" "] }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = harness.batch_disable_standards(request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    // Each published batch bound is accepted: one entry, and twenty-five. Each bound
    // gets its own hub, so no case addresses a subscription an earlier one disabled.
    let bounds: Vec<(&str, Vec<String>)> = vec![
        (
            "the published minimum batch",
            vec![subscription_arn.clone()],
        ),
        (
            "the published maximum batch",
            (0..25)
                .map(|index| CATALOG[index % CATALOG.len()].subscription_arn(REGION))
                .collect(),
        ),
    ];
    for (case, requested) in bounds {
        let harness = Harness::subscribed_without_standards().await;
        harness.enable_and_settle(case, &enabled).await;
        let (status, body) = harness
            .batch_disable_standards(deregister_request(&requested))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a published bound must be accepted, got {status} with {body}"
        );
    }
}
