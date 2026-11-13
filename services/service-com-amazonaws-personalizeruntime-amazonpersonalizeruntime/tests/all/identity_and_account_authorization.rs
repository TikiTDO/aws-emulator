// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER of an inference request may make it at all, driven end to end over
//! the wire with the services that hold the identity and the policies in force on it.
//!
//! The rule these cases assert, one level above the two recorded ones (one cross-account
//! `campaignArn`, one role without the action):
//!
//! > Every operation of this service is authorized BEFORE any resource the request names is
//! > looked up, on two things and nothing else. (1) The ACCOUNT each resource ARN the
//! > request names belongs to: an ARN of an account that is not the calling principal's is
//! > refused `AccessDeniedException` (HTTP 403) with the sentence naming the principal
//! > alone, whatever the registry holds for it. (2) The identity-based policies in force on
//! > the principal the request's credential names, on the action `personalize:<Operation>`
//! > and the campaign or recommender the request addresses: a call no statement allows
//! > (IAM's default deny) and one an explicit `Deny` covers are both refused the same status
//! > and code, with the sentence naming the principal, the action, that resource and which
//! > of IAM's rules refused it. A request that passes both reaches business logic, where an
//! > ARN the registry cannot resolve is `InvalidInputException` (HTTP 400) — so the two
//! > codes are never interchanged and neither can be used to probe what exists.
//!
//! The rows span what the rule generalises over: every operation, every request member that
//! names a resource ARN (`campaignArn`, `recommenderArn`, `filterArn`, a promotion's own
//! `filterArn`), several foreign accounts including one that HOLDS the named resource, and
//! the statements a policy can carry (an allow of the exact action, a wildcard allow, an
//! allow of another action of the same service, an allow scoped to another campaign, an
//! explicit deny, no policy at all).

use std::sync::Arc;

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime as runtime;
use runtime::state::{self as registry, Campaign, RecipeType, Recommender};

use crate::support::{
    actions, error_code, error_message, items, ACTION_RECOMMENDATIONS, ACCOUNT, HOST,
    PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The region every row's resources are created and addressed in.
const REGION: &str = "us-east-1";

/// The emulator's own published long-term access key: the account credential, which holds
/// no IAM policies.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The status and code AWS refuses a caller that may not make the call with. The code is
/// the service-framework refusal, not an error this operation's model declares
/// (`smithy_query(query="operation",operation="GetRecommendations")` lists
/// `InvalidInputException` and `ResourceNotFoundException` only).
const DENIED_STATUS: u16 = 403;
const DENIED_CODE: &str = "AccessDeniedException";

/// The status and code a request that PASSED authorization and named an unresolvable
/// resource earns — the contrast that makes the refusal above an authorization one.
const UNRESOLVED_STATUS: u16 = 400;
const UNRESOLVED_CODE: &str = "InvalidInputException";

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 12;

/// The names the seeded resources carry, so a foreign-account row can name the SAME
/// resource in another account.
const CAMPAIGN_USER_NAME: &str = "authorized-user";
const CAMPAIGN_RANKING_NAME: &str = "authorized-ranking";
const CAMPAIGN_ACTIONS_NAME: &str = "authorized-actions";
const RECOMMENDER_NAME: &str = "authorized-recommender";
const FILTER_ITEMS_NAME: &str = "authorized-item-filter";
const FILTER_ACTIONS_NAME: &str = "authorized-action-filter";

/// The resource name no seam registered in any account, which is what tells the
/// authorization refusal apart from the business logic's unresolvable-resource refusal.
const UNREGISTERED_NAME: &str = "never-created";

/// A Personalize resource ARN of `account`.
fn arn(account: &str, resource: &str) -> String {
    format!("arn:aws:personalize:{REGION}:{account}:{resource}")
}

/// The campaign `name` of `account`, and the same for the other two resource types.
fn campaign_arn(account: &str, name: &str) -> String {
    arn(account, &format!("campaign/{name}"))
}

fn recommender_arn(account: &str, name: &str) -> String {
    arn(account, &format!("recommender/{name}"))
}

fn filter_arn(account: &str, name: &str) -> String {
    arn(account, &format!("filter/{name}"))
}

/// The router every case drives: this service, plus the two that own the identity a request
/// speaks for (STS) and the policies in force on it (IAM), all on ONE engine.
struct Harness {
    engine: Arc<InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(runtime::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { engine, router }
    }

    /// One inference call, signed with `access_key_id`.
    async fn inference(&self, access_key_id: &str, route: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{HOST}{route}"))
            .header("host", HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/personalize/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One awsQuery call to IAM or STS, signed with `access_key_id`.
    async fn query(
        &self,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encoded(value));
        }
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// The principal `access_key_id` speaks for, as the caller itself reads it back from
    /// `sts:GetCallerIdentity` — so a refusal's expectation is built from the emulator's own
    /// answer about the identity rather than from a spelling restated here.
    async fn principal(&self, access_key_id: &str) -> String {
        let (status, body) = self
            .query(access_key_id, "sts", "2011-06-15", "GetCallerIdentity", &[])
            .await;
        assert_eq!(
            status, 200,
            "setup: GetCallerIdentity must succeed, got {status} {body}"
        );
        xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("setup: GetCallerIdentity must answer an Arn, got {body}"))
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the access key id of the session — the credential a caller
    /// then signs its inference requests with.
    async fn assumed_role_key(&self, role_name: &str, policy: Option<&str>) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
                    ACCOUNT_KEY,
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", policy.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "setup: putting the inline policy of {role_name:?} must succeed, got {status} \
                 {body}"
            );
        }
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: assuming {role_name:?} must succeed, got {status} {body}"
        );
        xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("setup: AssumeRole must answer an AccessKeyId, got {body}"))
    }

    /// How many `recommendationId`s this service has issued — a refused call must issue
    /// none.
    fn issued(&self) -> u64 {
        registry::issued_recommendation_ids(&*self.engine)
            .expect("this service's registry is on the test engine")
            .1
    }
}

/// One form field, percent-encoded the way an awsQuery body carries it.
fn form_encoded(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<name>` element of an awsQuery XML response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// An identity-based policy with one statement.
fn policy(effect: &str, actions: &[&str], resources: &[&str]) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Action": actions,
            "Resource": resources,
        }],
    })
    .to_string()
}

/// An emulator whose registry holds one resource of each kind in the account it serves —
/// and, deliberately, the SAME campaign and filter in a foreign account, so a row proves a
/// cross-account reference is refused without the registry being consulted.
fn seeded(harness: &Harness, foreign: &str) {
    let engine = &*harness.engine;
    let accounts: Vec<&str> = if foreign == ACCOUNT {
        vec![ACCOUNT]
    } else {
        vec![ACCOUNT, foreign]
    };
    for account in accounts {
        for (name, recipe_type) in [
            (CAMPAIGN_USER_NAME, RecipeType::UserPersonalization),
            (CAMPAIGN_RANKING_NAME, RecipeType::PersonalizedRanking),
        ] {
            let held = campaign_arn(account, name);
            assert!(
                registry::register_campaign(
                    engine,
                    Campaign::new(held.as_str(), "demo", recipe_type, items(CATALOG_SIZE))
                ),
                "setup: the campaign seam must register {held}"
            );
        }
        let held = campaign_arn(account, CAMPAIGN_ACTIONS_NAME);
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new_actions(held.as_str(), "actions", actions(CATALOG_SIZE))
            ),
            "setup: the campaign seam must register {held}"
        );
        let held = recommender_arn(account, RECOMMENDER_NAME);
        assert!(
            registry::register_recommender(
                engine,
                Recommender::new(
                    held.as_str(),
                    "top-picks",
                    "Top picks for you",
                    items(CATALOG_SIZE)
                )
            ),
            "setup: the recommender seam must register {held}"
        );
        let held = filter_arn(account, FILTER_ITEMS_NAME);
        assert_eq!(
            registry::register_filter(
                engine,
                held.as_str(),
                "INCLUDE ItemID WHERE Items.YEAR >= 1990"
            ),
            Ok(true),
            "setup: the filter seam must register {held}"
        );
        let held = filter_arn(account, FILTER_ACTIONS_NAME);
        assert_eq!(
            registry::register_filter(
                engine,
                held.as_str(),
                "INCLUDE ActionID WHERE Actions.CHANNEL IN (\"App\")"
            ),
            Ok(true),
            "setup: the filter seam must register {held}"
        );
    }
}

/// One request member that names a resource ARN, and how a request carrying a value in it
/// reads for each operation.
struct Member {
    /// How the row reads in an assertion message.
    case: &'static str,
    /// The operation's modelled route.
    route: &'static str,
    /// The ARN of the resource `name` in `account`, for the resource TYPE this member names.
    resource: fn(&str, &str) -> String,
    /// The name of the resource the seams registered for this member.
    name: &'static str,
    /// The request built around that member's value; every OTHER resource member in it is
    /// filled from the account this emulator serves, so a row varies only the member under
    /// test.
    body: fn(&str) -> Value,
}

/// Every member of every operation that names a resource ARN.
fn members() -> Vec<Member> {
    vec![
        Member {
            case: "GetRecommendations campaignArn",
            route: RECOMMENDATIONS,
            resource: campaign_arn,
            name: CAMPAIGN_USER_NAME,
            body: |value| json!({ "campaignArn": value, "userId": "user-1", "numResults": 3 }),
        },
        Member {
            case: "GetRecommendations recommenderArn",
            route: RECOMMENDATIONS,
            resource: recommender_arn,
            name: RECOMMENDER_NAME,
            body: |value| json!({ "recommenderArn": value, "userId": "user-1" }),
        },
        Member {
            case: "GetRecommendations filterArn",
            route: RECOMMENDATIONS,
            resource: filter_arn,
            name: FILTER_ITEMS_NAME,
            body: |value| {
                json!({
                    "campaignArn": campaign_arn(ACCOUNT, CAMPAIGN_USER_NAME),
                    "userId": "user-1",
                    "filterArn": value,
                })
            },
        },
        Member {
            case: "GetRecommendations promotions[0].filterArn",
            route: RECOMMENDATIONS,
            resource: filter_arn,
            name: FILTER_ITEMS_NAME,
            body: |value| {
                json!({
                    "campaignArn": campaign_arn(ACCOUNT, CAMPAIGN_USER_NAME),
                    "userId": "user-1",
                    "promotions": [{
                        "name": "promo",
                        "percentPromotedItems": 50,
                        "filterArn": value,
                    }],
                })
            },
        },
        Member {
            case: "GetPersonalizedRanking campaignArn",
            route: PERSONALIZE_RANKING,
            resource: campaign_arn,
            name: CAMPAIGN_RANKING_NAME,
            body: |value| {
                json!({
                    "campaignArn": value,
                    "userId": "user-1",
                    "inputList": ["item-1", "item-2"],
                })
            },
        },
        Member {
            case: "GetPersonalizedRanking filterArn",
            route: PERSONALIZE_RANKING,
            resource: filter_arn,
            name: FILTER_ITEMS_NAME,
            body: |value| {
                json!({
                    "campaignArn": campaign_arn(ACCOUNT, CAMPAIGN_RANKING_NAME),
                    "userId": "user-1",
                    "inputList": ["item-1"],
                    "filterArn": value,
                })
            },
        },
        Member {
            case: "GetActionRecommendations campaignArn",
            route: ACTION_RECOMMENDATIONS,
            resource: campaign_arn,
            name: CAMPAIGN_ACTIONS_NAME,
            body: |value| json!({ "campaignArn": value, "userId": "user-1", "numResults": 2 }),
        },
        Member {
            case: "GetActionRecommendations filterArn",
            route: ACTION_RECOMMENDATIONS,
            resource: filter_arn,
            name: FILTER_ACTIONS_NAME,
            body: |value| {
                json!({
                    "campaignArn": campaign_arn(ACCOUNT, CAMPAIGN_ACTIONS_NAME),
                    "userId": "user-1",
                    "filterArn": value,
                })
            },
        },
    ]
}

/// A resource ARN of an account that is not the caller's is refused 403
/// `AccessDeniedException` before anything is resolved, while the identical request against
/// the caller's own account is served — and the same request naming a resource the registry
/// does not hold, in the caller's own account, is the 400 `InvalidInputException` of the
/// business logic.
#[tokio::test]
async fn a_resource_of_another_account_is_refused_without_the_registry_being_consulted() {
    let harness = Harness::new();
    // The accounts a row can name instead of the caller's: the canonical documentation
    // account, one differing from the caller's in a single digit, and one at the other end
    // of the range. The FIRST of them also HOLDS every seeded resource, so a refusal there
    // cannot be a disguised not-found.
    let foreign_accounts = ["123456789012", "000000000001", "999999999999"];
    seeded(&harness, foreign_accounts[0]);
    // Every call below signs with the account's own published credential, which holds no IAM
    // policies and is therefore permitted every action — so the only thing a row's answer
    // can depend on is the account its ARN names.
    let principal = harness.principal(ACCOUNT_KEY).await;
    // The sentence AWS states this refusal in: the calling principal, and nothing else.
    let expected_message = format!("User: {principal} is not authorized to access this resource");

    for member in members() {
        let case = member.case;

        // The caller's own account, holding the resource: served.
        let value = (member.resource)(ACCOUNT, member.name);
        let body = (member.body)(&value);
        let (status, answered) = harness.inference(ACCOUNT_KEY, member.route, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?} (own account): expected HTTP 200, got {status} with {answered}"
        );

        // The caller's own account, naming a resource the registry does not hold: the
        // business logic's refusal, which is a DIFFERENT code and status.
        let missing = (member.resource)(ACCOUNT, UNREGISTERED_NAME);
        let body = (member.body)(&missing);
        let (status, answered) = harness.inference(ACCOUNT_KEY, member.route, &body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (UNRESOLVED_STATUS, UNRESOLVED_CODE),
            "case {case:?} (own account, unregistered): expected HTTP {UNRESOLVED_STATUS} \
             {UNRESOLVED_CODE}, got {status} with {answered}"
        );
        assert_eq!(
            error_message(&answered),
            format!("{missing} does not exist or is not active yet."),
            "case {case:?} (own account, unregistered): expected the registry's sentence, got \
             {answered}"
        );

        // Any other account: refused, whether or not that account's resource is registered.
        for foreign in foreign_accounts {
            let issued_before = harness.issued();
            let value = (member.resource)(foreign, member.name);
            let body = (member.body)(&value);
            let (status, answered) = harness.inference(ACCOUNT_KEY, member.route, &body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (DENIED_STATUS, DENIED_CODE),
                "case {case:?} (account {foreign}): expected HTTP {DENIED_STATUS} \
                 {DENIED_CODE}, got {status} with {answered}"
            );
            assert_eq!(
                error_message(&answered),
                expected_message,
                "case {case:?} (account {foreign}): expected the principal-only sentence, got \
                 {answered}"
            );
            // The refusal names neither the resource nor the action: a caller must learn
            // nothing about an account that is not its own.
            assert!(
                !error_message(&answered).contains(&value),
                "case {case:?} (account {foreign}): the refusal must not echo the ARN, got \
                 {answered}"
            );
            assert_eq!(
                harness.issued(),
                issued_before,
                "case {case:?} (account {foreign}): a refused call must issue no \
                 recommendationId"
            );
        }
    }
}

/// One policy row: what the role carries, and the status each operation owes it.
struct Row {
    case: &'static str,
    role: &'static str,
    /// The inline policy the role carries; `None` for a principal the account grants
    /// nothing.
    policy: Option<String>,
    /// The status `GetRecommendations` owes this caller.
    recommendations_status: u16,
    /// The status `GetPersonalizedRanking` owes it.
    ranking_status: u16,
    /// The status `GetActionRecommendations` owes it.
    actions_status: u16,
    /// Whether a document of this principal's EXPLICITLY denies the call, which is the rule
    /// a refusal names — as against IAM's default deny, where no document refused it and
    /// none allowed it.
    explicitly_denied: bool,
}

/// An inference call is served only to a principal whose identity-based policies allow the
/// action on the resource the request addresses; everyone else gets 403
/// `AccessDeniedException` naming the principal, the action and that resource.
#[tokio::test]
async fn an_inference_call_is_served_only_to_a_principal_iam_permits() {
    let harness = Harness::new();
    // Every resource lives in the account this emulator serves, so what a row varies is the
    // POLICY alone.
    seeded(&harness, ACCOUNT);
    let campaign_user = campaign_arn(ACCOUNT, CAMPAIGN_USER_NAME);
    let campaign_ranking = campaign_arn(ACCOUNT, CAMPAIGN_RANKING_NAME);
    let campaign_actions = campaign_arn(ACCOUNT, CAMPAIGN_ACTIONS_NAME);
    let every_campaign = campaign_arn(ACCOUNT, "*");
    let other_campaign = campaign_arn(ACCOUNT, "elsewhere");

    let rows = vec![
        Row {
            case: "a policy allowing all three actions on every campaign",
            role: "every-action",
            policy: Some(policy(
                "Allow",
                &[
                    "personalize:GetRecommendations",
                    "personalize:GetPersonalizedRanking",
                    "personalize:GetActionRecommendations",
                ],
                &[&every_campaign],
            )),
            recommendations_status: 200,
            ranking_status: 200,
            actions_status: 200,
            explicitly_denied: false,
        },
        Row {
            case: "a wildcard allow of every action on every resource",
            role: "wildcard",
            policy: Some(policy("Allow", &["personalize:*"], &["*"])),
            recommendations_status: 200,
            ranking_status: 200,
            actions_status: 200,
            explicitly_denied: false,
        },
        Row {
            case: "a policy allowing only GetRecommendations",
            role: "recommendations-only",
            policy: Some(policy(
                "Allow",
                &["personalize:GetRecommendations"],
                &["*"],
            )),
            recommendations_status: 200,
            ranking_status: DENIED_STATUS,
            actions_status: DENIED_STATUS,
            explicitly_denied: false,
        },
        Row {
            case: "a policy allowing only another action of this service",
            role: "describe-only",
            policy: Some(policy(
                "Allow",
                &["personalize:DescribeCampaign"],
                &["*"],
            )),
            recommendations_status: DENIED_STATUS,
            ranking_status: DENIED_STATUS,
            actions_status: DENIED_STATUS,
            explicitly_denied: false,
        },
        Row {
            case: "an allow scoped to another campaign",
            role: "other-campaign",
            policy: Some(policy("Allow", &["personalize:*"], &[&other_campaign])),
            recommendations_status: DENIED_STATUS,
            ranking_status: DENIED_STATUS,
            actions_status: DENIED_STATUS,
            explicitly_denied: false,
        },
        Row {
            case: "an explicit deny beside no allow",
            role: "explicit-deny",
            policy: Some(policy("Deny", &["personalize:*"], &["*"])),
            recommendations_status: DENIED_STATUS,
            ranking_status: DENIED_STATUS,
            actions_status: DENIED_STATUS,
            explicitly_denied: true,
        },
        Row {
            case: "an explicit deny of one action beside an allow of every action",
            role: "deny-wins",
            policy: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        { "Effect": "Allow", "Action": "personalize:*", "Resource": "*" },
                        {
                            "Effect": "Deny",
                            "Action": "personalize:GetRecommendations",
                            "Resource": "*",
                        },
                    ],
                })
                .to_string(),
            ),
            recommendations_status: DENIED_STATUS,
            ranking_status: 200,
            actions_status: 200,
            explicitly_denied: true,
        },
        Row {
            case: "no policy at all, which is IAM's default deny",
            role: "no-policy",
            policy: None,
            recommendations_status: DENIED_STATUS,
            ranking_status: DENIED_STATUS,
            actions_status: DENIED_STATUS,
            explicitly_denied: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let key = harness
            .assumed_role_key(row.role, row.policy.as_deref())
            .await;
        let principal = harness.principal(&key).await;

        // (the operation, its route, the body, the resource the action is performed on, the
        // status this row owes it)
        let calls: Vec<(&str, &str, Value, &str, u16)> = vec![
            (
                "GetRecommendations",
                RECOMMENDATIONS,
                json!({ "campaignArn": campaign_user, "userId": "user-1", "numResults": 3 }),
                campaign_user.as_str(),
                row.recommendations_status,
            ),
            (
                "GetPersonalizedRanking",
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": campaign_ranking,
                    "userId": "user-1",
                    "inputList": ["item-1", "item-2"],
                }),
                campaign_ranking.as_str(),
                row.ranking_status,
            ),
            (
                "GetActionRecommendations",
                ACTION_RECOMMENDATIONS,
                json!({ "campaignArn": campaign_actions, "userId": "user-1", "numResults": 2 }),
                campaign_actions.as_str(),
                row.actions_status,
            ),
        ];

        for (operation, route, body, resource, expected) in calls {
            let issued_before = harness.issued();
            let (status, answered) = harness.inference(&key, route, &body).await;
            assert_eq!(
                status, expected,
                "case {case:?}: {operation} expected HTTP {expected}, got {status} with \
                 {answered}"
            );
            if expected != DENIED_STATUS {
                continue;
            }
            assert_eq!(
                error_code(&answered).as_str(),
                DENIED_CODE,
                "case {case:?}: {operation} expected {DENIED_CODE}, got {answered}"
            );
            // The sentence AWS words an identity-based denial in: the principal, the action,
            // the resource the request addressed, and which of IAM's rules refused it.
            let action = format!("personalize:{operation}");
            let reason = if row.explicitly_denied {
                "with an explicit deny in an identity-based policy".to_string()
            } else {
                format!("because no identity-based policy allows the {action} action")
            };
            assert_eq!(
                error_message(&answered),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                ),
                "case {case:?}: {operation} expected IAM's wording, got {answered}"
            );
            assert_eq!(
                harness.issued(),
                issued_before,
                "case {case:?}: {operation} was refused, so it must issue no recommendationId"
            );
        }
    }

    // The account's own published credential holds no IAM policies — the position AWS
    // account root credentials are in — and is served.
    let (status, answered) = harness
        .inference(
            ACCOUNT_KEY,
            RECOMMENDATIONS,
            &json!({ "campaignArn": campaign_user, "userId": "user-1" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the account's own credential\": expected HTTP 200, got {status} with {answered}"
    );
}
